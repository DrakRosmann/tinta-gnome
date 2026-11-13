//! The Debug Adapter Protocol: a client for any adapter, and the state of a debug session.
//! Nothing here knows about the screen, so the whole flow is testable against a real debugger.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub enum Incoming {
    Message(Value),
    Stderr(String),
    Exited,
    /// A line from the GDB server (OpenOCD and the like) that talks to the board.
    Server(String),
    /// The GDB server couldn't be started, or never got ready.
    ServerFailed(String),
}

pub type Sink = Arc<dyn Fn(Incoming) + Send + Sync>;

// Breakpoints --------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BpState {
    /// No session has said anything about it.
    #[default]
    Unknown,
    Verified,
    /// The debugger couldn't place it (yet).
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breakpoint {
    /// Zero-based.
    pub line: usize,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    #[serde(skip)]
    pub state: BpState,
    #[serde(skip)]
    pub id: Option<i64>,
}

fn enabled_by_default() -> bool {
    true
}

impl Breakpoint {
    pub fn at(line: usize) -> Self {
        Self { line, condition: None, enabled: true, state: BpState::Unknown, id: None }
    }
}

pub type Breakpoints = BTreeMap<PathBuf, Vec<Breakpoint>>;

/// Sets it or clears it. True if there's a breakpoint on the line now.
pub fn toggle_breakpoint(list: &mut Vec<Breakpoint>, line: usize) -> bool {
    if let Some(i) = list.iter().position(|b| b.line == line) {
        list.remove(i);
        false
    } else {
        let at = list.partition_point(|b| b.line < line);
        list.insert(at, Breakpoint::at(line));
        true
    }
}

/// Keeps breakpoints on their code as lines come and go. Lines removed outright take theirs along.
pub fn shift_breakpoints(list: &mut Vec<Breakpoint>, map: impl Fn(usize) -> Option<usize>) {
    list.retain_mut(|b| match map(b.line) {
        Some(l) => {
            b.line = l;
            true
        }
        None => false,
    });
    list.sort_by_key(|b| b.line);
    list.dedup_by_key(|b| b.line);
}

// Configurations -----------------------------------------------------------------------------

/// One entry of `.tinta/launch.json`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LaunchConfig {
    pub name: String,
    /// `gdb`, `lldb`, `python`, `go`, `embedded` (a board, through a GDB server) or `probe-rs`,
    /// and VS Code's names for them (`cortex-debug`, `probe-rs-debug`).
    #[serde(rename = "type")]
    pub kind: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: HashMap<String, String>,
    pub stop_on_entry: bool,
    /// A shell command to run first, like `make`. The session starts only if it succeeds.
    pub build: Option<String>,
    /// Where the program reads and writes, in VS Code's words: `integratedTerminal`, a terminal
    /// of its own in the panel that you can type into, or `internalConsole`, the debug console,
    /// with nothing to read from. Empty: the terminal where the debugger can do it.
    pub console: String,
    /// Anything else goes to the adapter as it is.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub const LAUNCH_PATH: &str = ".tinta/launch.json";

pub const LAUNCH_TEMPLATE: &str = r#"// How to debug this project. Press F5 to start. "type" is gdb, lldb, python or go.
[
  {
    "name": "Debug",
    "type": "gdb",
    "build": "make",
    "program": "${workspaceFolder}/app",
    "args": [],
    "cwd": "${workspaceFolder}",
    "env": {},
    "stopOnEntry": false,
    "console": "integratedTerminal"
  }
]
"#;

/// A list of configurations, or an object with `configurations` as VS Code writes it.
pub fn parse_configs(src: &str) -> Result<Vec<LaunchConfig>, String> {
    let v: Value = serde_json::from_str(&crate::settings::strip_jsonc(src)).map_err(|e| e.to_string())?;
    let list = match v {
        Value::Object(mut o) => o.remove("configurations").unwrap_or(Value::Array(Vec::new())),
        other => other,
    };
    serde_json::from_value(list).map_err(|e| e.to_string())
}

pub fn load_configs(root: &Path) -> Result<Vec<LaunchConfig>, String> {
    match std::fs::read_to_string(root.join(LAUNCH_PATH)) {
        Ok(src) => parse_configs(&src).map_err(|e| format!("{LAUNCH_PATH}: {e}")),
        Err(_) => Ok(Vec::new()),
    }
}

fn normalize_kind(kind: &str) -> &str {
    match kind {
        "gdb" | "cppdbg" => "gdb",
        "lldb" | "lldb-dap" | "codelldb" => "lldb",
        "python" | "debugpy" => "python",
        "go" | "dlv" | "delve" => "go",
        "embedded" | "cortex-debug" => "embedded",
        "probe-rs" | "probe-rs-debug" => "probe-rs",
        other => other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub kind: String,
    pub command: String,
    pub args: Vec<String>,
}

/// The major version of a gdb built with Python.
fn gdb_major() -> Option<u32> {
    gdb_major_of("gdb")
}

fn gdb_major_of(gdb: &str) -> Option<u32> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<u32>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(gdb).copied()) {
        return v;
    }
    let version = (|| {
        let out = crate::host::command(gdb, ["--batch", "-ex", "python import gdb; print(gdb.VERSION)"], None, &[]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        text.trim().split('.').next()?.parse().ok()
    })();
    if let Ok(mut c) = cache.lock() {
        c.insert(gdb.to_string(), version);
    }
    version
}

/// A gdb that speaks DAP and knows the board's processor: the one asked for, gdb-multiarch, the
/// ARM toolchain's, or the system's (Fedora's knows them all).
fn embedded_gdb(asked: Option<&str>) -> Result<String, String> {
    let candidates: Vec<&str> = match asked {
        Some(a) => vec![a],
        None => vec!["gdb-multiarch", "arm-none-eabi-gdb", "gdb"],
    };
    candidates
        .into_iter()
        .find(|c| crate::lsp::find_in_path(c) && gdb_major_of(c).is_some_and(|m| m >= 14))
        .map(str::to_string)
        .ok_or_else(|| match asked {
            Some(a) => format!("{a}: gdb 14 or newer, built with Python, was not found"),
            None => "gdb 14 or newer, built with Python, was not found".into(),
        })
}

/// GDB speaks DAP from version 14 on, and only when built with Python.
fn gdb_with_dap() -> bool {
    gdb_major().is_some_and(|major| major >= 14)
}

/// Up to 15, gdb runs the program the moment `launch` arrives and ignores `configurationDone`, so
/// a `launch` sent early races the breakpoints, and sometimes wins. From 16 on it waits, and it
/// refuses a `configurationDone` with no `launch` before it, so there's no one order for both.
fn gdb_runs_on_launch() -> bool {
    gdb_major().is_some_and(|major| major <= 15)
}

/// Those same versions hand the path to gdb's `file` command as it is.
fn escape_for_file_command(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_whitespace() || matches!(c, '\\' | '\'' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn python_has_debugpy(python: &str) -> bool {
    crate::host::command(python, ["-c", "import debugpy"], None, &[]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// The debugger behind a type of configuration, or how to get one.
pub fn resolve_adapter(kind: &str) -> Result<Adapter, String> {
    let kind = normalize_kind(kind);
    let adapter = |command: &str, args: &[&str]| Adapter { kind: kind.to_string(), command: command.to_string(), args: args.iter().map(|s| s.to_string()).collect() };
    match kind {
        "gdb" if gdb_with_dap() => Ok(adapter("gdb", &["-q", "-i=dap"])),
        "gdb" => Err("gdb 14 or newer, built with Python, was not found".into()),
        "lldb" => ["lldb-dap", "lldb-vscode"]
            .iter()
            .find(|c| crate::lsp::find_in_path(c))
            .map(|c| adapter(c, &[]))
            .ok_or_else(|| "lldb-dap was not found".to_string()),
        "python" => ["python3", "python"]
            .iter()
            .find(|p| python_has_debugpy(p))
            .map(|p| adapter(p, &["-m", "debugpy.adapter"]))
            .ok_or_else(|| "debugpy was not found: pip install debugpy".to_string()),
        "go" if crate::lsp::find_in_path("dlv") => Ok(adapter("dlv", &["dap"])),
        // Without --port it talks over its standard input and output.
        "probe-rs" if crate::lsp::find_in_path("probe-rs") => Ok(adapter("probe-rs", &["dap-server", "--vscode"])),
        "probe-rs" => Err(format!("probe-rs was not found: {}", crate::embedded::tool_hint("probe-rs").command.unwrap_or_default())),
        "embedded" => embedded_gdb(None).map(|g| Adapter { kind: "gdb".into(), command: g, args: vec!["-q".into(), "-i=dap".into()] }),
        "go" => Err("dlv was not found: go install github.com/go-delve/delve/cmd/dlv@latest".into()),
        other => Err(format!("unknown debugger type: {other}")),
    }
}

/// The adapter for a configuration: like `resolve_adapter`, but a board's may name its gdb.
fn adapter_for(c: &LaunchConfig) -> Result<Adapter, String> {
    if normalize_kind(&c.kind) == "embedded" {
        let asked = c.extra.get("gdbPath").or_else(|| c.extra.get("gdb")).and_then(Value::as_str);
        return embedded_gdb(asked).map(|g| Adapter { kind: "gdb".into(), command: g, args: vec!["-q".into(), "-i=dap".into()] });
    }
    resolve_adapter(&c.kind)
}

fn native_kind() -> Result<&'static str, String> {
    if gdb_with_dap() {
        Ok("gdb")
    } else if resolve_adapter("lldb").is_ok() {
        Ok("lldb")
    } else {
        Err("no debugger found: install gdb 14 or newer, or lldb-dap".into())
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn substitute(s: &str, root: &Path, file: Option<&Path>) -> String {
    let mut out = s.replace("${workspaceFolder}", &root.to_string_lossy());
    if let Some(f) = file {
        out = out
            .replace("${file}", &f.to_string_lossy())
            .replace("${fileDirname}", &f.parent().unwrap_or(root).to_string_lossy())
            .replace("${fileBasenameNoExtension}", &f.file_stem().unwrap_or_default().to_string_lossy());
    }
    out
}

/// Where the program comes from, when only the build can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgramFrom {
    Known,
    Cargo { near: Option<PathBuf> },
    NewestExecutable,
    /// The firmware a microcontroller project built.
    Firmware(Box<crate::embedded::Project>),
}

/// `${workspaceFolder}` and friends, in every string of a value.
fn substitute_value(v: &mut Value, root: &Path, file: Option<&Path>) {
    match v {
        Value::String(s) => *s = substitute(s, root, file),
        Value::Array(a) => a.iter_mut().for_each(|x| substitute_value(x, root, file)),
        Value::Object(o) => o.values_mut().for_each(|x| substitute_value(x, root, file)),
        _ => {}
    }
}

/// What pressing F5 means here.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub root: PathBuf,
    pub config: LaunchConfig,
    pub program_from: ProgramFrom,
}

fn resolved(config: &LaunchConfig, root: &Path, file: Option<&Path>) -> Result<Plan, String> {
    let sub = |s: &str| substitute(s, root, file);
    let mut c = config.clone();
    if c.kind.is_empty() {
        return Err(format!("{}: \"type\" is missing", c.name));
    }
    c.kind = normalize_kind(&c.kind).to_string();
    if matches!(c.kind.as_str(), "embedded" | "probe-rs") {
        for v in c.extra.values_mut() {
            substitute_value(v, root, file);
        }
        // Cortex-Debug calls it `executable`; probe-rs has it in its first core.
        if c.program.is_empty() {
            let from = c.extra.get("executable").or_else(|| c.extra.get("coreConfigs").and_then(|cc| cc.get(0)).and_then(|cc| cc.get("programBinary")));
            c.program = from.and_then(Value::as_str).unwrap_or("").to_string();
        }
    }
    if c.program.is_empty() {
        return Err(format!("{}: \"program\" is missing", c.name));
    }
    c.kind = normalize_kind(&c.kind).to_string();
    c.program = sub(&c.program);
    if Path::new(&c.program).is_relative() {
        c.program = root.join(&c.program).to_string_lossy().into_owned();
    }
    c.args = c.args.iter().map(|a| sub(a)).collect();
    c.cwd = if c.cwd.is_empty() { root.to_string_lossy().into_owned() } else { sub(&c.cwd) };
    if Path::new(&c.cwd).is_relative() {
        c.cwd = root.join(&c.cwd).to_string_lossy().into_owned();
    }
    c.env = c.env.iter().map(|(k, v)| (k.clone(), sub(v))).collect();
    c.build = c.build.as_deref().map(sub);
    Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::Known })
}

/// The project's configuration if it has one and may use it, otherwise what its files and
/// language suggest.
/// A firmware project (`embedded`) is debugged on its board, through a probe.
pub fn plan(root: &Path, file: Option<&Path>, language: &str, selected: Option<&str>, use_configs: bool, embedded: Option<&crate::embedded::Project>, pt: bool) -> Result<Plan, String> {
    let configs = if use_configs { load_configs(root)? } else { Vec::new() };
    if !configs.is_empty() {
        let c = selected.and_then(|n| configs.iter().find(|c| c.name == n)).unwrap_or(&configs[0]);
        return resolved(c, root, file);
    }
    if let Some(p) = embedded
        && !matches!(language, "Python" | "Go")
    {
        return embedded_plan(root, p, pt);
    }
    auto_plan(root, file, language)
}

/// Build, then flash and debug the board: through probe-rs, which does both, or a GDB server.
fn embedded_plan(root: &Path, p: &crate::embedded::Project, pt: bool) -> Result<Plan, String> {
    use crate::embedded::Need;
    let find = |t: &str| crate::host::find(t);
    let need = |n: Need| match n {
        Need::Tool { tool, hint } => match hint.command {
            Some(c) => format!("{tool} was not found: {c}"),
            None => format!("{tool} was not found"),
        },
        Need::Setting(s) => s,
    };
    let setup = p.debug_setup(&find, pt).map_err(need)?;
    let build = p.build(&find, pt).map_err(need)?;
    let mut c = LaunchConfig { name: p.name(), cwd: root.to_string_lossy().into_owned(), build: Some(build), ..LaunchConfig::default() };
    if setup.server == "probe-rs" {
        c.kind = "probe-rs".into();
        c.extra.insert("chip".into(), json!(setup.chip));
    } else {
        adapter_for(&LaunchConfig { kind: "embedded".into(), ..LaunchConfig::default() })?;
        c.kind = "embedded".into();
        c.extra.insert("servertype".into(), json!(setup.server));
        c.extra.insert("configFiles".into(), json!(setup.openocd));
        if let Some(chip) = setup.chip {
            c.extra.insert("device".into(), json!(chip));
        }
        c.extra.insert("runToEntryPoint".into(), json!("main"));
    }
    Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::Firmware(Box::new(p.clone())) })
}

fn auto_plan(root: &Path, file: Option<&Path>, language: &str) -> Result<Plan, String> {
    let base = |kind: &str, name: String, program: String, build: Option<String>| LaunchConfig {
        name,
        kind: kind.to_string(),
        program,
        cwd: root.to_string_lossy().into_owned(),
        build,
        ..LaunchConfig::default()
    };
    let name_of = |f: &Path| f.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let has = |f: &str| root.join(f).exists();
    if language == "Rust" || (has("Cargo.toml") && !matches!(language, "Python" | "Go" | "C" | "C++")) {
        if !has("Cargo.toml") {
            return Err("this file isn't in a Cargo project: add .tinta/launch.json".into());
        }
        let c = base(native_kind()?, "cargo".into(), String::new(), Some("cargo build".into()));
        return Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::Cargo { near: file.map(Path::to_path_buf) } });
    }
    match language {
        "Python" => {
            resolve_adapter("python")?;
            let f = file.ok_or("save the file first")?;
            let c = base("python", name_of(f), f.to_string_lossy().into_owned(), None);
            Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::Known })
        }
        "Go" => {
            resolve_adapter("go")?;
            let dir = file.and_then(Path::parent).unwrap_or(root);
            let mut c = base("go", "go".into(), dir.to_string_lossy().into_owned(), None);
            c.extra.insert("mode".into(), json!("debug"));
            Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::Known })
        }
        "C" | "C++" => {
            let kind = native_kind()?;
            if has("Makefile") || has("makefile") || has("GNUmakefile") {
                let c = base(kind, "make".into(), String::new(), Some("make".into()));
                return Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::NewestExecutable });
            }
            if has("CMakeLists.txt") {
                let c = base(kind, "cmake".into(), String::new(), Some("cmake -S . -B build -DCMAKE_BUILD_TYPE=Debug && cmake --build build".into()));
                return Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::NewestExecutable });
            }
            // No build system: a single file, compiled on the side.
            let f = file.ok_or("save the file first")?;
            let dir = crate::host::temp_dir().join(format!("tinta-debug-{}", std::process::id()));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let exe = dir.join(f.file_stem().unwrap_or_default());
            let cc = if language == "C" { "cc -g -O0" } else { "c++ -g -O0" };
            let build = format!("{cc} {} -o {} -lm", shell_quote(&f.to_string_lossy()), shell_quote(&exe.to_string_lossy()));
            let c = base(kind, name_of(f), exe.to_string_lossy().into_owned(), Some(build));
            Ok(Plan { root: root.to_path_buf(), config: c, program_from: ProgramFrom::Known })
        }
        _ => Err(format!("no debugger is known for {language}: add .tinta/launch.json")),
    }
}

fn is_elf_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(p) else { return false };
    if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return false;
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && &magic == b"\x7fELF"
}

/// What a build most likely just made.
pub fn newest_executable(root: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for dir in [root.to_path_buf(), root.join("build"), root.join("bin"), root.join("out")] {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let p = e.path();
            if !is_elf_executable(&p) {
                continue;
            }
            let Ok(time) = e.metadata().and_then(|m| m.modified()) else { continue };
            if best.as_ref().is_none_or(|(t, _)| time > *t) {
                best = Some((time, p));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn cargo_binary(root: &Path, near: Option<&Path>) -> Result<PathBuf, String> {
    let out = crate::host::command("cargo", ["metadata", "--no-deps", "--format-version", "1"], Some(root), &[])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let target_dir = PathBuf::from(v["target_directory"].as_str().unwrap_or("target"));
    let mut bins: Vec<(PathBuf, String)> = Vec::new();
    for pkg in v["packages"].as_array().into_iter().flatten() {
        let dir = Path::new(pkg["manifest_path"].as_str().unwrap_or("")).parent().unwrap_or(root).to_path_buf();
        for t in pkg["targets"].as_array().into_iter().flatten() {
            if t["kind"].as_array().is_some_and(|k| k.iter().any(|x| x == "bin")) {
                bins.push((dir.clone(), t["name"].as_str().unwrap_or("").to_string()));
            }
        }
    }
    // The package the open file belongs to, if that says anything.
    let chosen = near
        .and_then(|f| bins.iter().filter(|(d, _)| f.starts_with(d)).max_by_key(|(d, _)| d.as_os_str().len()))
        .or(bins.first());
    let (_, name) = chosen.ok_or("this Cargo project has no binary: add .tinta/launch.json")?;
    Ok(target_dir.join("debug").join(name))
}

/// Fills in the program once the build is done.
pub fn finish_plan(mut plan: Plan) -> Result<Plan, String> {
    match &plan.program_from {
        ProgramFrom::Known => {}
        ProgramFrom::Cargo { near } => {
            let p = cargo_binary(&plan.root, near.as_deref())?;
            plan.config.name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            plan.config.program = p.to_string_lossy().into_owned();
        }
        ProgramFrom::NewestExecutable => {
            let p = newest_executable(&plan.root).ok_or("no program was found after the build: add .tinta/launch.json")?;
            plan.config.name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
            plan.config.program = p.to_string_lossy().into_owned();
        }
        ProgramFrom::Firmware(project) => {
            let p = project.firmware().ok_or("no firmware (.elf) was found after the build: add .tinta/launch.json")?;
            plan.config.program = p.to_string_lossy().into_owned();
        }
    }
    Ok(plan)
}

fn launch_arguments(c: &LaunchConfig) -> Value {
    if c.kind == "probe-rs" {
        return probe_rs_arguments(c);
    }
    let mut a = Map::new();
    a.insert("name".into(), json!(c.name));
    a.insert("type".into(), json!(c.kind));
    a.insert("request".into(), json!("launch"));
    a.insert("program".into(), json!(c.program));
    a.insert("args".into(), json!(c.args));
    a.insert("cwd".into(), json!(c.cwd));
    a.insert("env".into(), json!(c.env));
    match c.kind.as_str() {
        // Its stopOnEntry stops in the dynamic loader, before any code of yours.
        "gdb" => {
            a.insert("stopAtBeginningOfMainSubprogram".into(), json!(c.stop_on_entry));
        }
        "python" => {
            a.insert("console".into(), json!("internalConsole"));
            a.insert("justMyCode".into(), json!(true));
            a.insert("stopOnEntry".into(), json!(c.stop_on_entry));
        }
        _ => {
            a.insert("stopOnEntry".into(), json!(c.stop_on_entry));
        }
    }
    for (k, v) in &c.extra {
        a.insert(k.clone(), v.clone());
    }
    Value::Object(a)
}

/// probe-rs flashes the firmware and runs it, with its log (RTT) coming back as output.
fn probe_rs_arguments(c: &LaunchConfig) -> Value {
    let mut a = json!({
        "name": c.name,
        "type": "probe-rs-debug",
        "request": "launch",
        "cwd": c.cwd,
        "connectUnderReset": false,
        "consoleLogLevel": "Console",
        "flashingConfig": { "flashingEnabled": true, "haltAfterReset": c.stop_on_entry },
        "coreConfigs": [{ "coreIndex": 0, "programBinary": c.program, "rttEnabled": true }],
    });
    for (k, v) in &c.extra {
        a[k] = v.clone();
    }
    a
}

// Debugging a board through a GDB server ----------------------------------------------------------

/// A program that speaks GDB's remote protocol for the probe: OpenOCD, st-util, J-Link, pyOCD,
/// or one of the user's (QEMU, `west debugserver`).
#[derive(Debug, Clone)]
struct ServerLaunch {
    argv: Vec<String>,
    cwd: PathBuf,
    /// The port to wait for before gdb connects.
    port: Option<u16>,
}

/// What gdb does once it's connected: reset, flash, run to main.
#[derive(Debug, Clone, Default)]
struct RemoteSetup {
    commands: VecDeque<String>,
    /// The first stop (on connecting) has been seen, and the commands started.
    started: bool,
    /// Stay at the reset, instead of running.
    stop_on_entry: bool,
}

/// A port nothing is using right now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()).map(|a| a.port()).unwrap_or(3333)
}

/// Whether something on this computer listens on the port. Read from the kernel, not by
/// connecting: some servers (st-util, J-Link) quit when their first client leaves.
fn listening(port: u16) -> bool {
    let hex = format!(":{port:04X}");
    ["/proc/net/tcp", "/proc/net/tcp6"].iter().any(|f| {
        std::fs::read_to_string(f).is_ok_and(|s| {
            s.lines().skip(1).any(|l| {
                let mut w = l.split_whitespace();
                let local = w.nth(1).unwrap_or("");
                let state = w.nth(1).unwrap_or("");
                local.ends_with(&hex) && state == "0A"
            })
        })
    })
}

fn extra_str(c: &LaunchConfig, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| c.extra.get(*k).and_then(Value::as_str)).filter(|s| !s.is_empty()).map(str::to_string)
}

fn extra_list(c: &LaunchConfig, key: &str) -> Vec<String> {
    match c.extra.get(key) {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// The GDB server, gdb's `attach` arguments, and what to do once attached, for a configuration
/// of type `embedded`. The keys are Cortex-Debug's, so its launch.json files work.
fn remote_session(c: &LaunchConfig, gdb: &str) -> Result<(Option<ServerLaunch>, Value, RemoteSetup), String> {
    let server = extra_str(c, &["servertype", "server"]).unwrap_or_else(|| "external".into()).to_ascii_lowercase();
    let chip = extra_str(c, &["device", "chip"]);
    let need_chip = || chip.clone().ok_or_else(|| format!("{}: \"device\" is missing: the chip, like STM32F103C8", c.name));
    let port = free_port();
    let p = port.to_string();
    let local = format!("localhost:{port}");
    let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let (mut argv, target, wait): (Option<Vec<String>>, String, Option<u16>) = match server.as_str() {
        "openocd" => {
            let files = extra_list(c, "configFiles");
            if files.is_empty() {
                return Err(format!("{}: \"configFiles\" is missing: OpenOCD's files, like interface/stlink.cfg, target/stm32f1x.cfg", c.name));
            }
            let mut a = words(&["openocd", "-c", &format!("gdb_port {p}"), "-c", "tcl_port disabled", "-c", "telnet_port disabled"]);
            for f in files {
                a.push("-f".into());
                a.push(f);
            }
            (Some(a), local, Some(port))
        }
        "stlink" | "stutil" | "st-util" => (Some(words(&["st-util", "-p", &p, "--multi"])), local, Some(port)),
        "jlink" => {
            let iface = extra_str(c, &["interface"]).unwrap_or_else(|| "SWD".into()).to_ascii_uppercase();
            (Some(words(&["JLinkGDBServerCLExe", "-if", &iface, "-device", &need_chip()?, "-speed", "4000", "-port", &p])), local, Some(port))
        }
        "pyocd" => (Some(words(&["pyocd", "gdbserver", "-t", &need_chip()?.to_ascii_lowercase(), "-p", &p, "--persist"])), local, Some(port)),
        "external" | "qemu" => {
            let target = extra_str(c, &["gdbTarget"]).ok_or_else(|| format!("{}: \"gdbTarget\" is missing: where the GDB server listens, like localhost:3333", c.name))?;
            let command = extra_str(c, &["serverCommand"]);
            // Waiting only makes sense for a server started here, on this computer.
            let wait = command.as_ref().and_then(|_| {
                let (host, port) = target.rsplit_once(':')?;
                matches!(host, "" | "localhost" | "127.0.0.1").then(|| port.parse().ok()).flatten()
            });
            (command.map(|cmd| words(&["sh", "-c", &cmd])), target, wait)
        }
        other => return Err(format!("{}: unknown GDB server \"{other}\": openocd, stlink, jlink, pyocd or external", c.name)),
    };
    if let (Some(a), Some(path)) = (argv.as_mut(), extra_str(c, &["serverpath", "serverPath"])) {
        a[0] = path;
    }
    if let Some(a) = &argv
        && !crate::lsp::find_in_path(&a[0])
    {
        let hint = crate::embedded::tool_hint(&a[0]).command.map(|h| format!(": {h}")).unwrap_or_default();
        return Err(format!("{} was not found{hint}", a[0]));
    }
    let load = c.extra.get("load").and_then(Value::as_bool).unwrap_or(true);
    let attach_only = c.extra.get("request").and_then(Value::as_str) == Some("attach");
    let mut commands: VecDeque<String> = VecDeque::new();
    // Before 15, gdb's attach takes no program: it's loaded here instead.
    if gdb_major_of(gdb).is_some_and(|m| m < 15) {
        commands.push_back(format!("file {}", escape_for_file_command(&c.program)));
    }
    commands.extend(extra_list(c, "preLaunchCommands"));
    let over = extra_list(c, "overrideLaunchCommands");
    if !over.is_empty() {
        commands.extend(over);
    } else if !attach_only {
        let flush = "maintenance flush register-cache".to_string();
        let ld = load.then(|| "load".to_string());
        let steps: Vec<Option<String>> = match server.as_str() {
            "openocd" | "pyocd" => vec![Some("monitor reset halt".into()), ld, Some("monitor reset halt".into()), Some(flush)],
            "jlink" => vec![Some("monitor halt".into()), Some("monitor reset".into()), ld, Some("monitor reset".into()), Some(flush)],
            "stlink" | "stutil" | "st-util" => vec![ld, Some("monitor reset".into()), Some(flush)],
            _ => vec![ld],
        };
        commands.extend(steps.into_iter().flatten());
    }
    commands.extend(extra_list(c, "postLaunchCommands"));
    let stop_on_entry = c.stop_on_entry || attach_only;
    let run_to = match c.extra.get("runToEntryPoint").or_else(|| c.extra.get("runToMain")) {
        Some(Value::String(f)) if !f.is_empty() => Some(f.clone()),
        Some(Value::Bool(true)) => Some("main".into()),
        _ => None,
    };
    if let Some(f) = run_to.filter(|_| !stop_on_entry) {
        commands.push_back(format!("tbreak {f}"));
    }
    let attach = json!({ "program": c.program, "target": target });
    let server = argv.map(|argv| ServerLaunch { argv, cwd: PathBuf::from(&c.cwd), port: wait });
    Ok((server, attach, RemoteSetup { commands, started: false, stop_on_entry }))
}

/// Starts the GDB server, passes on what it says, and waits until it listens. An error if it
/// ends first, or doesn't get there in time.
fn start_server(s: &ServerLaunch, sink: &Sink, slot: &Arc<Mutex<Option<Child>>>) -> Result<(), String> {
    let name = s.argv[0].clone();
    let mut child = crate::host::command(&s.argv[0], &s.argv[1..], Some(&s.cwd).filter(|d| d.is_dir()).map(PathBuf::as_path), &[])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{name}: {e}"))?;
    for out in [child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>), child.stderr.take().map(|e| Box::new(e) as Box<dyn Read + Send>)].into_iter().flatten() {
        let sink = sink.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                sink(Incoming::Server(line));
            }
        });
    }
    if let Ok(mut s) = slot.lock() {
        *s = Some(child);
    }
    let Some(port) = s.port else { return Ok(()) };
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        if listening(port) {
            return Ok(());
        }
        let ended = slot.lock().ok().and_then(|mut c| c.as_mut().and_then(|c| c.try_wait().ok().flatten()));
        if let Some(status) = ended {
            return Err(format!("{name} ended before the debugger could connect ({status})"));
        }
        if Instant::now() > end {
            return Err(format!("{name} isn't listening on port {port}"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// The client ---------------------------------------------------------------------------------

struct Client {
    writer: Sender<Vec<u8>>,
    next_seq: AtomicI64,
    child: Arc<Mutex<Option<Child>>>,
    /// The GDB server started for the session, if any.
    server: Arc<Mutex<Option<Child>>>,
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

fn spawn_adapter(adapter: &Adapter, cwd: &Path, tty: Option<&ProgramTty>) -> std::io::Result<Child> {
    let mut command = match tty {
        // From a Flatpak, gdb gets the program's terminal as its fd 3.
        Some(t) if crate::host::sandboxed() && adapter.kind == "gdb" => {
            use std::os::fd::AsRawFd;
            crate::host::command_with_fd(&adapter.command, &adapter.args, Some(cwd), &[], t.slave.as_raw_fd())
        }
        _ => crate::host::command(&adapter.command, &adapter.args, Some(cwd), &[]),
    };
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
}

/// The adapter's pipes to the sink, and what's queued for it to the adapter.
fn wire(mut child: Child, rx: Receiver<Vec<u8>>, sink: Sink, slot: Arc<Mutex<Option<Child>>>) {
    let stdin = child.stdin.take().expect("piped");
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    if let Ok(mut s) = slot.lock() {
        *s = Some(child);
    }
    std::thread::spawn(move || {
        let mut stdin = stdin;
        while let Ok(buf) = rx.recv() {
            if stdin.write_all(&buf).and_then(|()| stdin.flush()).is_err() {
                break;
            }
        }
    });
    {
        let sink = sink.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                sink(Incoming::Stderr(line));
            }
        });
    }
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        while let Some(msg) = read_message(&mut r) {
            sink(Incoming::Message(msg));
        }
        if let Ok(mut c) = slot.lock()
            && let Some(c) = c.as_mut()
        {
            let _ = c.wait();
        }
        sink(Incoming::Exited);
    });
}

impl Client {
    /// Starts the adapter. With a GDB server, the server first, and the adapter once the server
    /// listens, all on a thread of its own; what's sent meanwhile waits for the adapter.
    fn start(adapter: &Adapter, cwd: &Path, sink: Sink, tty: Option<&ProgramTty>, server: Option<ServerLaunch>) -> std::io::Result<Self> {
        let (writer, rx) = channel::<Vec<u8>>();
        let child = Arc::new(Mutex::new(None));
        let server_slot = Arc::new(Mutex::new(None));
        match server {
            None => wire(spawn_adapter(adapter, cwd, tty)?, rx, sink, child.clone()),
            Some(s) => {
                let (adapter, cwd, child, server_slot) = (adapter.clone(), cwd.to_path_buf(), child.clone(), server_slot.clone());
                std::thread::spawn(move || {
                    if let Err(e) = start_server(&s, &sink, &server_slot) {
                        sink(Incoming::ServerFailed(e));
                        return;
                    }
                    match spawn_adapter(&adapter, &cwd, None) {
                        Ok(c) => wire(c, rx, sink, child),
                        Err(e) => sink(Incoming::ServerFailed(format!("{}: {e}", adapter.command))),
                    }
                });
            }
        }
        Ok(Self { writer, next_seq: AtomicI64::new(1), child, server: server_slot })
    }

    fn send(&self, v: &Value) {
        let body = v.to_string();
        let mut buf = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        buf.extend_from_slice(body.as_bytes());
        let _ = self.writer.send(buf);
    }

    fn request(&self, command: &str, arguments: Value) -> i64 {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.send(&json!({ "seq": seq, "type": "request", "command": command, "arguments": arguments }));
        seq
    }

    fn respond(&self, request_seq: i64, command: &str, body: Value) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.send(&json!({
            "seq": seq, "type": "response", "request_seq": request_seq, "command": command,
            "success": true, "body": body,
        }));
    }

    fn refuse(&self, request_seq: i64, command: &str) {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        self.send(&json!({
            "seq": seq, "type": "response", "request_seq": request_seq, "command": command,
            "success": false, "message": "not supported",
        }));
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // It got a disconnect, and a moment to act on it. Then nothing is left running: not the
        // adapter, and not the GDB server it was talking to.
        let pid = self.child.lock().ok().and_then(|c| c.as_ref().map(Child::id));
        let server = self.server.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(600));
            if let Some(pid) = pid {
                let _ = Command::new("kill").arg(pid.to_string()).stderr(Stdio::null()).status();
            }
            if let Ok(mut s) = server.lock()
                && let Some(c) = s.as_mut()
            {
                let _ = c.kill();
                let _ = c.wait();
            }
        });
    }
}

impl LaunchConfig {
    /// The program gets a terminal: what it prints goes there, and it reads what's typed in it.
    /// Gdb takes one before the program starts; debugpy asks the editor to start the program in
    /// one. With the other debuggers the program stays in the console.
    pub fn wants_terminal(&self) -> bool {
        self.console != "internalConsole" && matches!(normalize_kind(&self.kind), "gdb" | "python")
    }
}

// The program's terminal ---------------------------------------------------------------------

/// A pseudo-terminal for the program being debugged: the debugger connects the program to
/// `path`, and the editor shows `master` in a terminal.
pub struct ProgramTty {
    pub master: std::os::fd::OwnedFd,
    pub path: PathBuf,
    /// Held open so the terminal doesn't see the end of its input before the program starts,
    /// or between one run and the next. In a Flatpak it's also what gdb gets, as its fd 3.
    slave: std::os::fd::OwnedFd,
}

pub fn open_tty() -> std::io::Result<ProgramTty> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let err = std::io::Error::last_os_error;
    // SAFETY: plain calls on a descriptor this function owns, with a buffer of the size given.
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(err());
        }
        let master = OwnedFd::from_raw_fd(fd);
        if libc::grantpt(fd) != 0 || libc::unlockpt(fd) != 0 {
            return Err(err());
        }
        let mut name = [0 as libc::c_char; 128];
        if libc::ptsname_r(fd, name.as_mut_ptr(), name.len()) != 0 {
            return Err(err());
        }
        let path = PathBuf::from(std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned());
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).map_err(|_| std::io::Error::other("tty name"))?;
        let slave = libc::open(c.as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        if slave < 0 {
            return Err(err());
        }
        Ok(ProgramTty { master, path, slave: OwnedFd::from_raw_fd(slave) })
    }
}

// The session --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunState {
    Starting,
    Running,
    Stopped { reason: String, description: Option<String> },
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// Zero-based.
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub value: String,
    pub kind: Option<String>,
    pub reference: i64,
}

impl Variable {
    fn from_json(v: &Value) -> Self {
        Self {
            name: v["name"].as_str().unwrap_or("").to_string(),
            value: v["value"].as_str().unwrap_or("").to_string(),
            kind: v["type"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
            reference: v["variablesReference"].as_i64().unwrap_or(0),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub name: String,
    pub reference: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub expression: String,
    pub result: Result<Variable, String>,
}

/// One line of the variables tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VarRow {
    pub depth: usize,
    /// Stays the same from one stop to the next, unlike the debugger's references.
    pub path: String,
    pub name: String,
    pub value: String,
    pub reference: i64,
    pub expandable: bool,
    pub expanded: bool,
    pub changed: bool,
    pub is_scope: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EvalFor {
    Console(String),
    Watch(String),
    Hover { expression: String, pos: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Initialize,
    Launch,
    SetBreakpoints { path: PathBuf, lines: Vec<usize> },
    Threads,
    StackTrace { thread: i64 },
    Scopes,
    Variables { path: String },
    Evaluate(EvalFor),
    /// A new value for a variable or a watched expression.
    SetValue,
    /// One of the gdb commands that get a board ready.
    Setup(String),
    Quiet,
}

/// What the application has to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Output { category: String, text: String },
    /// Execution is here: the file should be on screen.
    Show { path: PathBuf, line: usize, column: usize },
    Stopped,
    Continued,
    Ended { code: Option<i64> },
    /// The session failed to start, or a request it couldn't do without did.
    Failed(String),
    Hover { expression: String, value: String, pos: usize },
}

pub struct Debugger {
    client: Client,
    pub kind: String,
    pub name: String,
    launch: Value,
    pub state: RunState,
    pub capabilities: Value,
    pending: HashMap<i64, Pending>,
    pub threads: Vec<Thread>,
    pub thread: Option<i64>,
    pub frames: Vec<Frame>,
    pub frame: usize,
    pub scopes: Vec<Scope>,
    children: HashMap<i64, Vec<Variable>>,
    expanded: HashSet<String>,
    collapsed_scopes: HashSet<String>,
    values: HashMap<String, String>,
    previous: HashMap<String, String>,
    pub watches: Vec<Watch>,
    pub exit_code: Option<i64>,
    configured: bool,
    /// When the adapter said the launch went fine, for the ones that say so regardless.
    launched_at: Option<Instant>,
    /// `launch` goes after `configurationDone` instead of before it.
    launch_last: bool,
    /// Anything that shows the program really is starting.
    program_seen: bool,
    pub start_limit: Duration,
    /// The terminal the program runs in, given to gdb before the program starts.
    tty: Option<PathBuf>,
    /// "Run to cursor" put a breakpoint in this file for a while: the file's real ones go back
    /// at the next stop.
    run_to: Option<PathBuf>,
    /// `launch`, or `attach` for a board behind a GDB server.
    request: &'static str,
    /// A board: what gdb does once connected.
    remote: Option<RemoteSetup>,
}

const MAX_ROWS: usize = 2000;
/// Generous: the program's symbols are already loaded when this starts counting.
const START_LIMIT: Duration = Duration::from_secs(15);
/// When the adapter loads symbols and runs the program all in the one request we're timing.
const START_LIMIT_LAUNCH_LAST: Duration = Duration::from_secs(60);

/// Catches what would otherwise look like a session that never does anything.
fn check_program(c: &LaunchConfig) -> Result<(), String> {
    let p = Path::new(&c.program);
    if !p.exists() {
        return Err(format!("program not found: {}", c.program));
    }
    // Firmware isn't run by this computer: no need for it to be executable.
    if matches!(c.kind.as_str(), "embedded" | "probe-rs") {
        return if crate::embedded::is_elf(p) { Ok(()) } else { Err(format!("not an ELF file: {}", c.program)) };
    }
    if matches!(c.kind.as_str(), "gdb" | "lldb") && !is_elf_executable(p) {
        return Err(format!("not an executable program: {}", c.program));
    }
    Ok(())
}

/// The debugger's `runInTerminal`: the command it gives, started in the program's terminal as
/// a session of its own, the terminal its controlling one, as a shell would. Its pid back.
fn run_in_terminal(args: &Value, tty: &Path) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    let argv: Vec<String> = args["args"].as_array().into_iter().flatten().filter_map(|a| a.as_str().map(str::to_string)).collect();
    let (program, rest) = argv.split_first().ok_or_else(|| std::io::Error::other("no command"))?;
    let open = || std::fs::OpenOptions::new().read(true).write(true).open(tty);
    // The program is the user's, so it runs on their system; in a Flatpak, flatpak-spawn
    // hands it this terminal as its input and output.
    let cwd = args["cwd"].as_str().filter(|c| !c.is_empty()).map(Path::new);
    let env: Vec<(String, String)> = args["env"].as_object().into_iter().flatten().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect();
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let mut cmd = crate::host::command(program, rest, cwd, &env);
    cmd.stdin(open()?).stdout(open()?).stderr(open()?);
    // SAFETY: only async-signal-safe calls, between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let pid = child.id();
    // Reaped when it ends, so it doesn't linger as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(pid)
}

/// `setBreakpoints` for a file: the lines asked for, and the arguments. `extra` is one more line.
fn breakpoint_args(path: &Path, list: &[Breakpoint], extra: Option<usize>) -> (Vec<usize>, Value) {
    let active: Vec<&Breakpoint> = list.iter().filter(|b| b.enabled).collect();
    let lines: Vec<usize> = active.iter().map(|b| b.line).collect();
    let mut bps: Vec<Value> = active
        .iter()
        .map(|b| {
            let mut o = json!({ "line": b.line + 1 });
            if let Some(c) = b.condition.as_deref().filter(|c| !c.trim().is_empty()) {
                o["condition"] = json!(c);
            }
            o
        })
        .collect();
    if let Some(l) = extra.filter(|l| !lines.contains(l)) {
        bps.push(json!({ "line": l + 1 }));
    }
    let args = json!({
        "source": { "path": path, "name": path.file_name().map(|n| n.to_string_lossy().into_owned()) },
        "breakpoints": bps,
    });
    (lines, args)
}

impl Debugger {
    pub fn start(plan: &Plan, watches: &[String], sink: Sink, tty: Option<&ProgramTty>) -> Result<Self, String> {
        let adapter = adapter_for(&plan.config)?;
        check_program(&plan.config)?;
        let cwd = Path::new(&plan.config.cwd);
        let embedded = plan.config.kind == "embedded";
        let (server, attach, remote) = if embedded {
            let (server, attach, setup) = remote_session(&plan.config, &adapter.command)?;
            (server, Some(attach), Some(setup))
        } else {
            (None, None, None)
        };
        let tty = tty.filter(|_| !embedded && matches!(adapter.kind.as_str(), "gdb" | "python"));
        let client = Client::start(&adapter, if cwd.is_dir() { cwd } else { &plan.root }, sink, tty, server).map_err(|e| format!("{}: {e}", adapter.command))?;
        let launch_last = !embedded && adapter.kind == "gdb" && gdb_runs_on_launch();
        let mut launch = attach.unwrap_or_else(|| launch_arguments(&plan.config));
        if launch_last {
            launch["program"] = json!(escape_for_file_command(&plan.config.program));
        }
        let mut d = Self {
            client,
            kind: adapter.kind.clone(),
            name: plan.config.name.clone(),
            launch,
            state: RunState::Starting,
            capabilities: Value::Null,
            pending: HashMap::new(),
            threads: Vec::new(),
            thread: None,
            frames: Vec::new(),
            frame: 0,
            scopes: Vec::new(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            collapsed_scopes: HashSet::new(),
            values: HashMap::new(),
            previous: HashMap::new(),
            watches: watches.iter().map(|w| Watch { expression: w.clone(), result: Err(String::new()) }).collect(),
            exit_code: None,
            configured: false,
            launched_at: None,
            launch_last,
            program_seen: false,
            start_limit: if launch_last { START_LIMIT_LAUNCH_LAST } else { START_LIMIT },
            // gdb on the other side of a Flatpak reaches the terminal through the fd it was given:
            // the program it starts inherits it, and opens it again by this name.
            tty: tty.map(|t| if crate::host::sandboxed() && adapter.kind == "gdb" { PathBuf::from("/proc/self/fd/3") } else { t.path.clone() }),
            run_to: None,
            request: if embedded { "attach" } else { "launch" },
            remote,
        };
        let mut args = json!({
            "clientID": "tinta",
            "clientName": "Tinta",
            "adapterID": adapter.kind,
            "pathFormat": "path",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "supportsVariableType": true,
        });
        // Debugpy starts the program through us, in the program's terminal.
        if d.tty.is_some() && d.kind == "python" {
            args["supportsRunInTerminalRequest"] = json!(true);
            d.launch["console"] = json!("integratedTerminal");
        }
        d.send(Pending::Initialize, "initialize", args);
        Ok(d)
    }

    fn send(&mut self, p: Pending, command: &str, args: Value) -> i64 {
        let seq = self.client.request(command, args);
        self.pending.insert(seq, p);
        seq
    }

    /// Gdb up to 17 reports a launch as fine even when it couldn't load the program, and then does
    /// nothing. It's the only adapter this looks for: it always announces the program, while others
    /// might not, and a limit on those could kill a program that's running normally.
    pub fn check_started(&mut self) -> Option<Notice> {
        if self.kind != "gdb" || self.program_seen || !self.is_live() {
            return None;
        }
        if self.launched_at?.elapsed() < self.start_limit {
            return None;
        }
        self.stop();
        Some(Notice::Failed(format!("the debugger did not start the program: {}", self.name)))
    }

    pub fn is_stopped(&self) -> bool {
        matches!(self.state, RunState::Stopped { .. })
    }

    pub fn is_live(&self) -> bool {
        self.state != RunState::Ended
    }

    /// Execution's place in the file on screen. The bool: it's the innermost frame.
    pub fn exec_location(&self) -> Option<(&Path, usize, bool)> {
        if !self.is_stopped() {
            return None;
        }
        let f = self.frames.get(self.frame)?;
        Some((f.path.as_deref()?, f.line, self.frame == 0))
    }

    pub fn current_frame_id(&self) -> Option<i64> {
        self.frames.get(self.frame).map(|f| f.id)
    }

    // Requests from the user --------------------------------------------------------------------

    pub fn send_breakpoints(&mut self, path: &Path, list: &[Breakpoint]) {
        let (lines, args) = breakpoint_args(path, list, None);
        self.send(Pending::SetBreakpoints { path: path.to_path_buf(), lines }, "setBreakpoints", args);
    }

    /// Runs on until `line`, through a breakpoint there that lasts until the next stop. `list`
    /// is the file's own breakpoints, which keep working on the way.
    pub fn run_to(&mut self, path: &Path, line: usize, list: &[Breakpoint]) {
        if !self.is_stopped() {
            return;
        }
        let (_, args) = breakpoint_args(path, list, Some(line));
        self.send(Pending::Quiet, "setBreakpoints", args);
        self.run_to = Some(path.to_path_buf());
        self.continue_();
    }

    /// Whether a line of the variables tree can take a new value: a variable or a watched
    /// expression, while paused, with a debugger that can do it.
    pub fn can_set(&self, row: &VarRow) -> bool {
        if !self.is_stopped() || row.is_scope {
            return false;
        }
        let watch = row.depth == 0 && row.path.starts_with("\u{1}watch/");
        let cap = if watch { "supportsSetExpression" } else { "supportsSetVariable" };
        self.capabilities[cap].as_bool().unwrap_or(false)
    }

    /// A new value for a line of the tree. The tree is read again once the debugger has it.
    pub fn set_value(&mut self, row: &VarRow, value: &str) {
        if !self.can_set(row) {
            return;
        }
        if row.depth == 0 && row.path.starts_with("\u{1}watch/") {
            let mut args = json!({ "expression": row.name, "value": value });
            if let Some(id) = self.current_frame_id() {
                args["frameId"] = json!(id);
            }
            self.send(Pending::SetValue, "setExpression", args);
            return;
        }
        let Some(parent) = row.path.rsplit_once('/').and_then(|(p, _)| self.reference_for(p)) else { return };
        self.send(Pending::SetValue, "setVariable", json!({ "variablesReference": parent, "name": row.name, "value": value }));
    }

    /// The file's real breakpoints again, after "run to cursor" borrowed it.
    fn end_run_to(&mut self, bps: &Breakpoints) {
        if let Some(path) = self.run_to.take() {
            let list = bps.get(&path).cloned().unwrap_or_default();
            self.send_breakpoints(&path, &list);
        }
    }

    fn resume(&mut self, command: &str) {
        let Some(thread) = self.thread else { return };
        if !self.is_stopped() {
            return;
        }
        self.send(Pending::Quiet, command, json!({ "threadId": thread }));
        self.state = RunState::Running;
    }

    pub fn continue_(&mut self) {
        self.resume("continue");
    }
    pub fn step_over(&mut self) {
        self.resume("next");
    }
    pub fn step_in(&mut self) {
        self.resume("stepIn");
    }
    pub fn step_out(&mut self) {
        self.resume("stepOut");
    }

    pub fn pause(&mut self) {
        if self.state == RunState::Running {
            let thread = self.thread.or_else(|| self.threads.first().map(|t| t.id)).unwrap_or(1);
            self.send(Pending::Quiet, "pause", json!({ "threadId": thread }));
        }
    }

    /// Right now, for when the editor itself is closing.
    pub fn kill(&mut self) {
        for slot in [&self.client.child, &self.client.server] {
            if let Ok(mut c) = slot.lock()
                && let Some(c) = c.as_mut()
            {
                let _ = c.kill();
            }
        }
    }

    /// Ends the session and the program with it. A board is let go instead, and goes on running.
    pub fn stop(&mut self) {
        let terminate = self.remote.is_none() && self.kind != "probe-rs";
        self.send(Pending::Quiet, "disconnect", json!({ "restart": false, "terminateDebuggee": terminate }));
        self.state = RunState::Ended;
    }

    pub fn select_frame(&mut self, i: usize) -> Vec<Notice> {
        if i >= self.frames.len() || !self.is_stopped() {
            return Vec::new();
        }
        self.frame = i;
        self.refresh_frame();
        self.show_frame()
    }

    pub fn select_thread(&mut self, id: i64) {
        if self.is_stopped() {
            self.thread = Some(id);
            self.send(Pending::StackTrace { thread: id }, "stackTrace", json!({ "threadId": id, "startFrame": 0, "levels": 64 }));
        }
    }

    fn evaluate(&mut self, expression: &str, context: &str, why: EvalFor) {
        let mut args = json!({ "expression": expression, "context": context });
        if let Some(id) = self.current_frame_id() {
            args["frameId"] = json!(id);
        }
        self.send(Pending::Evaluate(why), "evaluate", args);
    }

    /// GDB and LLDB take `repl` as one of their own commands, so `n*2` would step. Python's and
    /// Delve's `repl` is the language. `-exec` is the way to reach the debugger's own commands.
    pub fn console_input(&mut self, input: &str) {
        let (expression, context) = match input.strip_prefix("-exec ") {
            Some(cmd) => (cmd, "repl"),
            None if matches!(self.kind.as_str(), "gdb" | "lldb") => (input, "watch"),
            None => (input, "repl"),
        };
        let expression = expression.to_string();
        self.evaluate(&expression, context, EvalFor::Console(input.to_string()));
    }

    pub fn hover(&mut self, expression: &str, pos: usize) {
        self.evaluate(expression, "hover", EvalFor::Hover { expression: expression.to_string(), pos });
    }

    pub fn add_watch(&mut self, expression: &str) {
        self.watches.push(Watch { expression: expression.to_string(), result: Err(String::new()) });
        if self.is_stopped() {
            self.evaluate(expression, "watch", EvalFor::Watch(expression.to_string()));
        }
    }

    pub fn remove_watch(&mut self, expression: &str) {
        self.watches.retain(|w| w.expression != expression);
    }

    /// Opens or closes a line of the tree, asking for its contents if they aren't here yet.
    pub fn toggle_row(&mut self, row: &VarRow) {
        if row.is_scope {
            if !self.collapsed_scopes.remove(&row.path) {
                self.collapsed_scopes.insert(row.path.clone());
            }
        } else if !self.expanded.remove(&row.path) {
            self.expanded.insert(row.path.clone());
        }
        let open = if row.is_scope { !self.collapsed_scopes.contains(&row.path) } else { self.expanded.contains(&row.path) };
        if open && row.reference > 0 && !self.children.contains_key(&row.reference) {
            self.fetch(row.reference, row.path.clone());
        }
    }

    fn fetch(&mut self, reference: i64, path: String) {
        self.send(Pending::Variables { path }, "variables", json!({ "variablesReference": reference }));
    }

    // The tree -----------------------------------------------------------------------------------

    fn push_children(&self, reference: i64, parent: &str, depth: usize, out: &mut Vec<VarRow>) {
        let Some(list) = self.children.get(&reference) else { return };
        for v in list {
            if out.len() >= MAX_ROWS {
                return;
            }
            let path = format!("{parent}/{}", v.name);
            let expanded = v.reference > 0 && self.expanded.contains(&path);
            let changed = self.previous.get(&path).is_some_and(|old| *old != v.value);
            // A struct has no value of its own in some debuggers.
            let value = if v.value.is_empty() && v.reference > 0 { "{…}".to_string() } else { v.value.clone() };
            out.push(VarRow { depth, path: path.clone(), name: v.name.clone(), value, reference: v.reference, expandable: v.reference > 0, expanded, changed, is_scope: false });
            if expanded {
                self.push_children(v.reference, &path, depth + 1, out);
            }
        }
    }

    pub fn variable_rows(&self) -> Vec<VarRow> {
        let mut out = Vec::new();
        for s in &self.scopes {
            let expanded = !self.collapsed_scopes.contains(&s.name);
            out.push(VarRow { depth: 0, path: s.name.clone(), name: s.name.clone(), value: String::new(), reference: s.reference, expandable: true, expanded, changed: false, is_scope: true });
            if expanded {
                self.push_children(s.reference, &s.name, 1, &mut out);
            }
        }
        out
    }

    pub fn watch_rows(&self) -> Vec<(usize, VarRow)> {
        let mut out = Vec::new();
        for (i, w) in self.watches.iter().enumerate() {
            let path = format!("\u{1}watch/{}", w.expression);
            let (value, reference) = match &w.result {
                Ok(v) => (if v.value.is_empty() && v.reference > 0 { "{…}".to_string() } else { v.value.clone() }, v.reference),
                Err(e) => (e.clone(), 0),
            };
            let expanded = reference > 0 && self.expanded.contains(&path);
            let changed = self.previous.get(&path).is_some_and(|old| *old != value);
            out.push((i, VarRow { depth: 0, path: path.clone(), name: w.expression.clone(), value, reference, expandable: reference > 0, expanded, changed, is_scope: false }));
            if expanded {
                let mut kids = Vec::new();
                self.push_children(reference, &path, 1, &mut kids);
                out.extend(kids.into_iter().map(|k| (i, k)));
            }
        }
        out
    }

    // Messages -----------------------------------------------------------------------------------

    pub fn handle(&mut self, incoming: Incoming, bps: &mut Breakpoints) -> Vec<Notice> {
        match incoming {
            Incoming::Stderr(text) => vec![Notice::Output { category: "stderr".into(), text: format!("{text}\n") }],
            Incoming::Server(text) => vec![Notice::Output { category: "console".into(), text: format!("{text}\n") }],
            Incoming::ServerFailed(e) => {
                self.state = RunState::Ended;
                vec![Notice::Failed(e)]
            }
            Incoming::Exited => {
                let was_live = self.is_live();
                self.state = RunState::Ended;
                self.clear_stop();
                if was_live { vec![Notice::Ended { code: self.exit_code }] } else { Vec::new() }
            }
            Incoming::Message(m) => match m["type"].as_str() {
                Some("response") => self.on_response(&m, bps),
                Some("event") => self.on_event(&m, bps),
                Some("request") => {
                    let seq = m["seq"].as_i64().unwrap_or(0);
                    let command = m["command"].as_str().unwrap_or("");
                    match (command, &self.tty) {
                        ("runInTerminal", Some(tty)) => match run_in_terminal(&m["arguments"], tty) {
                            Ok(pid) => self.client.respond(seq, command, json!({ "processId": pid })),
                            Err(e) => {
                                self.client.refuse(seq, command);
                                return vec![Notice::Output { category: "stderr".into(), text: format!("runInTerminal: {e}\n") }];
                            }
                        },
                        _ => self.client.refuse(seq, command),
                    }
                    Vec::new()
                }
                _ => Vec::new(),
            },
        }
    }

    fn clear_stop(&mut self) {
        self.frames.clear();
        self.frame = 0;
        self.scopes.clear();
        self.children.clear();
        // What a value was at the last stop is how the next one knows it changed.
        if !self.values.is_empty() {
            self.previous = std::mem::take(&mut self.values);
        }
    }

    fn refresh_frame(&mut self) {
        self.scopes.clear();
        self.children.clear();
        let Some(id) = self.current_frame_id() else { return };
        self.send(Pending::Scopes, "scopes", json!({ "frameId": id }));
        let exprs: Vec<String> = self.watches.iter().map(|w| w.expression.clone()).collect();
        for e in exprs {
            self.evaluate(&e, "watch", EvalFor::Watch(e.clone()));
        }
    }

    fn show_frame(&self) -> Vec<Notice> {
        match self.frames.get(self.frame) {
            Some(Frame { path: Some(path), line, column, .. }) => vec![Notice::Show { path: path.clone(), line: *line, column: *column }],
            _ => Vec::new(),
        }
    }

    fn on_event(&mut self, m: &Value, bps: &mut Breakpoints) -> Vec<Notice> {
        let body = &m["body"];
        let event = m["event"].as_str().unwrap_or("");
        if matches!(event, "process" | "thread" | "module" | "stopped" | "exited" | "terminated") {
            self.program_seen = true;
        }
        match event {
            "initialized" => {
                if self.configured {
                    return Vec::new();
                }
                self.configured = true;
                let all: Vec<(PathBuf, Vec<Breakpoint>)> = bps.iter().map(|(p, l)| (p.clone(), l.clone())).collect();
                for (path, list) in all {
                    self.send_breakpoints(&path, &list);
                }
                self.send(Pending::Quiet, "configurationDone", json!({}));
                if self.state == RunState::Starting {
                    self.state = RunState::Running;
                }
                if self.launch_last {
                    // It may not answer until the program first stops, so the clock starts here.
                    let args = self.launch.clone();
                    self.send(Pending::Launch, self.request, args);
                    self.launched_at = Some(Instant::now());
                }
                Vec::new()
            }
            // A board stops as gdb connects: before anything else, reset it and flash it.
            "stopped" if self.remote.as_ref().is_some_and(|r| !r.started) => {
                if let Some(r) = self.remote.as_mut() {
                    r.started = true;
                }
                self.state = RunState::Stopped { reason: "attach".into(), description: None };
                if let Some(t) = body["threadId"].as_i64() {
                    self.thread = Some(t);
                }
                self.next_setup()
            }
            // probe-rs: the board's log, over RTT.
            "probe-rs-rtt-channel-config" => {
                let channel = body["channelNumber"].as_i64().unwrap_or(0);
                self.send(Pending::Quiet, "rttWindowOpened", json!({ "channelNumber": channel, "windowIsOpen": true }));
                Vec::new()
            }
            "probe-rs-rtt-data" => {
                let text = body["data"].as_str().unwrap_or("").to_string();
                if text.is_empty() { Vec::new() } else { vec![Notice::Output { category: "stdout".into(), text }] }
            }
            "probe-rs-show-message" => {
                let category = if body["severity"].as_str() == Some("error") { "stderr" } else { "console" };
                vec![Notice::Output { category: category.into(), text: format!("{}\n", body["message"].as_str().unwrap_or("")) }]
            }
            "stopped" => {
                let reason = body["reason"].as_str().unwrap_or("").to_string();
                let description = body["description"].as_str().or(body["text"].as_str()).map(str::to_string);
                self.state = RunState::Stopped { reason, description };
                self.clear_stop();
                self.end_run_to(bps);
                if let Some(t) = body["threadId"].as_i64() {
                    self.thread = Some(t);
                }
                self.send(Pending::Threads, "threads", json!({}));
                if let Some(t) = self.thread {
                    self.send(Pending::StackTrace { thread: t }, "stackTrace", json!({ "threadId": t, "startFrame": 0, "levels": 64 }));
                }
                vec![Notice::Stopped]
            }
            "continued" => {
                if self.state != RunState::Ended {
                    self.state = RunState::Running;
                    self.clear_stop();
                }
                vec![Notice::Continued]
            }
            "thread" => {
                if self.thread.is_none() {
                    self.thread = body["threadId"].as_i64();
                }
                Vec::new()
            }
            "output" => {
                let category = body["category"].as_str().unwrap_or("console").to_string();
                let text = body["output"].as_str().unwrap_or("").to_string();
                if category == "telemetry" || text.is_empty() || self.is_noise(&text) {
                    return Vec::new();
                }
                vec![Notice::Output { category, text }]
            }
            "breakpoint" => {
                self.on_breakpoint_changed(&body["breakpoint"], bps);
                Vec::new()
            }
            "exited" => {
                self.exit_code = body["exitCode"].as_i64();
                Vec::new()
            }
            "terminated" => {
                if !self.is_live() {
                    return Vec::new();
                }
                self.stop();
                self.clear_stop();
                vec![Notice::Ended { code: self.exit_code }]
            }
            _ => Vec::new(),
        }
    }

    /// The next of the commands that get a board ready, or, after the last, on to `main`.
    fn next_setup(&mut self) -> Vec<Notice> {
        let Some(r) = self.remote.as_mut() else { return Vec::new() };
        if let Some(cmd) = r.commands.pop_front() {
            self.send(Pending::Setup(cmd.clone()), "evaluate", json!({ "expression": cmd, "context": "repl" }));
            return Vec::new();
        }
        if r.stop_on_entry {
            // Shown where it is, as any other stop.
            self.send(Pending::Threads, "threads", json!({}));
            if let Some(t) = self.thread {
                self.send(Pending::StackTrace { thread: t }, "stackTrace", json!({ "threadId": t, "startFrame": 0, "levels": 64 }));
            }
            return vec![Notice::Stopped];
        }
        let thread = self.thread.unwrap_or(1);
        self.send(Pending::Quiet, "continue", json!({ "threadId": thread }));
        self.state = RunState::Running;
        vec![Notice::Continued]
    }

    /// GDB reports a breakpoint set before the program is loaded as an error, and then places it.
    fn is_noise(&self, text: &str) -> bool {
        self.kind == "gdb" && (text.contains("No source file named") || (text.starts_with("Breakpoint ") && text.trim_end().ends_with("pending.")))
    }

    fn on_breakpoint_changed(&mut self, b: &Value, bps: &mut Breakpoints) {
        let Some(id) = b["id"].as_i64() else { return };
        let verified = b["verified"].as_bool().unwrap_or(false);
        let line = b["line"].as_u64().map(|l| (l as usize).saturating_sub(1));
        for list in bps.values_mut() {
            if let Some(bp) = list.iter_mut().find(|x| x.id == Some(id)) {
                bp.state = if verified { BpState::Verified } else { BpState::Pending };
                if let Some(l) = line {
                    bp.line = l;
                }
                list.sort_by_key(|x| x.line);
                return;
            }
        }
    }

    fn on_response(&mut self, m: &Value, bps: &mut Breakpoints) -> Vec<Notice> {
        let Some(pending) = m["request_seq"].as_i64().and_then(|s| self.pending.remove(&s)) else { return Vec::new() };
        let ok = m["success"].as_bool().unwrap_or(false);
        let message = m["message"].as_str().unwrap_or("").to_string();
        let body = &m["body"];
        match pending {
            Pending::Initialize => {
                if !ok {
                    self.state = RunState::Ended;
                    return vec![Notice::Failed(message)];
                }
                self.capabilities = body.clone();
                if let Some(tty) = self.tty.clone().filter(|_| self.kind == "gdb") {
                    let cmd = format!("set inferior-tty {}", tty.display());
                    self.send(Pending::Quiet, "evaluate", json!({ "expression": cmd, "context": "repl" }));
                }
                if !self.launch_last {
                    let args = self.launch.clone();
                    // Some adapters answer this only after configurationDone, so nothing waits on it.
                    self.send(Pending::Launch, self.request, args);
                }
                Vec::new()
            }
            Pending::Launch => {
                if ok {
                    self.launched_at = Some(Instant::now());
                    return Vec::new();
                }
                self.stop();
                let detail = m["body"]["error"]["format"].as_str().filter(|s| !s.is_empty()).unwrap_or(&message).to_string();
                vec![Notice::Failed(detail)]
            }
            Pending::SetBreakpoints { path, lines } => {
                if !ok {
                    return vec![Notice::Output { category: "stderr".into(), text: format!("setBreakpoints: {message}\n") }];
                }
                let Some(list) = bps.get_mut(&path) else { return Vec::new() };
                let results = body["breakpoints"].as_array().cloned().unwrap_or_default();
                for (asked, r) in lines.iter().zip(&results) {
                    if let Some(bp) = list.iter_mut().find(|b| b.line == *asked && b.enabled) {
                        bp.id = r["id"].as_i64();
                        bp.state = if r["verified"].as_bool().unwrap_or(false) { BpState::Verified } else { BpState::Pending };
                        // The debugger moves it to the nearest line with code.
                        if let Some(l) = r["line"].as_u64() {
                            bp.line = (l as usize).saturating_sub(1);
                        }
                    }
                }
                list.sort_by_key(|b| b.line);
                Vec::new()
            }
            Pending::Threads => {
                self.threads = body["threads"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| Thread { id: t["id"].as_i64().unwrap_or(0), name: t["name"].as_str().unwrap_or("").to_string() })
                    .collect();
                Vec::new()
            }
            Pending::StackTrace { thread } => {
                if !ok || self.thread != Some(thread) || !self.is_stopped() {
                    return Vec::new();
                }
                self.frames = body["stackFrames"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|f| Frame {
                        id: f["id"].as_i64().unwrap_or(0),
                        name: f["name"].as_str().unwrap_or("").to_string(),
                        path: f["source"]["path"].as_str().map(PathBuf::from).filter(|p| p.is_file()),
                        line: (f["line"].as_u64().unwrap_or(1) as usize).saturating_sub(1),
                        column: (f["column"].as_u64().unwrap_or(1) as usize).saturating_sub(1),
                    })
                    .collect();
                // Stopping inside a library with no source: the first frame that has some.
                self.frame = self.frames.iter().position(|f| f.path.is_some()).unwrap_or(0);
                self.refresh_frame();
                self.show_frame()
            }
            Pending::Scopes => {
                if !ok {
                    return Vec::new();
                }
                self.scopes.clear();
                for s in body["scopes"].as_array().into_iter().flatten() {
                    let name = s["name"].as_str().unwrap_or("").to_string();
                    let reference = s["variablesReference"].as_i64().unwrap_or(0);
                    // Registers and Python's globals are long and rarely what you came for.
                    let closed_by_default = s["expensive"].as_bool().unwrap_or(false)
                        || s["presentationHint"].as_str() == Some("registers")
                        || name == "Registers"
                        || name == "Globals";
                    if closed_by_default && !self.expanded.contains(&format!("\u{1}open/{name}")) {
                        self.collapsed_scopes.insert(name.clone());
                    }
                    if !self.collapsed_scopes.contains(&name) && reference > 0 {
                        self.fetch(reference, name.clone());
                    }
                    self.scopes.push(Scope { name, reference });
                }
                Vec::new()
            }
            Pending::Variables { path } => {
                if !ok {
                    return Vec::new();
                }
                let list: Vec<Variable> = body["variables"].as_array().into_iter().flatten().map(Variable::from_json).collect();
                let Some(reference) = m["__reference"].as_i64().or_else(|| self.reference_for(&path)) else { return Vec::new() };
                for v in &list {
                    let p = format!("{path}/{}", v.name);
                    self.values.insert(p.clone(), v.value.clone());
                    // What was open at the last stop stays open.
                    if v.reference > 0 && self.expanded.contains(&p) {
                        self.fetch(v.reference, p);
                    }
                }
                self.children.insert(reference, list);
                Vec::new()
            }
            Pending::Evaluate(why) => self.on_evaluate(ok, &message, body, why),
            Pending::Setup(cmd) => {
                let mut out = Vec::new();
                if let Some(text) = body["result"].as_str().filter(|t| !t.trim().is_empty()) {
                    out.push(Notice::Output { category: "console".into(), text: format!("{}\n", text.trim_end()) });
                }
                if !ok {
                    // Without the firmware on the board there's nothing to debug.
                    if cmd == "load" || cmd.starts_with("file ") {
                        self.stop();
                        out.push(Notice::Failed(format!("{cmd}: {message}")));
                        return out;
                    }
                    out.push(Notice::Output { category: "stderr".into(), text: format!("{cmd}: {message}\n") });
                }
                out.extend(self.next_setup());
                out
            }
            Pending::SetValue => {
                if !ok {
                    return vec![Notice::Output { category: "stderr".into(), text: format!("{message}\n") }];
                }
                // Other variables may hang on this one: all of them are read again.
                if self.is_stopped() {
                    self.refresh_frame();
                }
                Vec::new()
            }
            Pending::Quiet => {
                // Stepping failing leaves everything where it was.
                if !ok && !message.is_empty() && m["command"] != "disconnect" {
                    return vec![Notice::Output { category: "stderr".into(), text: format!("{}: {message}\n", m["command"].as_str().unwrap_or("")) }];
                }
                Vec::new()
            }
        }
    }

    /// A tree path back to the debugger's number for it.
    fn reference_for(&self, path: &str) -> Option<i64> {
        if let Some(s) = self.scopes.iter().find(|s| s.name == path) {
            return Some(s.reference);
        }
        if let Some(expr) = path.strip_prefix("\u{1}watch/")
            && let Some(Watch { result: Ok(v), .. }) = self.watches.iter().find(|w| w.expression == expr)
        {
            return Some(v.reference);
        }
        let (parent, name) = path.rsplit_once('/')?;
        let parent_ref = self.reference_for(parent)?;
        self.children.get(&parent_ref)?.iter().find(|v| v.name == name).map(|v| v.reference)
    }

    fn on_evaluate(&mut self, ok: bool, message: &str, body: &Value, why: EvalFor) -> Vec<Notice> {
        let value = body["result"].as_str().unwrap_or("").to_string();
        let reference = body["variablesReference"].as_i64().unwrap_or(0);
        match why {
            EvalFor::Console(input) => {
                let shown = if value.is_empty() && reference > 0 { "{…}".to_string() } else { value };
                let echo = Notice::Output { category: "input".into(), text: format!("> {input}\n") };
                if ok {
                    vec![echo, Notice::Output { category: "result".into(), text: format!("{shown}\n") }]
                } else {
                    vec![echo, Notice::Output { category: "stderr".into(), text: format!("{message}\n") }]
                }
            }
            EvalFor::Watch(expression) => {
                let path = format!("\u{1}watch/{expression}");
                let mut fetch = None;
                if let Some(w) = self.watches.iter_mut().find(|w| w.expression == expression) {
                    w.result = if ok {
                        let shown = if value.is_empty() && reference > 0 { "{…}".to_string() } else { value.clone() };
                        Ok(Variable { name: expression.clone(), value: shown, kind: None, reference })
                    } else {
                        Err(message.to_string())
                    };
                    if ok {
                        self.values.insert(path.clone(), if value.is_empty() && reference > 0 { "{…}".to_string() } else { value });
                        if reference > 0 && self.expanded.contains(&path) {
                            fetch = Some(reference);
                        }
                    }
                }
                if let Some(r) = fetch {
                    self.fetch(r, path);
                }
                Vec::new()
            }
            EvalFor::Hover { expression, pos } => {
                if ok && !value.is_empty() {
                    vec![Notice::Hover { expression, value, pos }]
                } else if ok && reference > 0 {
                    vec![Notice::Hover { expression, value: "{…}".to_string(), pos }]
                } else {
                    Vec::new()
                }
            }
        }
    }
}

/// The expression under the pointer: `p.x` from the `x`, without running anything.
pub fn hover_expression(line: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    // A position from the mouse rounds to the nearest gap between chars, so the right half of a
    // word's last letter lands on whatever comes after it.
    let col = if chars.get(col).copied().is_some_and(is_word) {
        col
    } else if col > 0 && chars.get(col - 1).copied().is_some_and(is_word) {
        col - 1
    } else {
        return None;
    };
    let mut a = col;
    let mut b = col;
    while b + 1 < chars.len() && is_word(chars[b + 1]) {
        b += 1;
    }
    loop {
        while a > 0 && is_word(chars[a - 1]) {
            a -= 1;
        }
        if a >= 2 && chars[a - 1] == '.' && is_word(chars[a - 2]) {
            a -= 1;
        } else if a >= 3 && chars[a - 2] == '-' && chars[a - 1] == '>' && is_word(chars[a - 3]) {
            a -= 2;
        } else {
            break;
        }
    }
    let text: String = chars[a..=b].iter().collect();
    // A number isn't worth asking a debugger about.
    if text.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, RecvTimeoutError};

    #[test]
    fn breakpoints_toggle_in_order_and_follow_their_lines() {
        let mut l = Vec::new();
        assert!(toggle_breakpoint(&mut l, 5));
        assert!(toggle_breakpoint(&mut l, 2));
        assert_eq!(l.iter().map(|b| b.line).collect::<Vec<_>>(), vec![2, 5]);
        assert!(!toggle_breakpoint(&mut l, 5));
        assert_eq!(l.len(), 1);
    }

    /// Runs an edit on a document and returns where lines 0..n end up.
    fn lines_after(src: &str, at: usize, remove: usize, insert: &str, n: usize) -> Vec<Option<usize>> {
        use crate::buffer::{Document, Edit, EditKind};
        use crate::cursor::{Cursors, Selection};
        let mut doc = Document::new(src);
        let mut c = Cursors::single(Selection::cursor(0));
        doc.take_log();
        doc.apply(vec![Edit::new(at, at + remove, insert.to_string())], &mut c, EditKind::Other, |_, _| {});
        let log = doc.take_log();
        (0..n).map(|l| log[0].map_line(l)).collect()
    }

    #[test]
    fn marks_stay_with_their_code() {
        let src = "aa\nbb\ncc\ndd\n";
        // Enter at the start of a line pushes it down; in the middle or at the end, it stays.
        assert_eq!(lines_after(src, 3, 0, "\n", 4), vec![Some(0), Some(2), Some(3), Some(4)]);
        assert_eq!(lines_after(src, 4, 0, "\n", 4), vec![Some(0), Some(1), Some(3), Some(4)]);
        assert_eq!(lines_after(src, 5, 0, "\n", 4), vec![Some(0), Some(1), Some(3), Some(4)]);
        // Typing on a line changes nothing.
        assert_eq!(lines_after(src, 4, 0, "x", 4), vec![Some(0), Some(1), Some(2), Some(3)]);
        // Deleting a whole line: its mark goes, and the next line's stays on the next line's code.
        assert_eq!(lines_after(src, 3, 3, "", 4), vec![Some(0), None, Some(1), Some(2)]);
        // Backspace at the start of a line: it joins the one above.
        assert_eq!(lines_after(src, 5, 1, "", 4), vec![Some(0), Some(1), Some(1), Some(2)]);
        // The last line, deleted along with the newline before it.
        assert_eq!(lines_after("aa\nbb", 2, 3, "", 2), vec![Some(0), None]);
        // Across lines, from the middle of one to the middle of another.
        assert_eq!(lines_after(src, 1, 3, "", 4), vec![Some(0), Some(0), Some(1), Some(2)]);
        // ...and to the end of a line, which leaves nothing of it.
        assert_eq!(lines_after(src, 1, 4, "", 4), vec![Some(0), None, Some(1), Some(2)]);
        // A pasted block above.
        assert_eq!(lines_after(src, 3, 0, "x\ny\n", 4), vec![Some(0), Some(3), Some(4), Some(5)]);
    }

    #[test]
    fn shifting_keeps_order_and_merges() {
        let mut l: Vec<_> = [2, 5, 9].map(Breakpoint::at).into();
        shift_breakpoints(&mut l, |x| if x == 5 { None } else { Some(x + 1) });
        assert_eq!(l.iter().map(|b| b.line).collect::<Vec<_>>(), vec![3, 10]);
        let mut l: Vec<_> = [1, 2].map(Breakpoint::at).into();
        shift_breakpoints(&mut l, |_| Some(1));
        assert_eq!(l.len(), 1);
    }

    #[test]
    fn breakpoints_survive_the_session_file_but_not_their_state() {
        let mut b = Breakpoint::at(4);
        b.condition = Some("x > 1".into());
        b.state = BpState::Verified;
        b.id = Some(9);
        let back: Breakpoint = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
        assert_eq!((back.line, back.condition.as_deref(), back.enabled, back.state, back.id), (4, Some("x > 1"), true, BpState::Unknown, None));
        let old: Breakpoint = serde_json::from_str(r#"{"line":1}"#).unwrap();
        assert!(old.enabled);
    }

    #[test]
    fn hover_finds_member_chains() {
        let at = |s: &str| {
            let col = s.chars().position(|c| c == '|').unwrap();
            let text: String = s.chars().filter(|c| *c != '|').collect();
            hover_expression(&text, col)
        };
        assert_eq!(at("int |total = 1;").as_deref(), Some("total"));
        assert_eq!(at("int to|tal = 1;").as_deref(), Some("total"));
        assert_eq!(at("x = p.|y + 1").as_deref(), Some("p.y"));
        assert_eq!(at("x = p|.y + 1").as_deref(), Some("p"));
        assert_eq!(at("return r|;").as_deref(), Some("r"), "the right half of the last letter");
        assert_eq!(at("a = |;"), None);
        assert_eq!(at("x = a->b.|c;").as_deref(), Some("a->b.c"));
        assert_eq!(at("x = 1|0;"), None);
        assert_eq!(at("x =| 1"), None);
        assert_eq!(at("ação = |pão").as_deref(), Some("pão"));
    }

    #[test]
    fn launch_json_in_both_shapes() {
        let list = parse_configs(
            r#"// comment
            [{ "name": "a", "type": "cppdbg", "program": "x", "args": ["1"], "stopOnEntry": true, "miMode": "gdb", }]"#,
        )
        .unwrap();
        assert_eq!(list[0].kind, "cppdbg");
        assert!(list[0].stop_on_entry);
        assert_eq!(list[0].extra["miMode"], "gdb");
        let list = parse_configs(r#"{ "version": "0.2.0", "configurations": [{ "name": "b", "type": "python", "program": "m.py" }] }"#).unwrap();
        assert_eq!(list[0].name, "b");
        assert!(parse_configs("not json").is_err());
        assert!(parse_configs(LAUNCH_TEMPLATE).is_ok());
    }

    #[test]
    fn a_plan_fills_in_the_project_paths() {
        let dir = tempfile();
        std::fs::create_dir_all(dir.join(".tinta")).unwrap();
        std::fs::write(
            dir.join(LAUNCH_PATH),
            r#"[{ "name": "um", "type": "cppdbg", "program": "bin/a", "args": ["${file}"], "cwd": "sub", "build": "make -C ${workspaceFolder}" },
                { "name": "dois", "type": "debugpy", "program": "${fileDirname}/${fileBasenameNoExtension}.py" }]"#,
        )
        .unwrap();
        let file = dir.join("src/m.c");
        let p = plan(&dir, Some(&file), "C", None, true, None, false).unwrap();
        assert_eq!(p.config.kind, "gdb");
        assert_eq!(p.config.program, dir.join("bin/a").to_string_lossy());
        assert_eq!(p.config.cwd, dir.join("sub").to_string_lossy());
        assert_eq!(p.config.args, vec![file.to_string_lossy().to_string()]);
        assert_eq!(p.config.build.as_deref(), Some(format!("make -C {}", dir.display()).as_str()));
        let p = plan(&dir, Some(&file), "C", Some("dois"), true, None, false).unwrap();
        assert_eq!(p.config.kind, "python");
        assert_eq!(p.config.program, dir.join("src/m.py").to_string_lossy());
        // Without a type there's nothing to run it with, and it says so.
        std::fs::write(dir.join(LAUNCH_PATH), r#"[{ "name": "x", "program": "a" }]"#).unwrap();
        assert!(plan(&dir, None, "C", None, true, None, false).unwrap_err().contains("type"));
    }

    #[test]
    fn plans_without_a_configuration() {
        let dir = tempfile();
        let c = dir.join("a b.c");
        std::fs::write(&c, "").unwrap();
        if native_kind().is_ok() {
            let p = plan(&dir, Some(&c), "C", None, true, None, false).unwrap();
            let build = p.config.build.clone().unwrap();
            assert!(build.starts_with("cc -g -O0") && build.contains(&format!("'{}'", c.display())), "{build}");
            assert!(p.config.program.ends_with("/a b"));
            std::fs::write(dir.join("Makefile"), "").unwrap();
            let p = plan(&dir, Some(&c), "C", None, true, None, false).unwrap();
            assert_eq!(p.config.build.as_deref(), Some("make"));
            assert_eq!(p.program_from, ProgramFrom::NewestExecutable);
            std::fs::write(dir.join("Cargo.toml"), "").unwrap();
            let p = plan(&dir, Some(&dir.join("m.rs")), "Rust", None, true, None, false).unwrap();
            assert_eq!(p.config.build.as_deref(), Some("cargo build"));
        }
        assert!(plan(&dir, None, "Markdown", None, true, None, false).is_err() || dir.join("Cargo.toml").exists());
    }

    #[test]
    fn the_newest_executable_is_a_real_one() {
        let dir = tempfile();
        use std::os::unix::fs::PermissionsExt;
        let script = dir.join("run.sh");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(newest_executable(&dir), None, "a script isn't what a C build makes");
        std::fs::create_dir(dir.join("build")).unwrap();
        let elf = dir.join("build/app");
        std::fs::write(&elf, b"\x7fELF rest").unwrap();
        assert_eq!(newest_executable(&dir), None, "not executable");
        std::fs::set_permissions(&elf, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(newest_executable(&dir), Some(elf));
    }

    /// A C file built for debugging, with the user's compiler.
    fn compile(src: &Path, exe: &Path) -> bool {
        let args = [std::ffi::OsStr::new("-g"), "-O0".as_ref(), src.as_os_str(), "-o".as_ref(), exe.as_os_str()];
        crate::host::command("cc", args, None, &[]).status().is_ok_and(|s| s.success())
    }

    fn tempfile() -> PathBuf {
        static N: AtomicI64 = AtomicI64::new(0);
        // Where the user's programs (cc, gdb) see it too, in a Flatpak as well.
        let d = crate::host::temp_dir().join(format!("tinta-dap-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    // Against the real thing --------------------------------------------------------------------

    const PROGRAM: &str = r#"#include <stdio.h>

struct ponto { int x; int y; };

int dobro(int n) {
    int r = n * 2;
    return r;
}

int main(void) {
    struct ponto p = { 3, 4 };
    int total = dobro(p.x);
    total += p.y;
    printf("total %d\n", total);
    return 7;
}
"#;

    struct Rig {
        d: Debugger,
        rx: Receiver<Incoming>,
        bps: Breakpoints,
        notices: Vec<Notice>,
    }

    impl Rig {
        fn start(plan: &Plan, bps: Breakpoints) -> Self {
            let (tx, rx) = channel();
            let tx = Mutex::new(tx);
            let sink: Sink = Arc::new(move |m| {
                let _ = tx.lock().unwrap().send(m);
            });
            Self { d: Debugger::start(plan, &[], sink, None).unwrap(), rx, bps, notices: Vec::new() }
        }

        /// Pumps messages until the condition holds, and then until things go quiet, so the
        /// follow-up requests a stop sets off have finished too.
        fn until(&mut self, what: &str, cond: impl Fn(&Self) -> bool) {
            let end = Instant::now() + Duration::from_secs(20);
            loop {
                match self.rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(m) => {
                        let n = self.d.handle(m, &mut self.bps);
                        self.notices.extend(n);
                    }
                    Err(RecvTimeoutError::Timeout) if cond(self) => return,
                    Err(RecvTimeoutError::Timeout) => {
                        // What the application's slow tick does.
                        if let Some(n) = self.d.check_started() {
                            self.notices.push(n);
                        }
                    }
                    // The adapter has gone, and with it anything more to wait for.
                    Err(RecvTimeoutError::Disconnected) => {
                        assert!(cond(self), "{what}: the adapter left in state {:?}\n{:?}", self.d.state, self.notices);
                        return;
                    }
                }
                assert!(Instant::now() < end, "{what}: timed out in state {:?}\n{:?}", self.d.state, self.notices);
            }
        }

        fn line(&self) -> usize {
            self.d.frames[self.d.frame].line
        }

        fn var(&self, path: &str) -> Option<String> {
            self.d.variable_rows().into_iter().find(|r| r.path == path).map(|r| r.value)
        }

        fn stopped_with_vars(&self) -> bool {
            self.d.is_stopped() && !self.d.frames.is_empty() && !self.d.scopes.is_empty()
        }
    }

    fn c_program() -> Option<(PathBuf, Plan)> {
        if !gdb_with_dap() {
            eprintln!("skipped: no gdb with DAP");
            return None;
        }
        let dir = tempfile();
        let src = dir.join("prog.c");
        std::fs::write(&src, PROGRAM).unwrap();
        let exe = dir.join("prog");
        if !compile(&src, &exe) {
            return None;
        }
        let config = LaunchConfig {
            name: "prog".into(),
            kind: "gdb".into(),
            program: exe.to_string_lossy().into_owned(),
            cwd: dir.to_string_lossy().into_owned(),
            ..LaunchConfig::default()
        };
        Some((src, Plan { root: dir, config, program_from: ProgramFrom::Known }))
    }

    #[test]
    fn a_whole_session_with_gdb() {
        let Some((src, plan)) = c_program() else { return };
        let mut bps = Breakpoints::new();
        // Line 6 in the file: `int r = n * 2;`. Line 14: the printf. Line 2 is blank, and gdb moves it.
        bps.insert(src.clone(), vec![Breakpoint::at(5), Breakpoint::at(13)]);
        let mut r = Rig::start(&plan, bps);

        r.until("first stop", Rig::stopped_with_vars);
        assert!(r.bps[&src].iter().all(|b| b.state == BpState::Verified), "{:?}", r.bps);
        assert_eq!(r.line(), 5);
        assert!(matches!(&r.d.state, RunState::Stopped { reason, .. } if reason == "breakpoint"));
        assert_eq!(r.d.frames[0].name, "dobro");
        assert_eq!(r.d.frames[1].name, "main");
        assert!(r.notices.iter().any(|n| matches!(n, Notice::Show { path, line: 5, .. } if *path == src)));
        assert_eq!(r.var("Arguments/n").as_deref(), Some("3"));
        // Registers are there, closed.
        assert!(r.d.variable_rows().iter().any(|v| v.name == "Registers" && !v.expanded));
        assert!(r.var("Registers/rax").is_none());

        // Expressions in the console: not taken as gdb commands, or `n*10` would step.
        r.d.console_input("n*10");
        r.until("evaluate", |r| r.notices.iter().any(|n| matches!(n, Notice::Output { category, text } if category == "result" && text == "30\n")));
        assert_eq!(r.line(), 5);
        // Gdb's own commands are one prefix away.
        r.d.console_input("-exec p n+1");
        r.until("exec", |r| r.notices.iter().any(|n| matches!(n, Notice::Output { text, .. } if text.contains("= 4"))));
        // A bad expression says why.
        r.d.console_input("nada_disso");
        r.until("bad evaluate", |r| r.notices.iter().any(|n| matches!(n, Notice::Output { category, text } if category == "stderr" && text.contains("nada_disso"))));
        // Hover.
        r.d.hover("n", 42);
        r.until("hover", |r| r.notices.iter().any(|n| matches!(n, Notice::Hover { value, pos: 42, .. } if value == "3")));

        // A step changes `r`, and the tree says it changed.
        assert!(r.var("Locals/r").is_some());
        let before = r.var("Locals/r").unwrap();
        r.d.step_over();
        assert_eq!(r.d.state, RunState::Running);
        r.until("step", |r| r.stopped_with_vars() && r.line() == 6 && r.var("Locals/r").is_some());
        assert_eq!(r.var("Locals/r").as_deref(), Some("6"));
        assert_ne!(before, "6");
        let row = r.d.variable_rows().into_iter().find(|v| v.path == "Locals/r").unwrap();
        assert!(row.changed, "r went from {before} to 6");
        assert!(!r.d.variable_rows().into_iter().find(|v| v.path == "Arguments/n").unwrap().changed);

        // Going up a frame shows that frame's variables.
        let shown = r.d.select_frame(1);
        assert!(matches!(&shown[0], Notice::Show { line: 11, .. }), "{shown:?}");
        r.until("frame", |r| r.var("Locals/total").is_some());
        assert_eq!(r.d.exec_location().map(|(_, l, top)| (l, top)), Some((11, false)));

        // A struct opens, and stays open across stops.
        let p = r.d.variable_rows().into_iter().find(|v| v.path == "Locals/p").unwrap();
        assert!(p.expandable && !p.expanded);
        assert_eq!(p.value, "{…}", "gdb gives structs no value");
        r.d.toggle_row(&p);
        r.until("expand", |r| r.var("Locals/p/x").is_some());
        assert_eq!(r.var("Locals/p/y").as_deref(), Some("4"));

        // Watches follow along, and a watch on a struct opens too.
        r.d.add_watch("p.x + p.y");
        r.d.add_watch("nao_existe");
        r.until("watch", |r| matches!(&r.d.watches[0].result, Ok(v) if v.value == "7"));
        assert!(!matches!(&r.d.watches[1].result, Err(e) if e.is_empty()));

        // On to the next breakpoint, added while stopped, and the one that was there.
        let main_line = 12;
        crate::dap::toggle_breakpoint(r.bps.get_mut(&src).unwrap(), main_line);
        let list = r.bps[&src].clone();
        r.d.send_breakpoints(&src, &list);
        r.d.continue_();
        r.until("next breakpoint", |r| r.stopped_with_vars() && r.line() == main_line);
        assert_eq!(r.var("Locals/total").as_deref(), Some("6"));
        assert_eq!(r.var("Locals/p/x").as_deref(), Some("3"), "still open");
        assert!(r.bps[&src].iter().find(|b| b.line == main_line).unwrap().state == BpState::Verified);

        r.d.continue_();
        r.until("printf line", |r| r.stopped_with_vars() && r.line() == 13);
        assert_eq!(r.var("Locals/total").as_deref(), Some("10"));
        r.d.continue_();
        r.until("the end", |r| !r.d.is_live());
        assert!(r.notices.contains(&Notice::Ended { code: Some(7) }), "{:?}", r.notices);
        let output: String = r.notices.iter().filter_map(|n| if let Notice::Output { text, .. } = n { Some(text.as_str()) } else { None }).collect();
        assert!(output.contains("total 10"), "the program's output: {output}");
        assert!(!output.contains("No source file"), "gdb's noise is filtered: {output}");
    }

    #[test]
    fn input_new_values_and_running_to_a_line() {
        let Some((_, plan)) = c_program() else { return };
        let dir = plan.root.clone();
        let src = dir.join("le.c");
        std::fs::write(
            &src,
            "#include <stdio.h>\nint main(void) {\n    int n = 0;\n    if (scanf(\"%d\", &n) != 1) return 1;\n    int dobro = n * 2;\n    for (int i = 0; i < 3; i++) {\n        dobro += 1;\n    }\n    printf(\"resultado %d\\n\", dobro);\n    return 0;\n}\n",
        )
        .unwrap();
        let exe = dir.join("le");
        assert!(compile(&src, &exe));
        let mut plan = plan;
        plan.config.program = exe.to_string_lossy().into_owned();
        assert!(plan.config.wants_terminal());

        // The program's terminal: what's typed reaches the program, and what it prints comes back.
        let tty = open_tty().unwrap();
        let mut typed = std::fs::File::from(tty.master.try_clone().unwrap());
        let shown = Arc::new(Mutex::new(String::new()));
        {
            let mut out = std::fs::File::from(tty.master.try_clone().unwrap());
            let shown = shown.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 512];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    shown.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            });
        }
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let sink: Sink = Arc::new(move |m| {
            let _ = tx.lock().unwrap().send(m);
        });
        let mut bps = Breakpoints::new();
        bps.insert(src.clone(), vec![Breakpoint::at(4)]);
        let mut r = Rig { d: Debugger::start(&plan, &[], sink, Some(&tty)).unwrap(), rx, bps, notices: Vec::new() };
        typed.write_all(b"21\n").unwrap();
        r.until("after scanf", |r| r.stopped_with_vars() && r.line() == 4);
        assert_eq!(r.var("Locals/n").as_deref(), Some("21"), "read from the terminal");

        // A new value, which the program then goes on with.
        let n = r.d.variable_rows().into_iter().find(|v| v.path == "Locals/n").unwrap();
        assert!(r.d.can_set(&n));
        r.d.set_value(&n, "50");
        r.until("new value", |r| r.var("Locals/n").as_deref() == Some("50"));
        let scope = r.d.variable_rows().into_iter().find(|v| v.is_scope).unwrap();
        assert!(!r.d.can_set(&scope));

        // Run to a line inside the loop: it stops there once, and the file's breakpoints are its
        // own again afterwards, so the loop doesn't stop there on the next turns.
        let list = r.bps[&src].clone();
        r.d.run_to(&src, 6, &list);
        r.until("run to cursor", |r| r.stopped_with_vars() && r.line() == 6);
        assert_eq!(r.var("Locals/dobro").as_deref(), Some("100"));
        assert_eq!(r.bps[&src].iter().map(|b| b.line).collect::<Vec<_>>(), vec![4]);
        r.d.continue_();
        r.until("the end", |r| !r.d.is_live());
        let end = Instant::now() + Duration::from_secs(5);
        while !shown.lock().unwrap().contains("resultado 103") && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        let shown = shown.lock().unwrap().clone();
        assert!(shown.contains("resultado 103"), "the terminal showed: {shown:?}");
        let console: String = r.notices.iter().filter_map(|n| if let Notice::Output { text, .. } = n { Some(text.as_str()) } else { None }).collect();
        assert!(!console.contains("resultado"), "the output went to the terminal, not the console: {console}");
    }

    #[test]
    fn a_python_program_in_its_terminal() {
        if resolve_adapter("python").is_err() {
            eprintln!("skipped: no debugpy");
            return;
        }
        let dir = tempfile();
        let src = dir.join("le.py");
        std::fs::write(&src, "n = int(input('numero: '))\ndobro = n * 2\nprint('dobro', dobro)\n").unwrap();
        let config = LaunchConfig { name: "le".into(), kind: "python".into(), program: src.to_string_lossy().into_owned(), cwd: dir.to_string_lossy().into_owned(), ..LaunchConfig::default() };
        assert!(config.wants_terminal());
        let plan = Plan { root: dir, config, program_from: ProgramFrom::Known };
        let tty = open_tty().unwrap();
        let mut typed = std::fs::File::from(tty.master.try_clone().unwrap());
        let shown = Arc::new(Mutex::new(String::new()));
        {
            let mut out = std::fs::File::from(tty.master.try_clone().unwrap());
            let shown = shown.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 512];
                while let Ok(n) = out.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    shown.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            });
        }
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let sink: Sink = Arc::new(move |m| {
            let _ = tx.lock().unwrap().send(m);
        });
        let mut bps = Breakpoints::new();
        bps.insert(src.clone(), vec![Breakpoint::at(1)]);
        let mut r = Rig { d: Debugger::start(&plan, &[], sink, Some(&tty)).unwrap(), rx, bps, notices: Vec::new() };
        // The prompt shows in the terminal, and what's typed there is what input() reads.
        let end = Instant::now() + Duration::from_secs(20);
        while !shown.lock().unwrap().contains("numero:") && Instant::now() < end {
            if let Ok(m) = r.rx.recv_timeout(Duration::from_millis(100)) {
                let n = r.d.handle(m, &mut r.bps);
                r.notices.extend(n);
            }
        }
        assert!(shown.lock().unwrap().contains("numero:"), "the terminal showed {:?}\n{:?}", shown.lock().unwrap(), r.notices);
        typed.write_all(b"21\n").unwrap();
        // At the module's level, n is in Locals or Globals depending on debugpy's version.
        r.until("after input", |r| r.stopped_with_vars() && r.line() == 1 && (r.var("Locals/n").is_some() || r.var("Globals/n").is_some()));
        assert_eq!(r.var("Locals/n").or_else(|| r.var("Globals/n")).as_deref(), Some("21"));
        r.d.continue_();
        r.until("the end", |r| !r.d.is_live());
        let end = Instant::now() + Duration::from_secs(5);
        while !shown.lock().unwrap().contains("dobro 42") && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(shown.lock().unwrap().contains("dobro 42"), "{:?}", shown.lock().unwrap());
    }

    #[test]
    fn conditions_and_disabled_breakpoints() {
        let Some((src, plan)) = c_program() else { return };
        let dir = plan.root.clone();
        let looped = dir.join("laco.c");
        std::fs::write(&looped, "int main(void) {\n    int s = 0;\n    for (int i = 0; i < 9; i++) {\n        s += i;\n    }\n    return s;\n}\n").unwrap();
        let exe = dir.join("laco");
        assert!(compile(&looped, &exe));
        let mut plan = plan;
        plan.config.program = exe.to_string_lossy().into_owned();
        let mut off = Breakpoint::at(1);
        off.enabled = false;
        let mut cond = Breakpoint::at(3);
        cond.condition = Some("i == 5".into());
        let mut bps = Breakpoints::new();
        bps.insert(looped.clone(), vec![off, cond]);
        let _ = src;
        let mut r = Rig::start(&plan, bps);
        r.until("condition", Rig::stopped_with_vars);
        assert_eq!(r.line(), 3, "line 1 is disabled");
        assert_eq!(r.var("Locals/i").as_deref(), Some("5"));
        r.d.stop();
    }

    #[test]
    fn a_session_with_debugpy() {
        if resolve_adapter("python").is_err() {
            eprintln!("skipped: no debugpy");
            return;
        }
        let dir = tempfile();
        let src = dir.join("m.py");
        std::fs::write(&src, "def dobro(n):\n    r = n * 2\n    return r\n\nitens = {'a': [1, 2]}\ntotal = dobro(21)\nprint('total', total)\nraise SystemExit(3)\n").unwrap();
        let config = LaunchConfig { name: "m".into(), kind: "python".into(), program: src.to_string_lossy().into_owned(), cwd: dir.to_string_lossy().into_owned(), ..LaunchConfig::default() };
        let plan = Plan { root: dir, config, program_from: ProgramFrom::Known };
        let mut bps = Breakpoints::new();
        bps.insert(src.clone(), vec![Breakpoint::at(1), Breakpoint::at(6)]);
        let mut r = Rig::start(&plan, bps);
        r.until("first stop", Rig::stopped_with_vars);
        assert_eq!(r.line(), 1);
        assert_eq!(r.d.frames[0].name, "dobro");
        assert!(r.bps[&src].iter().all(|b| b.state == BpState::Verified), "{:?}", r.bps);
        r.until("locals", |r| r.var("Locals/n").is_some());
        assert_eq!(r.var("Locals/n").as_deref(), Some("21"));
        // Python's `repl` is Python, not debugger commands.
        r.d.console_input("n + 1");
        r.until("evaluate", |r| r.notices.iter().any(|n| matches!(n, Notice::Output { text, .. } if text == "22\n")));
        r.d.step_over();
        r.until("step", |r| r.stopped_with_vars() && r.line() == 2 && r.var("Locals/r").is_some());
        assert_eq!(r.var("Locals/r").as_deref(), Some("42"));
        assert!(r.d.variable_rows().into_iter().any(|v| v.name == "Globals" && !v.expanded), "globals start closed");
        r.d.continue_();
        r.until("second breakpoint", |r| r.stopped_with_vars() && r.line() == 6);
        // Nested values open level by level, with Python's own globals.
        let globals = r.d.variable_rows().into_iter().find(|v| v.name == "Globals").unwrap();
        r.d.toggle_row(&globals);
        r.until("globals", |r| r.var("Globals/itens").is_some());
        let itens = r.d.variable_rows().into_iter().find(|v| v.path == "Globals/itens").unwrap();
        assert!(itens.expandable);
        r.d.toggle_row(&itens);
        r.until("dict", |r| r.d.variable_rows().iter().any(|v| v.path.starts_with("Globals/itens/") && v.value.contains('1')));
        r.d.continue_();
        r.until("the end", |r| !r.d.is_live());
        let output: String = r.notices.iter().filter_map(|n| if let Notice::Output { text, .. } = n { Some(text.as_str()) } else { None }).collect();
        assert!(output.contains("total 42"), "{output}");
        assert!(r.notices.contains(&Notice::Ended { code: Some(3) }), "{:?}", r.notices);
    }

    #[test]
    fn a_path_with_spaces_and_quotes_runs() {
        if !gdb_with_dap() {
            return;
        }
        let dir = tempfile().join("minha pasta");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("o'programa novo.c");
        std::fs::write(&src, PROGRAM).unwrap();
        let exe = dir.join("o'programa novo");
        assert!(compile(&src, &exe));
        let mut bps = Breakpoints::new();
        bps.insert(src, vec![Breakpoint::at(5)]);
        let mut r = Rig::start(&plan_for(&exe, &dir), bps);
        r.d.start_limit = Duration::from_secs(10);
        r.until("stop", Rig::stopped_with_vars);
        assert_eq!(r.line(), 5);
        r.d.stop();
    }

    /// The race that older gdb loses now and then: a program too short to give it a second chance.
    #[test]
    fn breakpoints_are_in_place_before_the_program_runs() {
        let Some((src, plan)) = c_program() else { return };
        for _ in 0..12 {
            let mut bps = Breakpoints::new();
            bps.insert(src.clone(), vec![Breakpoint::at(5)]);
            let mut r = Rig::start(&plan, bps);
            r.until("stop", Rig::stopped_with_vars);
            assert_eq!(r.line(), 5);
            r.d.stop();
        }
    }

    #[test]
    fn escaping_for_the_file_command() {
        assert_eq!(escape_for_file_command("/a/b"), "/a/b");
        assert_eq!(escape_for_file_command("/a b/o'c"), r"/a\ b/o\'c");
    }

    #[test]
    fn stop_on_entry_stops_in_main() {
        let Some((_, mut plan)) = c_program() else { return };
        plan.config.stop_on_entry = true;
        let mut r = Rig::start(&plan, Breakpoints::new());
        r.until("entry", Rig::stopped_with_vars);
        assert_eq!(r.d.frames[0].name, "main");
        r.d.stop();
    }

    fn plan_for(program: &Path, dir: &Path) -> Plan {
        let config = LaunchConfig { name: "x".into(), kind: "gdb".into(), program: program.to_string_lossy().into_owned(), cwd: dir.to_string_lossy().into_owned(), ..LaunchConfig::default() };
        Plan { root: dir.to_path_buf(), config, program_from: ProgramFrom::Known }
    }

    #[test]
    fn a_program_that_cannot_run_is_refused_with_a_reason() {
        use std::os::unix::fs::PermissionsExt;
        if !gdb_with_dap() {
            return;
        }
        let dir = tempfile();
        let start = |p: &Path| Debugger::start(&plan_for(p, &dir), &[], Arc::new(|_| {}), None).err().unwrap_or_default();
        let missing = dir.join("nada");
        assert!(start(&missing).contains("program not found") && start(&missing).contains("nada"), "{}", start(&missing));
        // Gdb up to 17 answers "success" to these and then does nothing at all.
        let text = dir.join("texto");
        std::fs::write(&text, "isto nao e um programa\n").unwrap();
        std::fs::set_permissions(&text, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(start(&text).contains("not an executable program"), "{}", start(&text));
        let not_exec = dir.join("sem_permissao");
        std::fs::write(&not_exec, b"\x7fELF resto").unwrap();
        assert!(start(&not_exec).contains("not an executable program"), "{}", start(&not_exec));
    }

    #[test]
    fn a_program_gdb_cannot_load_does_not_hang_forever() {
        use std::os::unix::fs::PermissionsExt;
        if !gdb_with_dap() {
            return;
        }
        // Looks like a program from outside, so only the debugger can tell. Newer versions say so;
        // older ones go quiet, and it takes the time limit to notice.
        let dir = tempfile();
        let fake = dir.join("falso");
        std::fs::write(&fake, b"\x7fELF resto").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut r = Rig::start(&plan_for(&fake, &dir), Breakpoints::new());
        r.d.start_limit = Duration::from_secs(2);
        r.until("failure", |r| r.notices.iter().any(|n| matches!(n, Notice::Failed(_))));
        assert!(!r.d.is_live());
    }

    #[test]
    fn the_time_limit_leaves_a_program_that_started_alone() {
        let Some((src, plan)) = c_program() else { return };
        let mut bps = Breakpoints::new();
        bps.insert(src, vec![Breakpoint::at(5)]);
        let mut r = Rig::start(&plan, bps);
        r.d.start_limit = Duration::from_millis(1);
        r.until("stop", Rig::stopped_with_vars);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(r.d.check_started(), None);
        assert!(r.d.is_live());
        r.d.stop();
    }

    #[test]
    fn stopping_ends_the_program() {
        let Some((src, plan)) = c_program() else { return };
        let mut bps = Breakpoints::new();
        bps.insert(src, vec![Breakpoint::at(5)]);
        let mut r = Rig::start(&plan, bps);
        r.until("stop", Rig::stopped_with_vars);
        // The debuggee is whatever is running the program from that folder. From a Flatpak, the
        // system's pgrep also sees the sandbox's own flatpak-spawn asking, which isn't it.
        let running = || -> Vec<String> {
            let out = crate::host::command("pgrep", ["-af", plan.config.program.as_str()], None, &[]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.contains("pgrep")).map(str::to_string).collect()
        };
        assert!(!running().is_empty());
        r.d.stop();
        drop(r);
        std::thread::sleep(Duration::from_millis(1500));
        assert!(running().is_empty(), "left running: {:?}", running());
    }

    /// A board, emulated: the STM32 template built with the ARM toolchain, run by QEMU as the GDB
    /// server, flashed with `load`, stopped at main, then at a breakpoint in the loop.
    #[test]
    fn a_board_through_a_gdb_server() {
        if !["qemu-system-arm", "arm-none-eabi-gcc", "make"].iter().all(|t| crate::lsp::find_in_path(t)) || embedded_gdb(None).is_err() {
            eprintln!("skipped: needs qemu-system-arm, arm-none-eabi-gcc and a gdb with DAP");
            return;
        }
        let dir = tempfile();
        let t = crate::embedded::templates::by_id("stm32-c").unwrap();
        crate::embedded::templates::create(t, &dir, "pisca").unwrap();
        // QEMU's STM32VLDISCOVERY has an STM32F100, with 8 KiB of RAM.
        let ld = crate::embedded::templates::STM32F103_LD.replace("LENGTH = 20K", "LENGTH = 8K");
        std::fs::write(dir.join("stm32f103c8.ld"), ld).unwrap();
        let built = Command::new("make").current_dir(&dir).output().unwrap();
        assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
        let src = dir.join("main.c");
        let line = std::fs::read_to_string(&src).unwrap().lines().position(|l| l.contains("blinks++")).unwrap();
        let elf = dir.join("build/pisca.elf");
        let port = free_port();
        let launch = format!(
            r#"[{{ "name": "qemu", "type": "cortex-debug", "request": "launch", "servertype": "external",
                "executable": "${{workspaceFolder}}/build/pisca.elf", "gdbTarget": "localhost:{port}",
                "serverCommand": "qemu-system-arm -M stm32vldiscovery -kernel ${{workspaceFolder}}/build/pisca.elf -S -gdb tcp::{port} -nographic -monitor none -serial null",
                "runToEntryPoint": "main" }}]"#
        );
        let config = parse_configs(&launch).unwrap().remove(0);
        let plan = resolved(&config, &dir, None).unwrap();
        assert_eq!(plan.config.kind, "embedded");
        assert_eq!(Path::new(&plan.config.program), elf);
        let mut bps = Breakpoints::new();
        bps.insert(src.clone(), vec![Breakpoint::at(line)]);
        let mut r = Rig::start(&plan, bps);

        // Reset, flashed, and stopped at main by the temporary breakpoint.
        r.until("main", |r| r.stopped_with_vars() && r.d.frames.first().is_some_and(|f| f.name == "main"));
        let out: String = r.notices.iter().filter_map(|n| if let Notice::Output { text, .. } = n { Some(text.as_str()) } else { None }).collect();
        assert!(out.contains("Loading section .isr_vector"), "load ran: {out}");
        assert!(r.notices.iter().any(|n| matches!(n, Notice::Show { path, .. } if *path == src)));

        // On to the breakpoint in the loop, twice: the variable counts.
        r.d.continue_();
        r.until("loop", |r| r.stopped_with_vars() && r.line() == line);
        assert!(r.bps[&src].iter().all(|b| b.state == BpState::Verified), "{:?}", r.bps);
        assert_eq!(r.var("Locals/blinks").as_deref(), Some("0"));
        r.d.continue_();
        r.until("loop again", |r| r.stopped_with_vars() && r.var("Locals/blinks").as_deref() == Some("1"));

        // The registers of a Cortex-M, and gdb's own commands, reach the board.
        r.d.console_input("-exec info registers pc");
        r.until("registers", |r| r.notices.iter().any(|n| matches!(n, Notice::Output { text, .. } if text.contains("pc ") && text.contains("main"))));

        // Stopping lets go of the board, and the server goes with the session.
        let server = r.d.client.server.clone();
        r.d.stop();
        drop(r);
        std::thread::sleep(Duration::from_millis(1200));
        let gone = server.lock().unwrap().as_mut().is_none_or(|c| c.try_wait().ok().flatten().is_some());
        assert!(gone, "QEMU still running");
    }

    #[test]
    fn a_server_that_never_listens_fails_the_session() {
        let mut c = LaunchConfig { name: "x".into(), kind: "embedded".into(), program: "/x.elf".into(), ..LaunchConfig::default() };
        c.extra.insert("servertype".into(), json!("openocd"));
        assert!(remote_session(&c, "gdb").unwrap_err().contains("configFiles"));
        c.extra.insert("servertype".into(), json!("jlink"));
        assert!(remote_session(&c, "gdb").unwrap_err().contains("device"));
        c.extra.insert("servertype".into(), json!("external"));
        c.extra.insert("gdbTarget".into(), json!("localhost:1"));
        c.extra.insert("serverCommand".into(), json!("echo bye; exit 3"));
        c.extra.insert("load".into(), json!(false));
        let (server, attach, setup) = remote_session(&c, "gdb").unwrap();
        assert_eq!(attach["target"], "localhost:1");
        assert!(setup.commands.iter().all(|c| c != "load"));
        let (tx, rx) = channel();
        let tx = Mutex::new(tx);
        let sink: Sink = Arc::new(move |m| {
            let _ = tx.lock().unwrap().send(m);
        });
        let slot = Arc::new(Mutex::new(None));
        let err = start_server(&server.unwrap(), &sink, &slot).unwrap_err();
        assert!(err.contains("ended before the debugger could connect"), "{err}");
        let said: Vec<Incoming> = rx.try_iter().collect();
        assert!(said.iter().any(|m| matches!(m, Incoming::Server(t) if t == "bye")), "{said:?}");
    }

    #[test]
    fn openocd_and_friends_are_started_as_cortex_debug_would() {
        if !crate::lsp::find_in_path("sh") {
            return;
        }
        let mut c = LaunchConfig { name: "x".into(), kind: "embedded".into(), program: "/fw.elf".into(), cwd: "/".into(), ..LaunchConfig::default() };
        c.extra.insert("servertype".into(), json!("openocd"));
        c.extra.insert("configFiles".into(), json!(["interface/stlink.cfg", "target/stm32f1x.cfg"]));
        c.extra.insert("serverpath".into(), json!("sh"));
        c.extra.insert("runToEntryPoint".into(), json!("main"));
        let (server, attach, setup) = remote_session(&c, "gdb").unwrap();
        let s = server.unwrap();
        assert_eq!(s.argv[0], "sh");
        assert!(s.argv.windows(2).any(|w| w == ["-f", "target/stm32f1x.cfg"]));
        assert_eq!(attach["target"], format!("localhost:{}", s.port.unwrap()));
        let cmds: Vec<&str> = setup.commands.iter().map(String::as_str).collect();
        let at = |x: &str| cmds.iter().position(|c| *c == x).unwrap_or_else(|| panic!("{x} in {cmds:?}"));
        assert!(at("monitor reset halt") < at("load") && at("load") < at("tbreak main"));
        // Attaching leaves the board as it is: no reset, no flashing.
        c.extra.insert("request".into(), json!("attach"));
        let (_, _, setup) = remote_session(&c, "gdb").unwrap();
        assert!(setup.stop_on_entry && !setup.commands.iter().any(|c| c == "load" || c.starts_with("tbreak")));
    }

    #[test]
    fn probe_rs_gets_its_own_launch_arguments() {
        let c = LaunchConfig { name: "fw".into(), kind: "probe-rs".into(), program: "/t/fw".into(), cwd: "/t".into(), stop_on_entry: true, extra: [("chip".to_string(), json!("RP2040"))].into_iter().collect(), ..LaunchConfig::default() };
        let a = launch_arguments(&c);
        assert_eq!(a["type"], "probe-rs-debug");
        assert_eq!(a["chip"], "RP2040");
        assert_eq!(a["coreConfigs"][0]["programBinary"], "/t/fw");
        assert_eq!(a["flashingConfig"]["haltAfterReset"], true);
        // VS Code's own probe-rs configuration names the program in its first core.
        let vscode = r#"{ "configurations": [{ "type": "probe-rs-debug", "name": "p", "chip": "STM32F411CEUx",
            "coreConfigs": [{ "programBinary": "${workspaceFolder}/target/thumbv7em-none-eabihf/debug/app" }] }] }"#;
        let c = parse_configs(vscode).unwrap().remove(0);
        let plan = resolved(&c, Path::new("/w"), None).unwrap();
        assert_eq!(plan.config.kind, "probe-rs");
        assert_eq!(plan.config.program, "/w/target/thumbv7em-none-eabihf/debug/app");
        assert_eq!(launch_arguments(&plan.config)["coreConfigs"][0]["programBinary"], "/w/target/thumbv7em-none-eabihf/debug/app");
    }
}
