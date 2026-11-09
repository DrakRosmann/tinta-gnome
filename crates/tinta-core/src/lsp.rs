//! A Language Server Protocol client: JSON-RPC over a child process's stdio.

use crate::buffer::Document;
use crate::settings::LspServer;
use ropey::Rope;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pos {
    pub line: usize,
    pub character: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub start: Pos,
    pub end: Pos,
}

/// LSP columns count UTF-16 units, the editor counts chars.
pub fn to_lsp(rope: &Rope, pos: usize) -> Pos {
    let pos = pos.min(rope.len_chars());
    let line = rope.char_to_line(pos);
    let start = rope.line_to_char(line);
    Pos { line, character: rope.char_to_utf16_cu(pos) - rope.char_to_utf16_cu(start) }
}

pub fn from_lsp(rope: &Rope, p: Pos) -> usize {
    if p.line >= rope.len_lines() {
        return rope.len_chars();
    }
    let start = rope.line_to_char(p.line);
    let next = if p.line + 1 < rope.len_lines() { rope.line_to_char(p.line + 1) - 1 } else { rope.len_chars() };
    let start_u = rope.char_to_utf16_cu(start);
    let target = start_u + p.character;
    let end_u = rope.char_to_utf16_cu(next);
    if target >= end_u {
        return next;
    }
    rope.utf16_cu_to_char(target)
}

pub fn path_to_uri(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
    /// Raw JSON, sent back to the server when asking for code actions.
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub range: Range,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub kind: u32,
    pub insert_text: String,
    pub is_snippet: bool,
    pub filter_text: String,
    pub sort_text: String,
    pub edit: Option<TextEdit>,
    pub additional: Vec<TextEdit>,
}

impl CompletionItem {
    /// A short tag for the list, from the LSP kind number.
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            2..=4 => "fn",
            5 | 10 => "field",
            6 | 12 => "var",
            7 | 8 | 22 => "type",
            9 => "mod",
            13 | 20 => "enum",
            14 => "kw",
            15 => "snip",
            21 => "const",
            1 => "text",
            _ => "•",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DocSymbol {
    pub name: String,
    pub detail: String,
    pub kind: u32,
    pub line: usize,
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorkspaceEdit {
    pub changes: Vec<(PathBuf, Vec<TextEdit>)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodeAction {
    pub title: String,
    pub edit: Option<WorkspaceEdit>,
    pub command: Option<(String, Value)>,
    pub preferred: bool,
    /// The raw action, for servers that fill in the edit on `codeAction/resolve`.
    pub raw: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Completion { items: Vec<CompletionItem>, incomplete: bool },
    Hover(String),
    Signature { label: String, active: Option<(usize, usize)>, doc: String },
    Locations(Vec<Location>),
    Edits(Vec<TextEdit>),
    Workspace(WorkspaceEdit),
    Actions(Vec<CodeAction>),
    Action(Box<CodeAction>),
    Symbols(Vec<DocSymbol>),
    Empty,
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Completion,
    Hover,
    Signature,
    Locations,
    Edits,
    Workspace,
    Actions,
    ActionResolve,
    Symbols,
    Other,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Ready { server: String, capabilities: Box<Value> },
    Diagnostics { path: PathBuf, items: Vec<Diagnostic> },
    Reply { server: String, id: i64, reply: Reply },
    ApplyEdit { server: String, id: Value, edit: WorkspaceEdit },
    Progress { server: String, message: Option<String> },
    Message { server: String, severity: u32, text: String },
    Log { server: String, text: String },
    Exited { server: String },
}

pub type Sink = Arc<dyn Fn(Event) + Send + Sync>;

/// Commands tried for each language, in order. First one found on PATH wins.
pub fn default_servers(lsp_id: &str) -> &'static [(&'static str, &'static [&'static str])] {
    match lsp_id {
        "rust" => &[("rust-analyzer", &[])],
        "c" | "cpp" | "objective-c" | "objective-cpp" => &[("clangd", &[])],
        "python" => &[
            ("basedpyright-langserver", &["--stdio"]),
            ("pyright-langserver", &["--stdio"]),
            ("pylsp", &[]),
            ("ruff", &["server"]),
        ],
        "go" => &[("gopls", &[])],
        "javascript" | "typescript" | "javascriptreact" | "typescriptreact" => {
            &[("typescript-language-server", &["--stdio"]), ("vtsls", &["--stdio"])]
        }
        "html" => &[("vscode-html-language-server", &["--stdio"])],
        "css" | "scss" | "less" => &[("vscode-css-language-server", &["--stdio"])],
        "json" => &[("vscode-json-language-server", &["--stdio"])],
        "lua" => &[("lua-language-server", &[])],
        "shellscript" => &[("bash-language-server", &["start"])],
        "zig" => &[("zls", &[])],
        "java" => &[("jdtls", &[])],
        "kotlin" => &[("kotlin-language-server", &[])],
        "csharp" => &[("csharp-ls", &[]), ("OmniSharp", &["-lsp"])],
        "ruby" => &[("ruby-lsp", &[]), ("solargraph", &["stdio"])],
        "php" => &[("intelephense", &["--stdio"]), ("phpactor", &["language-server"])],
        "yaml" => &[("yaml-language-server", &["--stdio"])],
        "toml" => &[("taplo", &["lsp", "stdio"])],
        "dockerfile" => &[("docker-langserver", &["--stdio"])],
        "nix" => &[("nil", &[]), ("nixd", &[])],
        "markdown" => &[("marksman", &["server"])],
        "cmake" => &[("cmake-language-server", &[])],
        "latex" => &[("texlab", &[])],
        "haskell" => &[("haskell-language-server-wrapper", &["--lsp"])],
        "elixir" => &[("elixir-ls", &[])],
        "dart" => &[("dart", &["language-server"])],
        "swift" => &[("sourcekit-lsp", &[])],
        "terraform" => &[("terraform-ls", &["serve"])],
        "ocaml" => &[("ocamllsp", &[])],
        _ => &[],
    }
}

pub fn find_in_path(cmd: &str) -> bool {
    crate::host::find(cmd)
}

/// How to get a language's server when none is installed: its name, and the command that
/// installs it on this system, when there's one worth suggesting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallHint {
    pub server: String,
    pub command: Option<String>,
}

/// Which family of distributions the user's system is, from its os-release: `arch`, `fedora`,
/// `debian`, or empty. In a Flatpak the system's file is under /run/host.
pub fn distro_family() -> &'static str {
    static FAMILY: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    FAMILY.get_or_init(|| {
        let text = ["/run/host/os-release", "/etc/os-release", "/usr/lib/os-release"].iter().find_map(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
        family_of(&text)
    })
}

fn family_of(os_release: &str) -> &'static str {
    let mut ids = Vec::new();
    for line in os_release.lines() {
        if let Some(v) = line.strip_prefix("ID=").or_else(|| line.strip_prefix("ID_LIKE=")) {
            ids.extend(v.trim_matches('"').split_whitespace().map(str::to_string));
        }
    }
    ["arch", "fedora", "debian", "ubuntu"]
        .into_iter()
        .find(|f| ids.iter().any(|i| i == f))
        .map(|f| if f == "ubuntu" { "debian" } else { f })
        .unwrap_or("")
}

/// The hint for a language, on this system.
pub fn install_hint(lsp_id: &str) -> Option<InstallHint> {
    install_hint_for(lsp_id, distro_family(), crate::host::find("rustup"))
}

/// Only packages known to exist under that name are suggested; otherwise the language's own
/// installer, or just the server's name.
pub fn install_hint_for(lsp_id: &str, family: &str, has_rustup: bool) -> Option<InstallHint> {
    let manager = match family {
        "arch" => Some("sudo pacman -S"),
        "fedora" => Some("sudo dnf install"),
        "debian" => Some("sudo apt install"),
        _ => None,
    };
    let package = |arch: Option<&str>, fedora: Option<&str>, debian: Option<&str>| -> Option<String> {
        let pkg = match family {
            "arch" => arch,
            "fedora" => fedora,
            "debian" => debian,
            _ => None,
        }?;
        Some(format!("{} {pkg}", manager?))
    };
    let hint = |server: &str, command: Option<String>| Some(InstallHint { server: server.to_string(), command });
    match lsp_id {
        "rust" if has_rustup => hint("rust-analyzer", Some("rustup component add rust-analyzer".into())),
        "rust" => hint("rust-analyzer", package(Some("rust-analyzer"), Some("rust-analyzer"), None)),
        "c" | "cpp" | "objective-c" | "objective-cpp" => hint("clangd", package(Some("clang"), Some("clang-tools-extra"), Some("clangd"))),
        "python" => match family {
            "arch" => hint("pyright", package(Some("pyright"), None, None)),
            "fedora" | "debian" => hint("pylsp", package(None, Some("python3-lsp-server"), Some("python3-pylsp"))),
            _ => hint("basedpyright", Some("pipx install basedpyright".into())),
        },
        "go" => hint("gopls", package(Some("gopls"), Some("golang-x-tools-gopls"), Some("gopls")).or(Some("go install golang.org/x/tools/gopls@latest".into()))),
        "javascript" | "typescript" | "javascriptreact" | "typescriptreact" => hint(
            "typescript-language-server",
            package(Some("typescript-language-server"), None, None).or(Some("npm install -g typescript-language-server typescript".into())),
        ),
        "html" | "css" | "scss" | "less" | "json" => hint("vscode-langservers-extracted", Some("npm install -g vscode-langservers-extracted".into())),
        "shellscript" => hint("bash-language-server", package(Some("bash-language-server"), None, None).or(Some("npm install -g bash-language-server".into()))),
        "yaml" => hint("yaml-language-server", package(Some("yaml-language-server"), None, None).or(Some("npm install -g yaml-language-server".into()))),
        "lua" => hint("lua-language-server", package(Some("lua-language-server"), None, None)),
        "toml" => hint("taplo", package(Some("taplo-cli"), None, None).or(Some("cargo install taplo-cli --locked".into()))),
        "markdown" => hint("marksman", package(Some("marksman"), None, None)),
        "zig" => hint("zls", package(Some("zls"), None, None)),
        "latex" => hint("texlab", package(Some("texlab"), None, None)),
        other => default_servers(other).first().map(|(c, _)| InstallHint { server: c.to_string(), command: None }),
    }
}

/// Works out which command serves this language, honouring the user's settings.
pub fn resolve_server(lsp_id: &str, overrides: &HashMap<String, LspServer>) -> Option<(String, Vec<String>)> {
    if let Some(o) = overrides.get(lsp_id) {
        if o.command.is_empty() {
            return None; // Explicitly turned off.
        }
        return find_in_path(&o.command).then(|| (o.command.clone(), o.args.clone()));
    }
    default_servers(lsp_id)
        .iter()
        .find(|(c, _)| find_in_path(c))
        .map(|(c, a)| (c.to_string(), a.iter().map(|s| s.to_string()).collect()))
}

/// The project a file belongs to: nearest folder with a project marker, or `.git`.
pub fn find_root(path: &Path, workspace: Option<&Path>) -> PathBuf {
    const MARKERS: &[&str] = &[
        "Cargo.toml", "package.json", "go.mod", "pyproject.toml", "setup.py", "compile_commands.json",
        "CMakeLists.txt", "Makefile", "meson.build", "build.zig", "deno.json", "tsconfig.json", "pom.xml",
        "build.gradle", "mix.exs", "Gemfile", "composer.json", "flake.nix", ".git",
    ];
    let start = path.parent().unwrap_or(path);
    if let Some(w) = workspace
        && start.starts_with(w)
    {
        return w.to_path_buf();
    }
    let mut best = None;
    for dir in start.ancestors() {
        if MARKERS.iter().any(|m| dir.join(m).exists()) {
            // For workspaces (Cargo, npm) the outermost marker below .git is the real root.
            best = Some(dir.to_path_buf());
            if dir.join(".git").exists() {
                break;
            }
        }
    }
    best.unwrap_or_else(|| start.to_path_buf())
}

pub struct Client {
    pub name: String,
    pub server_key: String,
    pub root: PathBuf,
    writer: Sender<Vec<u8>>,
    next_id: AtomicI64,
    pending: Arc<Mutex<HashMap<i64, Kind>>>,
    child: Arc<Mutex<Child>>,
    pub capabilities: Value,
    versions: Mutex<HashMap<PathBuf, i64>>,
}

impl Drop for Client {
    fn drop(&mut self) {
        // Polite first; the reader thread reaps. Then make sure nothing is left running.
        self.request_raw(Kind::Other, "shutdown", Value::Null);
        self.notify("exit", Value::Null);
        if let Ok(mut c) = self.child.lock() {
            let child_pid = c.id();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let _ = Command::new("kill").arg(child_pid.to_string()).stderr(Stdio::null()).status();
            });
            let _ = c.try_wait();
        }
    }
}

impl Client {
    pub fn start(cmd: &str, args: &[String], root: &Path, key: &str, sink: Sink) -> std::io::Result<Self> {
        let mut child = crate::host::command(cmd, args, Some(root), &[])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let (writer, rx) = channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut stdin = stdin;
            while let Ok(buf) = rx.recv() {
                if stdin.write_all(&buf).and_then(|()| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        let pending: Arc<Mutex<HashMap<i64, Kind>>> = Arc::default();
        let name = key.to_string();
        {
            let sink = sink.clone();
            let name = name.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    sink(Event::Log { server: name.clone(), text: line });
                }
            });
        }
        let child = Arc::new(Mutex::new(child));
        {
            let pending = pending.clone();
            let writer = writer.clone();
            let name = name.clone();
            let child = child.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(stdout);
                while let Some(msg) = read_message(&mut r) {
                    handle_message(&msg, &name, &pending, &writer, &sink);
                }
                if let Ok(mut c) = child.lock() {
                    let _ = c.wait();
                }
                sink(Event::Exited { server: name });
            });
        }
        let client = Self {
            name: cmd.to_string(),
            server_key: key.to_string(),
            root: root.to_path_buf(),
            writer,
            next_id: AtomicI64::new(1),
            pending,
            child,
            capabilities: Value::Null,
            versions: Mutex::new(HashMap::new()),
        };
        client.request_raw(Kind::Other, "initialize", initialize_params(root));
        Ok(client)
    }

    fn send(&self, v: &Value) {
        let body = v.to_string();
        let mut buf = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        buf.extend_from_slice(body.as_bytes());
        let _ = self.writer.send(buf);
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request_raw(&self, kind: Kind, method: &str, params: Value) -> i64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut p) = self.pending.lock() {
            p.insert(id, if method == "initialize" { Kind::Other } else { kind });
        }
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    pub fn has_capability(&self, name: &str) -> bool {
        match self.capabilities.get(name) {
            None | Some(Value::Null) | Some(Value::Bool(false)) => false,
            Some(_) => true,
        }
    }

    pub fn trigger_characters(&self, provider: &str) -> Vec<String> {
        self.capabilities
            .get(provider)
            .and_then(|c| c.get("triggerCharacters"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    }

    pub fn did_open(&self, path: &Path, lsp_id: &str, doc: &Document) {
        if let Ok(mut v) = self.versions.lock() {
            v.insert(path.to_path_buf(), 1);
        }
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": path_to_uri(path), "languageId": lsp_id, "version": 1,
                "text": doc.rope.to_string(),
            }}),
        );
    }

    /// How the server wants changes: `true` for one at a time, `false` for the whole text.
    pub fn takes_incremental(&self) -> bool {
        let sync = self.capabilities.get("textDocumentSync");
        let kind = match sync {
            Some(Value::Number(n)) => n.as_u64(),
            Some(Value::Object(o)) => o.get("change").and_then(Value::as_u64),
            _ => None,
        };
        kind == Some(2)
    }

    /// Tells the server what changed. `changes` is `None` when only the whole text will do.
    pub fn did_change(&self, path: &Path, doc: &Document, changes: Option<Vec<crate::buffer::LspChange>>) {
        let version = {
            let Ok(mut v) = self.versions.lock() else { return };
            let e = v.entry(path.to_path_buf()).or_insert(1);
            *e += 1;
            *e
        };
        let content = match changes {
            Some(list) if self.takes_incremental() => list
                .into_iter()
                .map(|c| {
                    json!({
                        "range": {
                            "start": { "line": c.start.line, "character": c.start.character },
                            "end": { "line": c.end.line, "character": c.end.character },
                        },
                        "text": c.text,
                    })
                })
                .collect::<Vec<_>>(),
            _ => vec![json!({ "text": doc.rope.to_string() })],
        };
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": path_to_uri(path), "version": version },
                "contentChanges": content,
            }),
        );
    }

    pub fn did_save(&self, path: &Path, doc: &Document) {
        self.notify(
            "textDocument/didSave",
            json!({ "textDocument": { "uri": path_to_uri(path) }, "text": doc.rope.to_string() }),
        );
    }

    pub fn did_close(&self, path: &Path) {
        if let Ok(mut v) = self.versions.lock() {
            v.remove(path);
        }
        self.notify("textDocument/didClose", json!({ "textDocument": { "uri": path_to_uri(path) } }));
    }

    fn text_position(path: &Path, rope: &Rope, pos: usize) -> Value {
        let p = to_lsp(rope, pos);
        json!({ "textDocument": { "uri": path_to_uri(path) }, "position": { "line": p.line, "character": p.character } })
    }

    pub fn completion(&self, path: &Path, rope: &Rope, pos: usize, trigger: Option<&str>) -> i64 {
        let mut params = Self::text_position(path, rope, pos);
        params["context"] = match trigger {
            Some(t) => json!({ "triggerKind": 2, "triggerCharacter": t }),
            None => json!({ "triggerKind": 1 }),
        };
        self.request_raw(Kind::Completion, "textDocument/completion", params)
    }

    pub fn hover(&self, path: &Path, rope: &Rope, pos: usize) -> i64 {
        self.request_raw(Kind::Hover, "textDocument/hover", Self::text_position(path, rope, pos))
    }

    pub fn signature(&self, path: &Path, rope: &Rope, pos: usize) -> i64 {
        self.request_raw(Kind::Signature, "textDocument/signatureHelp", Self::text_position(path, rope, pos))
    }

    /// `method`: definition, declaration, implementation or typeDefinition.
    pub fn locations(&self, method: &str, path: &Path, rope: &Rope, pos: usize) -> i64 {
        self.request_raw(Kind::Locations, &format!("textDocument/{method}"), Self::text_position(path, rope, pos))
    }

    pub fn references(&self, path: &Path, rope: &Rope, pos: usize) -> i64 {
        let mut params = Self::text_position(path, rope, pos);
        params["context"] = json!({ "includeDeclaration": true });
        self.request_raw(Kind::Locations, "textDocument/references", params)
    }

    pub fn rename(&self, path: &Path, rope: &Rope, pos: usize, new_name: &str) -> i64 {
        let mut params = Self::text_position(path, rope, pos);
        params["newName"] = json!(new_name);
        self.request_raw(Kind::Workspace, "textDocument/rename", params)
    }

    pub fn format(&self, path: &Path, tab: usize, spaces: bool) -> i64 {
        self.request_raw(
            Kind::Edits,
            "textDocument/formatting",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "options": { "tabSize": tab, "insertSpaces": spaces },
            }),
        )
    }

    pub fn code_actions(&self, path: &Path, rope: &Rope, start: usize, end: usize, diags: &[Value]) -> i64 {
        let (a, b) = (to_lsp(rope, start), to_lsp(rope, end));
        self.request_raw(
            Kind::Actions,
            "textDocument/codeAction",
            json!({
                "textDocument": { "uri": path_to_uri(path) },
                "range": { "start": { "line": a.line, "character": a.character }, "end": { "line": b.line, "character": b.character } },
                "context": { "diagnostics": diags },
            }),
        )
    }

    pub fn resolve_action(&self, raw: &Value) -> i64 {
        self.request_raw(Kind::ActionResolve, "codeAction/resolve", raw.clone())
    }

    pub fn execute_command(&self, command: &str, args: &Value) -> i64 {
        self.request_raw(Kind::Other, "workspace/executeCommand", json!({ "command": command, "arguments": args }))
    }

    pub fn symbols(&self, path: &Path) -> i64 {
        self.request_raw(Kind::Symbols, "textDocument/documentSymbol", json!({ "textDocument": { "uri": path_to_uri(path) } }))
    }
}

fn initialize_params(root: &Path) -> Value {
    json!({
        "processId": std::process::id(),
        "rootUri": path_to_uri(root),
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": [{ "uri": path_to_uri(root), "name": root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default() }],
        "clientInfo": { "name": "Tinta", "version": env!("CARGO_PKG_VERSION") },
        "capabilities": {
            "general": { "positionEncodings": ["utf-16"] },
            "textDocument": {
                "synchronization": { "didSave": true, "dynamicRegistration": false },
                "publishDiagnostics": { "relatedInformation": false, "versionSupport": false },
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "completion": {
                    "completionItem": {
                        "snippetSupport": true,
                        "documentationFormat": ["markdown", "plaintext"],
                        "deprecatedSupport": true,
                        "insertReplaceSupport": false,
                    },
                    "completionItemKind": { "valueSet": (1..=25).collect::<Vec<_>>() },
                    "contextSupport": true,
                },
                "signatureHelp": { "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                }},
                "definition": { "linkSupport": true },
                "declaration": { "linkSupport": true },
                "implementation": { "linkSupport": true },
                "typeDefinition": { "linkSupport": true },
                "references": {},
                "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                "formatting": {},
                "rename": { "prepareSupport": false },
                "codeAction": {
                    "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports", "source.fixAll"] } },
                    "resolveSupport": { "properties": ["edit"] },
                    "isPreferredSupport": true,
                },
            },
            "workspace": {
                "applyEdit": true,
                "workspaceEdit": { "documentChanges": true },
                "workspaceFolders": true,
                "configuration": true,
                "didChangeConfiguration": { "dynamicRegistration": false },
            },
            "window": { "workDoneProgress": true, "showMessage": {} },
        },
    })
}

fn read_message(r: &mut impl BufRead) -> Option<Value> {
    let mut len = None;
    let mut line = String::new();
    loop {
        line.clear();
        if r.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let l = line.trim();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.strip_prefix("Content-Length:") {
            len = v.trim().parse::<usize>().ok();
        }
    }
    let mut buf = vec![0; len?];
    r.read_exact(&mut buf).ok()?;
    serde_json::from_slice(&buf).ok()
}

fn respond(writer: &Sender<Vec<u8>>, id: &Value, result: Value) {
    let body = json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string();
    let mut buf = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    buf.extend_from_slice(body.as_bytes());
    let _ = writer.send(buf);
}

fn handle_message(
    msg: &Value,
    server: &str,
    pending: &Mutex<HashMap<i64, Kind>>,
    writer: &Sender<Vec<u8>>,
    sink: &Sink,
) {
    let name = server.to_string();
    match (msg.get("id"), msg.get("method").and_then(Value::as_str)) {
        // A reply to us.
        (Some(id), None) => {
            let Some(id) = id.as_i64() else { return };
            let kind = pending.lock().ok().and_then(|mut p| p.remove(&id)).unwrap_or(Kind::Other);
            if id == 1 {
                // `initialize` is always the first request; finish the handshake here.
                let caps = msg.pointer("/result/capabilities").cloned().unwrap_or(Value::Null);
                let body = json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }).to_string();
                let mut buf = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
                buf.extend_from_slice(body.as_bytes());
                let _ = writer.send(buf);
                sink(Event::Ready { server: name, capabilities: Box::new(caps) });
                return;
            }
            let reply = if let Some(e) = msg.get("error") {
                Reply::Error(e.get("message").and_then(Value::as_str).unwrap_or("error").to_string())
            } else {
                parse_reply(kind, msg.get("result").unwrap_or(&Value::Null))
            };
            sink(Event::Reply { server: name, id, reply });
        }
        // A request from the server: every one needs an answer or some servers hang.
        (Some(id), Some(method)) => match method {
            "workspace/applyEdit" => {
                let edit = msg.pointer("/params/edit").map(parse_workspace_edit).unwrap_or_default();
                sink(Event::ApplyEdit { server: name, id: id.clone(), edit });
                respond(writer, id, json!({ "applied": true }));
            }
            "workspace/configuration" => {
                let n = msg.pointer("/params/items").and_then(Value::as_array).map_or(0, Vec::len);
                respond(writer, id, Value::Array(vec![Value::Null; n]));
            }
            "workspace/workspaceFolders" => respond(writer, id, Value::Null),
            _ => respond(writer, id, Value::Null),
        },
        // A notification.
        (None, Some(method)) => {
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "textDocument/publishDiagnostics" => {
                    let Some(path) = params.get("uri").and_then(Value::as_str).and_then(uri_to_path) else { return };
                    let items = params
                        .get("diagnostics")
                        .and_then(Value::as_array)
                        .map(|a| a.iter().map(parse_diagnostic).collect())
                        .unwrap_or_default();
                    sink(Event::Diagnostics { path, items });
                }
                "window/showMessage" => sink(Event::Message {
                    server: name,
                    severity: params.get("type").and_then(Value::as_u64).unwrap_or(3) as u32,
                    text: params.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                }),
                "window/logMessage" => sink(Event::Log {
                    server: name,
                    text: params.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
                }),
                "$/progress" => {
                    let v = params.get("value").cloned().unwrap_or(Value::Null);
                    let message = match v.get("kind").and_then(Value::as_str) {
                        Some("end") => None,
                        _ => {
                            let title = v.get("title").and_then(Value::as_str).unwrap_or("");
                            let m = v.get("message").and_then(Value::as_str).unwrap_or("");
                            Some(format!("{title} {m}").trim().to_string())
                        }
                    };
                    sink(Event::Progress { server: name, message });
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn parse_pos(v: &Value) -> Pos {
    Pos {
        line: v.get("line").and_then(Value::as_u64).unwrap_or(0) as usize,
        character: v.get("character").and_then(Value::as_u64).unwrap_or(0) as usize,
    }
}

fn parse_range(v: &Value) -> Range {
    Range {
        start: v.get("start").map(parse_pos).unwrap_or_default(),
        end: v.get("end").map(parse_pos).unwrap_or_default(),
    }
}

fn parse_diagnostic(v: &Value) -> Diagnostic {
    Diagnostic {
        range: v.get("range").map(parse_range).unwrap_or_default(),
        severity: match v.get("severity").and_then(Value::as_u64) {
            Some(1) | None => Severity::Error,
            Some(2) => Severity::Warning,
            Some(3) => Severity::Info,
            _ => Severity::Hint,
        },
        message: v.get("message").and_then(Value::as_str).unwrap_or("").to_string(),
        source: v.get("source").and_then(Value::as_str).map(str::to_string),
        code: v.get("code").map(|c| match c {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }),
        raw: v.clone(),
    }
}

fn parse_text_edit(v: &Value) -> Option<TextEdit> {
    Some(TextEdit {
        range: parse_range(v.get("range")?),
        new_text: v.get("newText")?.as_str()?.to_string(),
    })
}

fn parse_edits(v: Option<&Value>) -> Vec<TextEdit> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(parse_text_edit).collect()).unwrap_or_default()
}

/// `markdown`, `plaintext`, or a bare string, or a list of those.
fn markup(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(a) => a.iter().map(markup).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n"),
        Value::Object(o) => {
            let value = o.get("value").and_then(Value::as_str).unwrap_or("");
            match o.get("language").and_then(Value::as_str) {
                Some(l) => format!("```{l}\n{value}\n```"),
                None => value.to_string(),
            }
        }
        _ => String::new(),
    }
}

fn parse_completion_item(v: &Value) -> CompletionItem {
    let label = v.get("label").and_then(Value::as_str).unwrap_or("").to_string();
    let edit = v.get("textEdit").and_then(|t| {
        // InsertReplaceEdit has `insert` instead of `range`.
        let range = t.get("range").or_else(|| t.get("insert"))?;
        Some(TextEdit { range: parse_range(range), new_text: t.get("newText")?.as_str()?.to_string() })
    });
    let insert_text = v
        .get("insertText")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| edit.as_ref().map(|e| e.new_text.clone()))
        .unwrap_or_else(|| label.clone());
    CompletionItem {
        detail: v.get("detail").and_then(Value::as_str).map(str::to_string),
        documentation: v.get("documentation").map(markup).filter(|s| !s.is_empty()),
        kind: v.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32,
        is_snippet: v.get("insertTextFormat").and_then(Value::as_u64) == Some(2),
        filter_text: v.get("filterText").and_then(Value::as_str).unwrap_or(&label).to_string(),
        sort_text: v.get("sortText").and_then(Value::as_str).unwrap_or(&label).to_string(),
        additional: parse_edits(v.get("additionalTextEdits")),
        insert_text,
        edit,
        label,
    }
}

fn parse_location(v: &Value) -> Option<Location> {
    // Location or LocationLink.
    let uri = v.get("uri").or_else(|| v.get("targetUri"))?.as_str()?;
    let range = v.get("range").or_else(|| v.get("targetSelectionRange")).or_else(|| v.get("targetRange"))?;
    Some(Location { path: uri_to_path(uri)?, range: parse_range(range) })
}

pub fn parse_workspace_edit(v: &Value) -> WorkspaceEdit {
    let mut out = WorkspaceEdit::default();
    if let Some(dc) = v.get("documentChanges").and_then(Value::as_array) {
        for d in dc {
            // Skip create/rename/delete file operations: only text edits have textDocument.
            if let Some(uri) = d.pointer("/textDocument/uri").and_then(Value::as_str)
                && let Some(p) = uri_to_path(uri)
            {
                out.changes.push((p, parse_edits(d.get("edits"))));
            }
        }
    } else if let Some(c) = v.get("changes").and_then(Value::as_object) {
        for (uri, edits) in c {
            if let Some(p) = uri_to_path(uri) {
                out.changes.push((p, parse_edits(Some(edits))));
            }
        }
    }
    out
}

fn parse_action(v: &Value) -> Option<CodeAction> {
    let title = v.get("title")?.as_str()?.to_string();
    let command = match v.get("command") {
        Some(Value::String(c)) => Some((c.clone(), v.get("arguments").cloned().unwrap_or(json!([])))),
        Some(Value::Object(o)) => Some((
            o.get("command")?.as_str()?.to_string(),
            o.get("arguments").cloned().unwrap_or(json!([])),
        )),
        _ => None,
    };
    Some(CodeAction {
        title,
        edit: v.get("edit").map(parse_workspace_edit),
        command,
        preferred: v.get("isPreferred").and_then(Value::as_bool).unwrap_or(false),
        raw: v.clone(),
    })
}

fn flatten_symbols(v: &Value, depth: usize, out: &mut Vec<DocSymbol>) {
    let Some(a) = v.as_array() else { return };
    for s in a {
        let name = s.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let range = s.get("selectionRange").or_else(|| s.pointer("/location/range"));
        out.push(DocSymbol {
            name,
            detail: s.get("detail").and_then(Value::as_str).unwrap_or("").to_string(),
            kind: s.get("kind").and_then(Value::as_u64).unwrap_or(0) as u32,
            line: range.map(parse_range).unwrap_or_default().start.line,
            depth,
        });
        if let Some(children) = s.get("children") {
            flatten_symbols(children, depth + 1, out);
        }
    }
}

pub fn parse_reply(kind: Kind, r: &Value) -> Reply {
    match kind {
        Kind::Completion => {
            let (list, incomplete) = match r {
                Value::Array(a) => (a.clone(), false),
                Value::Object(o) => (
                    o.get("items").and_then(Value::as_array).cloned().unwrap_or_default(),
                    o.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false),
                ),
                _ => (Vec::new(), false),
            };
            Reply::Completion { items: list.iter().map(parse_completion_item).collect(), incomplete }
        }
        Kind::Hover => match r.get("contents") {
            Some(c) => Reply::Hover(markup(c)),
            None => Reply::Empty,
        },
        Kind::Signature => {
            let sigs = r.get("signatures").and_then(Value::as_array);
            let Some(sig) = sigs.and_then(|s| {
                let i = r.get("activeSignature").and_then(Value::as_u64).unwrap_or(0) as usize;
                s.get(i).or(s.first())
            }) else {
                return Reply::Empty;
            };
            let label = sig.get("label").and_then(Value::as_str).unwrap_or("").to_string();
            let idx = sig
                .get("activeParameter")
                .or_else(|| r.get("activeParameter"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let active = sig.get("parameters").and_then(Value::as_array).and_then(|p| p.get(idx)).and_then(|p| {
                match p.get("label")? {
                    Value::String(s) => label.find(s.as_str()).map(|b| {
                        let a = label[..b].chars().count();
                        (a, a + s.chars().count())
                    }),
                    Value::Array(a) => {
                        Some((a.first()?.as_u64()? as usize, a.get(1)?.as_u64()? as usize))
                    }
                    _ => None,
                }
            });
            Reply::Signature { label, active, doc: sig.get("documentation").map(markup).unwrap_or_default() }
        }
        Kind::Locations => {
            let list = match r {
                Value::Array(a) => a.iter().filter_map(parse_location).collect(),
                Value::Null => Vec::new(),
                single => parse_location(single).into_iter().collect(),
            };
            Reply::Locations(list)
        }
        Kind::Edits => Reply::Edits(parse_edits(Some(r))),
        Kind::Workspace => Reply::Workspace(parse_workspace_edit(r)),
        Kind::Actions => Reply::Actions(
            r.as_array().map(|a| a.iter().filter_map(parse_action).collect()).unwrap_or_default(),
        ),
        Kind::ActionResolve => parse_action(r).map(|a| Reply::Action(Box::new(a))).unwrap_or(Reply::Empty),
        Kind::Symbols => {
            let mut out = Vec::new();
            flatten_symbols(r, 0, &mut out);
            Reply::Symbols(out)
        }
        Kind::Other => Reply::Empty,
    }
}

/// Text edits as editor edits. Assumes they do not overlap, which the protocol guarantees.
pub fn to_edits(rope: &Rope, edits: &[TextEdit]) -> Vec<crate::buffer::Edit> {
    edits
        .iter()
        .map(|e| {
            let text = crate::buffer::normalize_newlines(&e.new_text).into_owned();
            crate::buffer::Edit::new(from_lsp(rope, e.range.start), from_lsp(rope, e.range.end), text)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hints_follow_the_distribution() {
        assert_eq!(family_of("ID=cachyos\nID_LIKE=arch\n"), "arch");
        assert_eq!(family_of("ID=ubuntu\nID_LIKE=debian\n"), "debian");
        assert_eq!(family_of("ID=\"fedora\"\n"), "fedora");
        assert_eq!(family_of("ID=nixos\n"), "");
        let cmd = |id, fam, rustup| install_hint_for(id, fam, rustup).and_then(|h| h.command);
        assert_eq!(cmd("c", "arch", false).as_deref(), Some("sudo pacman -S clang"));
        assert_eq!(cmd("cpp", "fedora", false).as_deref(), Some("sudo dnf install clang-tools-extra"));
        assert_eq!(cmd("c", "debian", false).as_deref(), Some("sudo apt install clangd"));
        assert_eq!(cmd("rust", "debian", true).as_deref(), Some("rustup component add rust-analyzer"));
        assert_eq!(cmd("go", "", false).as_deref(), Some("go install golang.org/x/tools/gopls@latest"));
        // Nothing to suggest on an unknown system: the server's name is still there.
        assert_eq!(install_hint_for("c", "", false), Some(InstallHint { server: "clangd".into(), command: None }));
        assert_eq!(install_hint_for("haskell", "arch", false).map(|h| h.server).as_deref(), Some("haskell-language-server-wrapper"));
        assert_eq!(install_hint_for("plaintext", "arch", false), None);
    }
    use std::sync::mpsc::{Receiver, channel};
    use std::time::Duration;

    #[test]
    fn utf16_columns_convert_both_ways() {
        let r = Rope::from_str("a😀b\nação\nend");
        // The emoji is one char and two UTF-16 units.
        assert_eq!(to_lsp(&r, 2), Pos { line: 0, character: 3 });
        assert_eq!(from_lsp(&r, Pos { line: 0, character: 3 }), 2);
        assert_eq!(from_lsp(&r, Pos { line: 1, character: 4 }), 8);
        // Past the end clamps to the line's end, not the next line.
        assert_eq!(from_lsp(&r, Pos { line: 0, character: 99 }), 3);
        assert_eq!(from_lsp(&r, Pos { line: 99, character: 0 }), r.len_chars());
    }

    #[test]
    fn uris_round_trip_with_odd_characters() {
        let p = Path::new("/tmp/pasta com espaço/a#b.rs");
        let u = path_to_uri(p);
        assert!(u.starts_with("file:///tmp/pasta%20com%20espa"));
        assert_eq!(uri_to_path(&u).unwrap(), p);
    }

    #[test]
    fn messages_are_framed() {
        let body = r#"{"a":1}"#;
        let raw = format!("Content-Length: {}\r\nContent-Type: x\r\n\r\n{body}", body.len());
        let mut c = std::io::Cursor::new(raw.into_bytes());
        assert_eq!(read_message(&mut c).unwrap(), json!({ "a": 1 }));
        assert!(read_message(&mut c).is_none());
    }

    #[test]
    fn parses_replies() {
        let r = parse_reply(Kind::Hover, &json!({ "contents": { "kind": "markdown", "value": "**x**" } }));
        assert_eq!(r, Reply::Hover("**x**".into()));
        let r = parse_reply(Kind::Completion, &json!({ "isIncomplete": true, "items": [
            { "label": "foo", "kind": 3, "insertText": "foo($1)", "insertTextFormat": 2 },
            { "label": "bar", "textEdit": { "range": { "start": {"line":0,"character":0}, "end": {"line":0,"character":1} }, "newText": "barre" } },
        ]}));
        let Reply::Completion { items, incomplete } = r else { panic!() };
        assert!(incomplete && items[0].is_snippet);
        assert_eq!(items[0].kind_label(), "fn");
        assert_eq!(items[1].insert_text, "barre");
        let r = parse_reply(Kind::Locations, &json!({ "uri": "file:///a.rs", "range": { "start": {"line":1,"character":2}, "end": {"line":1,"character":3} } }));
        let Reply::Locations(l) = r else { panic!() };
        assert_eq!(l[0].range.start.line, 1);
        let r = parse_reply(Kind::Locations, &json!([{ "targetUri": "file:///b.rs", "targetRange": { "start": {"line":5,"character":0}, "end": {"line":6,"character":0} } }]));
        let Reply::Locations(l) = r else { panic!() };
        assert_eq!(l[0].path, Path::new("/b.rs"));
        let r = parse_reply(Kind::Signature, &json!({ "signatures": [{ "label": "f(a: i32, b: u8)", "parameters": [{ "label": "a: i32" }, { "label": [10, 15] }] }], "activeParameter": 1 }));
        assert!(matches!(r, Reply::Signature { active: Some((10, 15)), .. }));
        let r = parse_reply(Kind::Symbols, &json!([{ "name": "A", "kind": 5, "selectionRange": { "start": {"line":2,"character":0}, "end": {"line":2,"character":1} }, "children": [{ "name": "b", "kind": 6, "selectionRange": { "start": {"line":3,"character":0}, "end": {"line":3,"character":1} } }] }]));
        let Reply::Symbols(s) = r else { panic!() };
        assert_eq!((s[1].name.as_str(), s[1].depth, s[1].line), ("b", 1, 3));
        let e = parse_workspace_edit(&json!({ "changes": { "file:///c.rs": [{ "range": { "start": {"line":0,"character":0}, "end": {"line":0,"character":1} }, "newText": "x" }] } }));
        assert_eq!(e.changes[0].1[0].new_text, "x");
        let e = parse_workspace_edit(&json!({ "documentChanges": [{ "textDocument": { "uri": "file:///d.rs" }, "edits": [] }, { "kind": "create", "uri": "file:///e.rs" }] }));
        assert_eq!(e.changes.len(), 1);
    }

    fn collect() -> (Sink, Receiver<Event>) {
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        (Arc::new(move |e| { let _ = tx.lock().unwrap().send(e); }), rx)
    }

    fn wait(rx: &Receiver<Event>, mut f: impl FnMut(&Event) -> bool) -> Event {
        loop {
            let e = rx.recv_timeout(Duration::from_secs(30)).expect("server answered in time");
            if f(&e) {
                return e;
            }
        }
    }

    /// Against the real thing, when it is installed: diagnostics, completion, hover, definition,
    /// rename and formatting, end to end.
    #[test]
    fn clangd_end_to_end() {
        if !find_in_path("clangd") {
            eprintln!("clangd not installed, skipping");
            return;
        }
        let dir = std::env::temp_dir().join(format!("tinta-lsp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.c");
        // The error stays on its own line: older clangd drops a C expression with an error in it
        // from its tree, and then has nothing to say about the rest of that expression.
        let src = "int soma(int a, int b) { return a + b; }\nint main(void) {\n  int e = nada;\n  return soma(1, 2) + e;\n}\n";
        std::fs::write(&file, src).unwrap();
        let doc = Document::open(&file, crate::buffer::Indent { spaces: true, width: 4 }).unwrap();
        let (sink, rx) = collect();
        let mut c = Client::start("clangd", &[], &dir, "clangd", sink).unwrap();
        let Event::Ready { capabilities, .. } = wait(&rx, |e| matches!(e, Event::Ready { .. })) else { panic!() };
        c.capabilities = *capabilities;
        assert!(c.has_capability("completionProvider"));
        assert!(c.trigger_characters("completionProvider").contains(&".".to_string()));
        c.did_open(&file, "c", &doc);

        let Event::Diagnostics { items, path } = wait(&rx, |e| matches!(e, Event::Diagnostics { items, .. } if !items.is_empty())) else { panic!() };
        assert_eq!(std::fs::canonicalize(path).unwrap(), std::fs::canonicalize(&file).unwrap());
        assert!(items[0].message.contains("nada"), "{}", items[0].message);
        assert_eq!(items[0].severity, Severity::Error);

        let call = doc.pos(3, 9);
        let id = c.hover(&file, &doc.rope, call);
        let Event::Reply { reply, .. } = wait(&rx, |e| matches!(e, Event::Reply { id: i, .. } if *i == id)) else { panic!() };
        let Reply::Hover(text) = reply else { panic!("{reply:?}") };
        assert!(text.contains("soma"), "{text}");

        let id = c.locations("definition", &file, &doc.rope, call);
        let Event::Reply { reply, .. } = wait(&rx, |e| matches!(e, Event::Reply { id: i, .. } if *i == id)) else { panic!() };
        let Reply::Locations(l) = reply else { panic!("{reply:?}") };
        assert_eq!(l[0].range.start.line, 0);

        let id = c.completion(&file, &doc.rope, doc.pos(3, 11), None);
        let Event::Reply { reply, .. } = wait(&rx, |e| matches!(e, Event::Reply { id: i, .. } if *i == id)) else { panic!() };
        let Reply::Completion { items, .. } = reply else { panic!("{reply:?}") };
        assert!(items.iter().any(|i| i.label.contains("soma")), "{items:?}");

        let id = c.rename(&file, &doc.rope, call, "adicao");
        let Event::Reply { reply, .. } = wait(&rx, |e| matches!(e, Event::Reply { id: i, .. } if *i == id)) else { panic!() };
        let Reply::Workspace(edit) = reply else { panic!("{reply:?}") };
        assert_eq!(edit.changes[0].1.len(), 2, "definition and call");

        let id = c.format(&file, 4, true);
        let Event::Reply { reply, .. } = wait(&rx, |e| matches!(e, Event::Reply { id: i, .. } if *i == id)) else { panic!() };
        assert!(matches!(reply, Reply::Edits(_)), "{reply:?}");

        let id = c.symbols(&file);
        let Event::Reply { reply, .. } = wait(&rx, |e| matches!(e, Event::Reply { id: i, .. } if *i == id)) else { panic!() };
        let Reply::Symbols(s) = reply else { panic!("{reply:?}") };
        assert!(s.iter().any(|x| x.name.contains("main")));

        // One small edit: the server has to end up with the same text as us.
        assert!(c.takes_incremental(), "clangd takes changes one at a time");
        let mut doc = doc;
        doc.track_lsp = true;
        let mut cur = crate::Cursors::default();
        let at = doc.pos(2, 2);
        doc.apply(vec![crate::Edit::insert(at, "int nada = 0;\n  ")], &mut cur, crate::EditKind::Other, |_, _| {});
        let changes = doc.take_lsp_changes();
        assert_eq!(changes.as_ref().unwrap().len(), 1);
        c.did_change(&file, &doc, changes);
        // The undeclared name is declared now, so its error goes away.
        let Event::Diagnostics { items, .. } = wait(&rx, |e| matches!(e, Event::Diagnostics { items, .. } if !items.iter().any(|d| d.message.contains("nada")))) else { panic!() };
        assert!(items.is_empty(), "{items:?}");

        drop(c);
        wait(&rx, |e| matches!(e, Event::Exited { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
