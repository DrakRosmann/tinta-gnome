//! Running a file without the debugger. The command comes from the first of these that knows
//! the file: the user's `runCommands`, the project's `.tinta/run.json`, the project around the
//! file (Cargo, a Go module, a virtual environment…), its shebang, or the table of languages
//! below. Also the REPLs a selection can be sent to.

use crate::lsp::InstallHint;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const RUN_PATH: &str = ".tinta/run.json";

pub const RUN_TEMPLATE: &str = r#"// How Ctrl+F5 runs files in this project, by language name or by extension.
// A string is the command; an object can build first, and run somewhere else.
// The variables come already quoted: ${file}, ${fileDirname}, ${fileBasename},
// ${fileBasenameNoExtension}, ${workspaceFolder}, ${out} (a program to build) and ${outDir}.
// The arguments from "Run File with Arguments…" go at the end, or where ${args} is.
{
  // "Python": "python3 -X dev ${file}",
  // ".c": { "build": "cc -O2 -Wall -o ${out} ${file} -lm", "run": "${out}" },
  // "Rust": { "run": "cargo run -q --release -- ${args}", "cwd": "${workspaceFolder}" }
}
"#;

/// A command the user or the project gave, in `runCommands` or `.tinta/run.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserRunner {
    Command(String),
    Steps {
        #[serde(default)]
        build: Option<String>,
        run: String,
        #[serde(default)]
        cwd: Option<String>,
    },
}

pub type UserRunners = HashMap<String, UserRunner>;

/// The project's `.tinta/run.json`, if it has one.
pub fn load_project(root: &Path) -> Result<Option<UserRunners>, String> {
    let Ok(src) = std::fs::read_to_string(root.join(RUN_PATH)) else { return Ok(None) };
    serde_json::from_str(&crate::settings::strip_jsonc(&src)).map(Some).map_err(|e| format!("{RUN_PATH}: {e}"))
}

/// What running the file means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Shell commands: an optional build, whose errors go to the code, then the program.
    Shell { build: Option<String>, run: String },
    /// A page for the browser.
    Open(PathBuf),
    /// Markdown: its preview.
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Settings,
    Project,
    Shebang,
    /// The project around the file: Cargo, a Go module, a virtual environment, a Makefile…
    Environment,
    Builtin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlan {
    /// The file's name, for the terminal's title.
    pub title: String,
    pub cwd: PathBuf,
    pub action: Action,
    pub source: Source,
    /// After the build, the program is the newest one it made here (make, cmake), and the run
    /// command is only what goes after it.
    pub newest_in: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// Nothing here knows how to run this language.
    Unknown(String),
    /// The language is known, but none of the programs that run it is installed.
    Missing { language: String, tool: String, hint: InstallHint },
    Config(String),
}

/// What to run, and where its pieces are.
pub struct Request<'a> {
    pub file: &'a Path,
    /// The grammar's name, as the editor shows it.
    pub language: &'a str,
    pub root: &'a Path,
    pub user: &'a UserRunners,
    /// The project's `.tinta/run.json`, if it may be used.
    pub project: Option<&'a UserRunners>,
    /// Shell words, as the user typed them.
    pub args: &'a str,
    /// Where programs built to run go.
    pub out_dir: &'a Path,
    /// The text's first line, for a shebang.
    pub first_line: &'a str,
}

/// One way to run a language: the program it needs, a build, and the run.
struct Way {
    tool: &'static str,
    build: Option<&'static str>,
    run: &'static str,
}

const fn way(tool: &'static str, run: &'static str) -> Way {
    Way { tool, build: None, run }
}

const fn built(tool: &'static str, build: &'static str, run: &'static str) -> Way {
    Way { tool, build: Some(build), run }
}

struct Runner {
    /// Extensions, lowercase and without the dot. They decide first.
    exts: &'static [&'static str],
    /// Grammar names, for files whose extension says nothing, and for untitled text.
    langs: &'static [&'static str],
    /// Tried in order: the first whose program is installed wins.
    ways: &'static [Way],
}

const fn runner(exts: &'static [&'static str], langs: &'static [&'static str], ways: &'static [Way]) -> Runner {
    Runner { exts, langs, ways }
}

/// A compiled program run from where the build put it.
const OUT: &str = "${out}";

const RUNNERS: &[Runner] = &[
    runner(&["py", "pyw", "py3"], &["Python"], &[way("python3", "python3 ${file}"), way("python", "python ${file}"), way("pypy3", "pypy3 ${file}")]),
    runner(
        &["js", "mjs", "cjs"],
        &["JavaScript", "JavaScript (Babel)", "JSX"],
        &[way("node", "node ${file}"), way("bun", "bun run ${file}"), way("deno", "deno run -A ${file}"), way("qjs", "qjs ${file}")],
    ),
    runner(
        &["ts", "mts", "cts", "tsx"],
        &["TypeScript", "TypeScriptReact"],
        &[
            way("tsx", "tsx ${file}"),
            way("bun", "bun run ${file}"),
            way("deno", "deno run -A ${file}"),
            way("ts-node", "ts-node ${file}"),
            // Node strips the types on its own since version 23.6.
            way("node", "node ${file}"),
        ],
    ),
    runner(&["coffee"], &["CoffeeScript"], &[way("coffee", "coffee ${file}")]),
    runner(&["rb"], &["Ruby"], &[way("ruby", "ruby ${file}")]),
    runner(&["pl", "pm", "t"], &["Perl"], &[way("perl", "perl ${file}")]),
    runner(&["raku", "rakumod", "p6"], &["Raku"], &[way("raku", "raku ${file}")]),
    runner(&["php"], &["PHP"], &[way("php", "php ${file}")]),
    runner(&["lua"], &["Lua"], &[way("lua", "lua ${file}"), way("luajit", "luajit ${file}"), way("lua5.4", "lua5.4 ${file}")]),
    runner(&["fnl"], &["Fennel"], &[way("fennel", "fennel ${file}")]),
    runner(&["r"], &["R"], &[way("Rscript", "Rscript ${file}")]),
    runner(&["jl"], &["Julia"], &[way("julia", "julia ${file}")]),
    runner(&["sh", "bash"], &["Bourne Again Shell (bash)", "Shell-Unix-Generic"], &[way("bash", "bash ${file}"), way("sh", "sh ${file}")]),
    runner(&["zsh"], &[], &[way("zsh", "zsh ${file}")]),
    runner(&["fish"], &["Fish", "fish"], &[way("fish", "fish ${file}")]),
    runner(&["ps1"], &["PowerShell"], &[way("pwsh", "pwsh -NoProfile -File ${file}")]),
    runner(&["tcl"], &["Tcl"], &[way("tclsh", "tclsh ${file}")]),
    runner(&["awk"], &["AWK"], &[way("awk", "awk -f ${file}")]),
    runner(&["ex", "exs"], &["Elixir"], &[way("elixir", "elixir ${file}")]),
    runner(
        &["erl"],
        &["Erlang"],
        &[built("erlc", "erlc -o ${outDir} ${file}", "erl -noshell -pa ${outDir} -s ${fileBasenameNoExtension} main -s init stop")],
    ),
    runner(&["escript"], &[], &[way("escript", "escript ${file}")]),
    runner(&["hs"], &["Haskell"], &[built("ghc", "ghc -O0 -outputdir ${outDir} -o ${out} ${file}", OUT), way("runghc", "runghc ${file}"), way("stack", "stack runghc ${file}")]),
    runner(&["ml"], &["OCaml"], &[way("ocaml", "ocaml ${file}")]),
    runner(&["fsx", "fs"], &["F#"], &[way("dotnet", "dotnet fsi ${file}")]),
    runner(&["cs", "csx"], &["C#"], &[way("dotnet", "dotnet run ${file}"), built("mcs", "mcs -out:${out}.exe ${file}", "mono ${out}.exe")]),
    runner(&["java"], &["Java"], &[way("java", "java ${file}")]),
    runner(&["kts"], &[], &[way("kotlin", "kotlin ${file}"), way("kotlinc", "kotlinc -script ${file}")]),
    runner(&["kt"], &["Kotlin"], &[built("kotlinc", "kotlinc ${file} -include-runtime -d ${out}.jar", "java -jar ${out}.jar")]),
    runner(&["scala", "sc"], &["Scala"], &[way("scala-cli", "scala-cli run ${file}"), way("scala", "scala run ${file}")]),
    runner(&["groovy", "gvy"], &["Groovy"], &[way("groovy", "groovy ${file}")]),
    runner(&["clj", "cljc", "bb"], &["Clojure"], &[way("bb", "bb ${file}"), way("clojure", "clojure -M ${file}"), way("clj", "clj -M ${file}")]),
    runner(&["swift"], &["Swift"], &[way("swift", "swift ${file}")]),
    runner(&["dart"], &["Dart"], &[way("dart", "dart run ${file}")]),
    runner(&["go"], &["Go"], &[built("go", "go build -o ${out} ${file}", OUT)]),
    runner(&["rs"], &["Rust"], &[built("rustc", "rustc --edition 2024 -g -o ${out} ${file}", OUT)]),
    runner(
        &["c"],
        &["C"],
        &[
            built("cc", "cc -g -Wall -o ${out} ${file} -lm", OUT),
            built("gcc", "gcc -g -Wall -o ${out} ${file} -lm", OUT),
            built("clang", "clang -g -Wall -o ${out} ${file} -lm", OUT),
        ],
    ),
    runner(
        &["cpp", "cc", "cxx", "c++", "cp"],
        &["C++"],
        &[
            built("c++", "c++ -std=c++20 -g -Wall -o ${out} ${file}", OUT),
            built("g++", "g++ -std=c++20 -g -Wall -o ${out} ${file}", OUT),
            built("clang++", "clang++ -std=c++20 -g -Wall -o ${out} ${file}", OUT),
        ],
    ),
    runner(&["m"], &["Objective-C"], &[built("clang", "clang -g -o ${out} ${file} $(gnustep-config --objc-flags --base-libs 2>/dev/null)", OUT)]),
    runner(&["zig"], &["Zig"], &[built("zig", "zig build-exe ${file} -femit-bin=${out}", OUT)]),
    runner(&["nim", "nims"], &["Nim"], &[built("nim", "nim c --hints:off --nimcache:${outDir} -o:${out} ${file}", OUT)]),
    runner(&["cr"], &["Crystal"], &[built("crystal", "crystal build -o ${out} ${file}", OUT)]),
    runner(&["odin"], &["Odin"], &[built("odin", "odin build ${file} -file -out:${out}", OUT)]),
    runner(&["mojo"], &[], &[way("mojo", "mojo run ${file}")]),
    runner(
        &["d"],
        &["D"],
        &[
            built("dmd", "dmd -g -of=${out} ${file}", OUT),
            built("ldc2", "ldc2 -g -of=${out} ${file}", OUT),
            built("gdc", "gdc -g -o ${out} ${file}", OUT),
            way("rdmd", "rdmd ${file}"),
        ],
    ),
    runner(
        &["f90", "f95", "f03", "f08", "f", "for", "f77"],
        &["Fortran (Modern)", "Fortran (Fixed Form)"],
        &[built("gfortran", "gfortran -g -o ${out} ${file}", OUT), built("flang", "flang -g -o ${out} ${file}", OUT)],
    ),
    runner(&["pas", "pp", "dpr"], &["Pascal"], &[built("fpc", "fpc -v0 -FU${outDir} -o${out} ${file}", OUT)]),
    runner(&["adb"], &["Ada"], &[built("gnatmake", "gnatmake -q -D ${outDir} ${file} -o ${out}", OUT)]),
    runner(&["cob", "cbl", "cobol"], &["COBOL"], &[built("cobc", "cobc -x -o ${out} ${file}", OUT)]),
    runner(&["asm", "nasm"], &["x86_64 Assembly"], &[built("nasm", "nasm -f elf64 -g -o ${out}.o ${file} && ld -o ${out} ${out}.o", OUT)]),
    runner(&["s"], &["ARM Assembly"], &[built("cc", "cc -g -o ${out} ${file}", OUT)]),
    runner(&["v", "sv"], &["Verilog", "SystemVerilog"], &[built("iverilog", "iverilog -g2012 -o ${out} ${file}", "vvp ${out}")]),
    runner(&["pro", "prolog"], &["Prolog"], &[way("swipl", "swipl -q ${file}")]),
    runner(&["rkt"], &["Racket"], &[way("racket", "racket ${file}")]),
    runner(&["scm", "ss", "sld"], &[], &[way("guile", "guile ${file}"), way("chez", "chez --script ${file}"), way("scheme", "scheme --script ${file}"), way("csi", "csi -s ${file}")]),
    runner(&["lisp", "cl", "lsp"], &["Lisp"], &[way("sbcl", "sbcl --script ${file}"), way("clisp", "clisp ${file}"), way("ecl", "ecl --shell ${file}")]),
    runner(&["el"], &[], &[way("emacs", "emacs --script ${file}")]),
    runner(&["janet"], &[], &[way("janet", "janet ${file}")]),
    runner(&["st"], &[], &[way("gst", "gst ${file}")]),
    runner(&["hx"], &[], &[way("haxe", "haxe --main ${fileBasenameNoExtension} --interp")]),
    runner(&["matlab"], &["MATLAB"], &[way("octave", "octave --no-gui -q ${file}")]),
    runner(&["sql"], &["SQL"], &[way("sqlite3", "sqlite3 -header -column :memory: < ${file}")]),
    runner(&["nix"], &["Nix"], &[way("nix-instantiate", "nix-instantiate --eval --strict ${file}")]),
    runner(&["gp", "gpl", "gnuplot", "plt"], &["gnuplot"], &[way("gnuplot", "gnuplot -p ${file}")]),
    runner(&["typ"], &["Typst"], &[way("typst", "typst compile ${file}")]),
    runner(
        &["tex", "ltx"],
        &["LaTeX"],
        &[
            way("tectonic", "tectonic ${file}"),
            way("latexmk", "latexmk -pdf -interaction=nonstopmode ${file}"),
            way("pdflatex", "pdflatex -interaction=nonstopmode ${file}"),
        ],
    ),
    runner(&["mk", "make"], &["Makefile"], &[way("make", "make -f ${file}")]),
    runner(&["just"], &[], &[way("just", "just --justfile ${file}")]),
    runner(&["dockerfile"], &["Dockerfile"], &[way("docker", "docker build -f ${file} ${fileDirname}"), way("podman", "podman build -f ${file} ${fileDirname}")]),
    runner(&["bat", "cmd"], &["Batch File"], &[way("wine", "wine cmd /c ${file}")]),
];

/// Extensions, grammar names, and the REPLs tried in order: the program and its command.
type ReplRow = (&'static [&'static str], &'static [&'static str], &'static [(&'static str, &'static str)]);

/// The languages with a REPL a selection can go to. Python gets its own handling of blank lines.
const REPLS: &[ReplRow] = &[
    (&["py", "pyw", "py3"], &["Python"], &[("ipython", "ipython --no-autoindent"), ("python3", "python3 -q"), ("python", "python -q")]),
    (&["js", "mjs", "cjs"], &["JavaScript", "JavaScript (Babel)", "JSX"], &[("node", "node"), ("bun", "bun repl"), ("deno", "deno repl")]),
    (&["ts", "mts", "cts", "tsx"], &["TypeScript", "TypeScriptReact"], &[("deno", "deno repl"), ("ts-node", "ts-node"), ("bun", "bun repl")]),
    (&["rb"], &["Ruby"], &[("irb", "irb"), ("pry", "pry")]),
    (&["lua"], &["Lua"], &[("lua", "lua"), ("luajit", "luajit"), ("lua5.4", "lua5.4")]),
    (&["r"], &["R"], &[("R", "R --quiet --no-save")]),
    (&["jl"], &["Julia"], &[("julia", "julia")]),
    (&["hs"], &["Haskell"], &[("ghci", "ghci"), ("stack", "stack ghci")]),
    (&["ml"], &["OCaml"], &[("utop", "utop"), ("ocaml", "ocaml")]),
    (&["ex", "exs"], &["Elixir"], &[("iex", "iex")]),
    (&["erl"], &["Erlang"], &[("erl", "erl")]),
    (&["clj", "cljc", "bb"], &["Clojure"], &[("bb", "bb"), ("clj", "clj"), ("clojure", "clojure")]),
    (&["scm", "ss", "sld"], &[], &[("guile", "guile"), ("chez", "chez"), ("csi", "csi")]),
    (&["rkt"], &["Racket"], &[("racket", "racket")]),
    (&["lisp", "cl", "lsp"], &["Lisp"], &[("sbcl", "sbcl"), ("clisp", "clisp")]),
    (&["php"], &["PHP"], &[("psysh", "psysh"), ("php", "php -a")]),
    (&["pl", "pm"], &["Perl"], &[("re.pl", "re.pl"), ("perl", "perl -de0")]),
    (&["raku", "rakumod", "p6"], &["Raku"], &[("raku", "raku")]),
    (&["scala", "sc"], &["Scala"], &[("scala-cli", "scala-cli repl"), ("scala", "scala")]),
    (&["groovy", "gvy"], &["Groovy"], &[("groovysh", "groovysh")]),
    (&["java"], &["Java"], &[("jshell", "jshell")]),
    (&["kt", "kts"], &["Kotlin"], &[("ki", "ki")]),
    (&["swift"], &["Swift"], &[("swift", "swift repl")]),
    (&["fsx", "fs"], &["F#"], &[("dotnet", "dotnet fsi")]),
    (&["cs", "csx"], &["C#"], &[("csharprepl", "csharprepl")]),
    (&["rs"], &["Rust"], &[("evcxr", "evcxr")]),
    (&["sh", "bash"], &["Bourne Again Shell (bash)", "Shell-Unix-Generic"], &[("bash", "bash --norc")]),
    (&["zsh"], &[], &[("zsh", "zsh -f")]),
    (&["fish"], &["Fish", "fish"], &[("fish", "fish --no-config")]),
    (&["ps1"], &["PowerShell"], &[("pwsh", "pwsh -NoProfile")]),
    (&["tcl"], &["Tcl"], &[("tclsh", "tclsh")]),
    (&["matlab"], &["MATLAB"], &[("octave", "octave --no-gui -q")]),
    (&["sql"], &["SQL"], &[("sqlite3", "sqlite3 -header -column")]),
    (&["nix"], &["Nix"], &[("nix", "nix repl")]),
    (&["pro", "prolog"], &["Prolog"], &[("swipl", "swipl")]),
    (&["janet"], &[], &[("janet", "janet")]),
    (&["fnl"], &["Fennel"], &[("fennel", "fennel")]),
    (&["gp", "gpl", "gnuplot", "plt"], &["gnuplot"], &[("gnuplot", "gnuplot")]),
];

fn ext_of(file: &Path) -> String {
    match file.file_name().and_then(|n| n.to_str()) {
        Some("Dockerfile" | "Containerfile") => "dockerfile".into(),
        Some("Makefile" | "makefile" | "GNUmakefile") => "make".into(),
        Some("justfile" | "Justfile") => "just".into(),
        _ => file.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase(),
    }
}

/// The table's entry for a file: by its extension, or else by its language.
fn runner_for(file: &Path, language: &str) -> Option<&'static Runner> {
    let ext = ext_of(file);
    (!ext.is_empty())
        .then(|| RUNNERS.iter().find(|r| r.exts.contains(&ext.as_str())))
        .flatten()
        .or_else(|| RUNNERS.iter().find(|r| r.langs.iter().any(|l| l.eq_ignore_ascii_case(language))))
}

/// The extension an untitled text in this language gets, to be saved and run.
pub fn extension_for(language: &str) -> Option<&'static str> {
    if language.eq_ignore_ascii_case("Markdown") {
        return Some("md");
    }
    if language.eq_ignore_ascii_case("HTML") {
        return Some("html");
    }
    RUNNERS.iter().find(|r| r.langs.iter().any(|l| l.eq_ignore_ascii_case(language))).and_then(|r| r.exts.first().copied())
}

/// Whether running this file is something the editor knows, short of the user's own commands.
pub fn knows(file: &Path, language: &str) -> bool {
    runner_for(file, language).is_some() || matches!(ext_of(file).as_str(), "html" | "htm" | "xhtml" | "md" | "markdown")
}

pub fn shell_quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+=:@%,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Where a program built from this file goes.
pub fn out_path(out_dir: &Path, file: &Path) -> PathBuf {
    let stem: String = file.file_stem().unwrap_or_default().to_string_lossy().chars().map(|c| if c.is_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    out_dir.join(if stem.is_empty() { "program".into() } else { stem })
}

/// Fills the variables in, each one quoted for the shell, and puts the arguments in.
fn substitute(template: &str, req: &Request, args: Option<&str>) -> String {
    let f = req.file;
    let dir = f.parent().unwrap_or(req.root);
    let q = |p: &Path| shell_quote(&p.to_string_lossy());
    let mut out = template
        .replace("${fileBasenameNoExtension}", &shell_quote(&f.file_stem().unwrap_or_default().to_string_lossy()))
        .replace("${fileBasename}", &shell_quote(&f.file_name().unwrap_or_default().to_string_lossy()))
        .replace("${fileDirname}", &q(dir))
        .replace("${file}", &q(f))
        .replace("${workspaceFolder}", &q(req.root))
        .replace("${outDir}", &q(req.out_dir))
        .replace("${out}", &q(&out_path(req.out_dir, f)));
    if let Some(args) = args {
        let args = args.trim();
        if out.contains("${args}") {
            out = out.replace("${args}", args);
            // `cargo run -- ` with nothing after it is fine, but reads oddly.
            out = out.trim_end().trim_end_matches(" --").to_string();
        } else if !args.is_empty() {
            out = format!("{out} {args}");
        }
    }
    out
}

fn title_of(file: &Path) -> String {
    file.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

fn user_entry<'a>(map: &'a UserRunners, file: &Path, language: &str) -> Option<&'a UserRunner> {
    let ext = ext_of(file);
    let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
    // The file's own name, then its extension, then its language.
    let by_name = map.iter().find(|(k, _)| k.as_str() == name);
    let by_ext = || map.iter().find(|(k, _)| k.strip_prefix('.').is_some_and(|e| !ext.is_empty() && e.eq_ignore_ascii_case(&ext)));
    let by_language = || map.iter().find(|(k, _)| !k.starts_with('.') && k.eq_ignore_ascii_case(language));
    by_name.or_else(by_ext).or_else(by_language).map(|(_, v)| v)
}

fn from_user(entry: &UserRunner, req: &Request, source: Source) -> RunPlan {
    let dir = req.file.parent().unwrap_or(req.root).to_path_buf();
    let (build, run, cwd) = match entry {
        UserRunner::Command(c) => (None, c.as_str(), None),
        UserRunner::Steps { build, run, cwd } => (build.as_deref(), run.as_str(), cwd.as_deref()),
    };
    let cwd = match cwd {
        // The folder is a path, not a shell word: no quotes.
        Some(c) => {
            let c = c
                .replace("${workspaceFolder}", &req.root.to_string_lossy())
                .replace("${fileDirname}", &dir.to_string_lossy());
            let p = PathBuf::from(c);
            if p.is_relative() { req.root.join(p) } else { p }
        }
        None => dir,
    };
    RunPlan {
        title: title_of(req.file),
        cwd,
        action: Action::Shell { build: build.map(|b| substitute(b, req, None)), run: substitute(run, req, Some(req.args)) },
        source,
        newest_in: None,
    }
}

/// `#!/usr/bin/env python3` and the like: the command, and the program it needs.
pub fn shebang(first_line: &str) -> Option<(String, String)> {
    let rest = first_line.strip_prefix("#!")?;
    // `#![allow(…)]` is Rust, not a shebang.
    if rest.starts_with('[') {
        return None;
    }
    let rest = rest.trim();
    let mut words = rest.split_whitespace();
    let first = words.next()?;
    let tool = if Path::new(first).file_name().is_some_and(|n| n == "env") { words.find(|w| !w.starts_with('-'))? } else { first };
    Some((rest.to_string(), tool.to_string()))
}

fn ancestors_until<'p>(start: &'p Path, root: &Path) -> impl Iterator<Item = &'p Path> {
    let inside = start.starts_with(root);
    let root = root.to_path_buf();
    let mut done = false;
    start.ancestors().take_while(move |d| {
        if done {
            return false;
        }
        // Inside the project, up to its root; outside it, just the file's own folder.
        done = !inside || *d == root;
        true
    })
}

fn nearest(file: &Path, root: &Path, marker: &str) -> Option<PathBuf> {
    let dir = file.parent()?;
    ancestors_until(dir, root).find(|d| d.join(marker).exists()).map(Path::to_path_buf)
}

/// A Cargo target: `--bin x`, `--example x`, `--test x` from where the file is.
fn cargo_target(manifest_dir: &Path, file: &Path) -> (String, &'static str) {
    let rel = file.strip_prefix(manifest_dir).unwrap_or(file);
    let parts: Vec<String> = rel.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    let stem = |s: &str| s.strip_suffix(".rs").unwrap_or(s).to_string();
    let named = |i: usize| -> Option<String> {
        match parts.len().checked_sub(i)? {
            1 => Some(stem(&parts[i])),
            2 if parts[i + 1] == "main.rs" => Some(parts[i].clone()),
            _ => None,
        }
    };
    match parts.first().map(String::as_str) {
        Some("src") if parts.get(1).map(String::as_str) == Some("bin") => named(2).map(|n| (format!("--bin {}", shell_quote(&n)), "run")).unwrap_or_default(),
        Some("examples") => named(1).map(|n| (format!("--example {}", shell_quote(&n)), "run")).unwrap_or_default(),
        Some("tests") => named(1).map(|n| (format!("--test {}", shell_quote(&n)), "test")).unwrap_or_default(),
        Some("benches") => named(1).map(|n| (format!("--bench {}", shell_quote(&n)), "bench")).unwrap_or_default(),
        _ => (String::new(), "run"),
    }
}

fn join(a: &str, b: &str) -> String {
    if b.is_empty() { a.to_string() } else { format!("{a} {b}") }
}

/// The project around the file, when that says more than the file alone.
fn from_environment(req: &Request, runner: Option<&Runner>, find: &dyn Fn(&str) -> bool) -> Option<RunPlan> {
    let file = req.file;
    let dir = file.parent().unwrap_or(req.root);
    let lang = |l: &str| runner.is_some_and(|r| r.langs.contains(&l));
    let plan = |cwd: &Path, build: Option<String>, run: String| RunPlan {
        title: title_of(file),
        cwd: cwd.to_path_buf(),
        action: Action::Shell { build, run },
        source: Source::Environment,
        newest_in: None,
    };
    let args = req.args.trim();
    if lang("Rust")
        && let Some(m) = nearest(file, req.root, "Cargo.toml")
        && find("cargo")
    {
        let (target, verb) = cargo_target(&m, file);
        let after = if args.is_empty() { String::new() } else { format!(" -- {args}") };
        let (build, run) = match verb {
            "test" => (join("cargo test --no-run", &target), format!("{}{after}", join("cargo test -q", &target))),
            "bench" => (join("cargo bench --no-run", &target), format!("{}{after}", join("cargo bench -q", &target))),
            _ => (join("cargo build", &target), format!("{}{after}", join("cargo run -q", &target))),
        };
        return Some(plan(&m, Some(build), run));
    }
    if lang("Go")
        && nearest(file, req.root, "go.mod").is_some()
        && find("go")
    {
        if file.to_string_lossy().ends_with("_test.go") {
            return Some(plan(dir, None, join("go test -v .", args)));
        }
        let out = shell_quote(&out_path(req.out_dir, file).to_string_lossy());
        return Some(plan(dir, Some(format!("go build -o {out} .")), join(&out, args)));
    }
    if lang("Python") {
        for d in ancestors_until(dir, req.root) {
            for venv in [".venv", "venv", "env"] {
                let py = d.join(venv).join("bin/python");
                if py.is_file() {
                    let run = format!("{} {}", shell_quote(&py.to_string_lossy()), shell_quote(&file.to_string_lossy()));
                    return Some(plan(dir, None, join(&run, args)));
                }
            }
        }
        if nearest(file, req.root, "uv.lock").is_some() && find("uv") {
            return Some(plan(dir, None, join(&format!("uv run {}", shell_quote(&file.to_string_lossy())), args)));
        }
        if nearest(file, req.root, "poetry.lock").is_some() && find("poetry") {
            return Some(plan(dir, None, join(&format!("poetry run python {}", shell_quote(&file.to_string_lossy())), args)));
        }
    }
    if lang("JavaScript") || lang("TypeScript") {
        let f = shell_quote(&file.to_string_lossy());
        if (nearest(file, req.root, "deno.json").is_some() || nearest(file, req.root, "deno.jsonc").is_some()) && find("deno") {
            return Some(plan(dir, None, join(&format!("deno run -A {f}"), args)));
        }
        if (nearest(file, req.root, "bun.lock").is_some() || nearest(file, req.root, "bun.lockb").is_some()) && find("bun") {
            return Some(plan(dir, None, join(&format!("bun run {f}"), args)));
        }
    }
    if lang("C") || lang("C++") {
        // A project with its own build: the program is whatever it makes.
        if ["Makefile", "makefile", "GNUmakefile"].iter().any(|m| req.root.join(m).exists()) && find("make") {
            let mut p = plan(req.root, Some("make".into()), args.to_string());
            p.newest_in = Some(req.root.to_path_buf());
            return Some(p);
        }
        if req.root.join("CMakeLists.txt").exists() && find("cmake") {
            let mut p = plan(req.root, Some("cmake -S . -B build && cmake --build build".into()), args.to_string());
            p.newest_in = Some(req.root.join("build"));
            return Some(p);
        }
    }
    None
}

/// What running the file means, or why it can't be run. `find` says whether a program is
/// installed.
pub fn plan(req: &Request, find: &dyn Fn(&str) -> bool) -> Result<RunPlan, RunError> {
    if let Some(e) = user_entry(req.user, req.file, req.language) {
        return Ok(from_user(e, req, Source::Settings));
    }
    if let Some(e) = req.project.and_then(|p| user_entry(p, req.file, req.language)) {
        return Ok(from_user(e, req, Source::Project));
    }
    let ext = ext_of(req.file);
    let title = title_of(req.file);
    let cwd = req.file.parent().unwrap_or(req.root).to_path_buf();
    let simple = |action: Action, source: Source| RunPlan { title: title.clone(), cwd: cwd.clone(), action, source, newest_in: None };
    if matches!(ext.as_str(), "html" | "htm" | "xhtml") {
        return Ok(simple(Action::Open(req.file.to_path_buf()), Source::Builtin));
    }
    if matches!(ext.as_str(), "md" | "markdown") || req.language.eq_ignore_ascii_case("Markdown") {
        return Ok(simple(Action::Preview, Source::Builtin));
    }
    let runner = runner_for(req.file, req.language);
    if let Some(p) = from_environment(req, runner, find) {
        return Ok(p);
    }
    if let Some((command, tool)) = shebang(req.first_line) {
        if !find(&tool) {
            let name = Path::new(&tool).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(tool.clone());
            return Err(RunError::Missing { language: req.language.to_string(), hint: tool_hint(&name), tool: name });
        }
        let run = join(&format!("{command} {}", shell_quote(&req.file.to_string_lossy())), req.args.trim());
        return Ok(simple(Action::Shell { build: None, run }, Source::Shebang));
    }
    let Some(runner) = runner else { return Err(RunError::Unknown(req.language.to_string())) };
    let Some(w) = runner.ways.iter().find(|w| find(w.tool)) else {
        let tool = runner.ways[0].tool.to_string();
        return Err(RunError::Missing { language: req.language.to_string(), hint: tool_hint(&tool), tool });
    };
    let action = Action::Shell { build: w.build.map(|b| substitute(b, req, None)), run: substitute(w.run, req, Some(req.args)) };
    Ok(simple(action, Source::Builtin))
}

/// After a build of a project that names no program (make, cmake): the newest one it made, with
/// the arguments after it.
pub fn finish(mut plan: RunPlan) -> Result<RunPlan, String> {
    let Some(dir) = plan.newest_in.take() else { return Ok(plan) };
    let exe = crate::dap::newest_executable(&dir).ok_or_else(|| format!("no program was found after the build in {}: add {RUN_PATH}", dir.display()))?;
    if let Action::Shell { run, .. } = &mut plan.action {
        *run = join(&shell_quote(&exe.to_string_lossy()), run.trim());
    }
    plan.title = exe.file_name().unwrap_or_default().to_string_lossy().into_owned();
    Ok(plan)
}

// REPLs ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplKind {
    Python,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repl {
    /// The program, which also tells one REPL from another.
    pub tool: String,
    pub command: String,
    pub kind: ReplKind,
}

/// The REPL for a file's language. None when it has none; an error when none is installed.
pub fn repl_for(file: Option<&Path>, language: &str, find: &dyn Fn(&str) -> bool) -> Option<Result<Repl, (String, InstallHint)>> {
    let ext = file.map(ext_of).unwrap_or_default();
    let entry = (!ext.is_empty())
        .then(|| REPLS.iter().find(|(exts, _, _)| exts.contains(&ext.as_str())))
        .flatten()
        .or_else(|| REPLS.iter().find(|(_, langs, _)| langs.iter().any(|l| l.eq_ignore_ascii_case(language))))?;
    let kind = if entry.1.contains(&"Python") { ReplKind::Python } else { ReplKind::Other };
    match entry.2.iter().find(|(tool, _)| find(tool)) {
        Some((tool, command)) => Some(Ok(Repl { tool: tool.to_string(), command: command.to_string(), kind })),
        None => {
            // Suggest the plain one, not the fancy one listed first.
            let tool = entry.2.iter().map(|(t, _)| *t).find(|t| matches!(*t, "python3" | "node" | "deno")).unwrap_or(entry.2[0].0);
            Some(Err((tool.to_string(), tool_hint(tool))))
        }
    }
}

/// The lines to type into a REPL, and how many times to press Enter after them. Indentation
/// common to all lines goes, so a block from inside a function can be sent. For Python, blank
/// lines would end a block halfway, and a block needs one to end before the next statement;
/// the newer REPL (3.13) runs several lines only after an Enter on an empty one, and the older
/// one takes that extra Enter for an empty prompt.
pub fn repl_input(kind: ReplKind, text: &str) -> (String, usize) {
    let lines: Vec<&str> = text.lines().collect();
    let indent = lines.iter().filter(|l| !l.trim().is_empty()).map(|l| l.len() - l.trim_start().len()).min().unwrap_or(0);
    let cut = |l: &str| -> String { if l.len() >= indent && l.is_char_boundary(indent) { l[indent..].trim_end().to_string() } else { l.trim().to_string() } };
    if kind == ReplKind::Other {
        let out: Vec<String> = lines.iter().map(|l| cut(l)).collect();
        return (out.join("\n").trim_end_matches('\n').to_string(), 1);
    }
    let mut out: Vec<String> = Vec::new();
    let mut prev_indented = false;
    for l in lines.iter().map(|l| cut(l)).filter(|l| !l.trim().is_empty()) {
        let indented = l.starts_with([' ', '\t']);
        let continues = ["else", "elif", "except", "finally", ")", "]", "}"].iter().any(|k| l.starts_with(k));
        if !indented && prev_indented && !continues {
            out.push(String::new());
        }
        prev_indented = indented;
        out.push(l);
    }
    let enters = if out.len() > 1 { 2 } else { 1 };
    (out.join("\n"), enters)
}

// Installing what's missing ------------------------------------------------------------------

/// How to get a program that runs a language, on this system.
pub fn tool_hint(tool: &str) -> InstallHint {
    tool_hint_for(tool, crate::lsp::distro_family(), crate::host::find("rustup"))
}

/// Packages by family: Arch, Fedora, Debian. Only names known to exist are suggested; when the
/// system's packages don't have it, the language's own installer.
pub fn tool_hint_for(tool: &str, family: &str, has_rustup: bool) -> InstallHint {
    let pkgs: (Option<&str>, Option<&str>, Option<&str>) = match tool {
        "python3" | "python" => (Some("python"), Some("python3"), Some("python3")),
        "ipython" => (Some("ipython"), Some("python3-ipython"), Some("ipython3")),
        "node" => (Some("nodejs"), Some("nodejs"), Some("nodejs")),
        "deno" => (Some("deno"), None, None),
        "ruby" | "irb" => (Some("ruby"), Some("ruby"), Some("ruby")),
        "perl" => (Some("perl"), Some("perl"), Some("perl")),
        "raku" => (Some("rakudo"), Some("rakudo"), Some("rakudo")),
        "php" => (Some("php"), Some("php-cli"), Some("php-cli")),
        "lua" | "lua5.4" => (Some("lua"), Some("lua"), Some("lua5.4")),
        "luajit" => (Some("luajit"), Some("luajit"), Some("luajit")),
        "Rscript" | "R" => (Some("r"), Some("R"), Some("r-base")),
        "bash" => (Some("bash"), Some("bash"), Some("bash")),
        "zsh" => (Some("zsh"), Some("zsh"), Some("zsh")),
        "fish" => (Some("fish"), Some("fish"), Some("fish")),
        "tclsh" => (Some("tcl"), Some("tcl"), Some("tcl")),
        "awk" => (Some("gawk"), Some("gawk"), Some("gawk")),
        "elixir" | "iex" => (Some("elixir"), Some("elixir"), Some("elixir")),
        "erlc" | "erl" | "escript" => (Some("erlang"), Some("erlang"), Some("erlang")),
        "ghc" | "ghci" | "runghc" => (Some("ghc"), Some("ghc"), Some("ghc")),
        "ocaml" => (Some("ocaml"), Some("ocaml"), Some("ocaml")),
        "java" | "jshell" => (Some("jdk-openjdk"), Some("java-latest-openjdk-devel"), Some("default-jdk")),
        "kotlinc" | "kotlin" => (Some("kotlin"), None, Some("kotlin")),
        "groovy" | "groovysh" => (Some("groovy"), Some("groovy"), Some("groovy")),
        "clojure" | "clj" => (Some("clojure"), None, Some("clojure")),
        "dart" => (Some("dart"), None, None),
        "go" => (Some("go"), Some("golang"), Some("golang-go")),
        "rustc" => (Some("rust"), Some("rust"), Some("rustc")),
        "cc" | "gcc" => (Some("gcc"), Some("gcc"), Some("gcc")),
        "c++" | "g++" => (Some("gcc"), Some("gcc-c++"), Some("g++")),
        "clang" | "clang++" => (Some("clang"), Some("clang"), Some("clang")),
        "zig" => (Some("zig"), Some("zig"), None),
        "nim" => (Some("nim"), Some("nim"), Some("nim")),
        "crystal" => (Some("crystal"), None, Some("crystal")),
        "odin" => (Some("odin"), None, None),
        "dmd" | "ldc2" => (Some("dmd"), Some("ldc"), Some("ldc")),
        "gfortran" => (Some("gcc-fortran"), Some("gcc-gfortran"), Some("gfortran")),
        "fpc" => (Some("fpc"), Some("fpc"), Some("fpc")),
        "gnatmake" => (Some("gcc-ada"), Some("gcc-gnat"), Some("gnat")),
        "cobc" => (None, Some("gnucobol"), Some("gnucobol")),
        "nasm" => (Some("nasm"), Some("nasm"), Some("nasm")),
        "iverilog" => (Some("iverilog"), Some("iverilog"), Some("iverilog")),
        "swipl" => (Some("swi-prolog"), Some("pl"), Some("swi-prolog")),
        "racket" => (Some("racket"), Some("racket"), Some("racket")),
        "guile" => (Some("guile"), Some("guile30"), Some("guile-3.0")),
        "sbcl" => (Some("sbcl"), Some("sbcl"), Some("sbcl")),
        "emacs" => (Some("emacs"), Some("emacs"), Some("emacs")),
        "janet" => (None, None, None),
        "gst" => (Some("smalltalk"), None, Some("gnu-smalltalk")),
        "haxe" => (Some("haxe"), Some("haxe"), Some("haxe")),
        "octave" => (Some("octave"), Some("octave"), Some("octave")),
        "sqlite3" => (Some("sqlite"), Some("sqlite"), Some("sqlite3")),
        "gnuplot" => (Some("gnuplot"), Some("gnuplot"), Some("gnuplot")),
        "typst" => (Some("typst"), None, None),
        "tectonic" => (Some("tectonic"), None, None),
        "make" => (Some("make"), Some("make"), Some("make")),
        "just" => (Some("just"), Some("just"), Some("just")),
        "docker" => (Some("docker"), Some("moby-engine"), Some("docker.io")),
        "dotnet" => (Some("dotnet-sdk"), Some("dotnet-sdk-10.0"), None),
        "mcs" => (Some("mono"), Some("mono-devel"), Some("mono-mcs")),
        "wine" => (Some("wine"), Some("wine"), Some("wine")),
        "nix-instantiate" | "nix" => (Some("nix"), None, Some("nix-bin")),
        "pwsh" => (None, None, None),
        _ => (None, None, None),
    };
    let package = match family {
        "arch" => pkgs.0.map(|p| format!("sudo pacman -S {p}")),
        "fedora" => pkgs.1.map(|p| format!("sudo dnf install {p}")),
        "debian" => pkgs.2.map(|p| format!("sudo apt install {p}")),
        _ => None,
    };
    let fallback = match tool {
        "rustc" if has_rustup => Some("rustup default stable"),
        "rustc" => Some("curl --proto '=https' -sSf https://sh.rustup.rs | sh"),
        "tsx" => Some("npm install -g tsx"),
        "ts-node" => Some("npm install -g ts-node typescript"),
        "coffee" => Some("npm install -g coffeescript"),
        "bun" => Some("curl -fsSL https://bun.sh/install | bash"),
        "deno" => Some("curl -fsSL https://deno.land/install.sh | sh"),
        "julia" => Some("curl -fsSL https://install.julialang.org | sh"),
        "scala-cli" | "scala" => Some("curl -sSLf https://scala-cli.virtuslab.org/get | sh"),
        "bb" => Some("bash <(curl -s https://raw.githubusercontent.com/babashka/babashka/master/install)"),
        "fennel" => Some("luarocks install fennel"),
        "evcxr" => Some("cargo install evcxr_repl"),
        "typst" => Some("cargo install --locked typst-cli"),
        "uv" => Some("curl -LsSf https://astral.sh/uv/install.sh | sh"),
        _ => None,
    };
    InstallHint { server: tool.to_string(), command: package.or_else(|| fallback.map(str::to_string)) }
}

// Trusting a folder ---------------------------------------------------------------------------

/// Folders whose `.tinta/` files may run commands: the user said so once, and it's kept.
pub mod trust {
    use std::path::{Path, PathBuf};

    fn store() -> PathBuf {
        crate::settings::state_dir().join("trusted.json")
    }

    fn load() -> Vec<PathBuf> {
        std::fs::read_to_string(store()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    fn save(list: &[PathBuf]) -> Result<(), String> {
        let p = store();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
        std::fs::write(p, text).map_err(|e| e.to_string())
    }

    fn canonical(dir: &Path) -> PathBuf {
        std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
    }

    /// The folder, or one it's in, was trusted.
    pub fn is_trusted(dir: &Path) -> bool {
        let dir = canonical(dir);
        load().iter().any(|t| dir.starts_with(t))
    }

    pub fn trust(dir: &Path) -> Result<(), String> {
        let dir = canonical(dir);
        let mut list = load();
        if !list.contains(&dir) {
            list.push(dir);
        }
        save(&list)
    }

    /// Takes back the trust in the folder and in any folder above it that covered it.
    pub fn untrust(dir: &Path) -> Result<(), String> {
        let dir = canonical(dir);
        let mut list = load();
        list.retain(|t| !dir.starts_with(t));
        save(&list)
    }

    /// The project's files that run commands, those that exist.
    pub fn command_files(root: &Path) -> Vec<&'static str> {
        [super::RUN_PATH, crate::dap::LAUNCH_PATH, crate::tasks::TASKS_PATH].into_iter().filter(|f| root.join(f).exists()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req<'a>(file: &'a Path, language: &'a str, root: &'a Path, user: &'a UserRunners) -> Request<'a> {
        Request { file, language, root, user, project: None, args: "", out_dir: Path::new("/tmp/out"), first_line: "" }
    }

    fn all(_: &str) -> bool {
        true
    }

    fn shell(p: &RunPlan) -> (Option<String>, String) {
        match &p.action {
            Action::Shell { build, run } => (build.clone(), run.clone()),
            a => panic!("{a:?}"),
        }
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tinta-run-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn interpreted_languages_run_the_file() {
        let user = UserRunners::new();
        let root = Path::new("/p");
        for (file, lang, expected) in [
            ("/p/a.py", "Python", "python3 /p/a.py"),
            ("/p/a.rb", "Ruby", "ruby /p/a.rb"),
            ("/p/a.js", "JavaScript (Babel)", "node /p/a.js"),
            ("/p/a.lua", "Lua", "lua /p/a.lua"),
            ("/p/a.R", "R", "Rscript /p/a.R"),
            ("/p/a.java", "Java", "java /p/a.java"),
            ("/p/a.scm", "Lisp", "guile /p/a.scm"),
            ("/p/a.lisp", "Lisp", "sbcl --script /p/a.lisp"),
            ("/p/Makefile", "Makefile", "make -f /p/Makefile"),
        ] {
            let p = plan(&req(Path::new(file), lang, root, &user), &all).unwrap();
            assert_eq!(shell(&p), (None, expected.to_string()), "{file}");
            assert_eq!(p.cwd, root);
            assert_eq!(p.source, Source::Builtin);
        }
    }

    #[test]
    fn compiled_languages_build_then_run_what_they_built() {
        let user = UserRunners::new();
        let p = plan(&req(Path::new("/p/my prog.c"), "C", Path::new("/p"), &user), &all).unwrap();
        let (build, run) = shell(&p);
        assert_eq!(build.unwrap(), "cc -g -Wall -o /tmp/out/my_prog '/p/my prog.c' -lm");
        assert_eq!(run, "/tmp/out/my_prog");
        let p = plan(&req(Path::new("/p/a.kt"), "Kotlin", Path::new("/p"), &user), &all).unwrap();
        assert_eq!(shell(&p).1, "java -jar /tmp/out/a.jar");
    }

    #[test]
    fn the_first_installed_way_wins_and_none_says_what_to_install() {
        let user = UserRunners::new();
        let only_bun = |t: &str| t == "bun";
        let p = plan(&req(Path::new("/p/a.ts"), "TypeScript", Path::new("/p"), &user), &only_bun).unwrap();
        assert_eq!(shell(&p).1, "bun run /p/a.ts");
        match plan(&req(Path::new("/p/a.rb"), "Ruby", Path::new("/p"), &user), &|_| false) {
            Err(RunError::Missing { tool, .. }) => assert_eq!(tool, "ruby"),
            other => panic!("{other:?}"),
        }
        assert_eq!(plan(&req(Path::new("/p/a.xyz"), "Plain Text", Path::new("/p"), &user), &all), Err(RunError::Unknown("Plain Text".into())));
    }

    #[test]
    fn arguments_go_at_the_end_or_where_asked() {
        let user = UserRunners::new();
        let mut r = req(Path::new("/p/a.py"), "Python", Path::new("/p"), &user);
        r.args = "--n 3 < in.txt";
        assert_eq!(shell(&plan(&r, &all).unwrap()).1, "python3 /p/a.py --n 3 < in.txt");
        let mut user = UserRunners::new();
        user.insert("Python".into(), UserRunner::Command("python3 ${file} ${args} | head".into()));
        let mut r = req(Path::new("/p/a.py"), "Python", Path::new("/p"), &user);
        r.args = "x";
        assert_eq!(shell(&plan(&r, &all).unwrap()).1, "python3 /p/a.py x | head");
    }

    #[test]
    fn the_users_commands_come_first_by_extension_or_language() {
        let mut user = UserRunners::new();
        user.insert(".PY".into(), UserRunner::Command("py-ext ${file}".into()));
        user.insert("ruby".into(), UserRunner::Steps { build: Some("check ${fileBasename}".into()), run: "rb ${fileBasenameNoExtension}".into(), cwd: Some("${workspaceFolder}/sub".into()) });
        let p = plan(&req(Path::new("/p/x/a.py"), "Python", Path::new("/p"), &user), &all).unwrap();
        assert_eq!(shell(&p).1, "py-ext /p/x/a.py");
        assert_eq!((p.source, p.cwd), (Source::Settings, PathBuf::from("/p/x")));
        let p = plan(&req(Path::new("/p/a.rb"), "Ruby", Path::new("/p"), &user), &all).unwrap();
        assert_eq!(shell(&p), (Some("check a.rb".into()), "rb a".into()));
        assert_eq!(p.cwd, PathBuf::from("/p/sub"));
        // The project's come after the user's.
        let mut project = UserRunners::new();
        project.insert("Python".into(), UserRunner::Command("proj ${file}".into()));
        project.insert("Lua".into(), UserRunner::Command("proj-lua ${file}".into()));
        let mut r = req(Path::new("/p/a.py"), "Python", Path::new("/p"), &user);
        r.project = Some(&project);
        assert_eq!(shell(&plan(&r, &all).unwrap()).1, "py-ext /p/a.py");
        let mut r = req(Path::new("/p/a.lua"), "Lua", Path::new("/p"), &user);
        r.project = Some(&project);
        let p = plan(&r, &all).unwrap();
        assert_eq!((shell(&p).1, p.source), ("proj-lua /p/a.lua".into(), Source::Project));
    }

    #[test]
    fn shebangs_say_what_runs_the_file() {
        assert_eq!(shebang("#!/usr/bin/env python3"), Some(("/usr/bin/env python3".into(), "python3".into())));
        assert_eq!(shebang("#!/usr/bin/env -S deno run -A"), Some(("/usr/bin/env -S deno run -A".into(), "deno".into())));
        assert_eq!(shebang("#!/bin/bash -e"), Some(("/bin/bash -e".into(), "/bin/bash".into())));
        assert_eq!(shebang("#![allow(unused)]"), None);
        assert_eq!(shebang("print(1)"), None);
        let user = UserRunners::new();
        let mut r = req(Path::new("/p/script"), "Plain Text", Path::new("/p"), &user);
        r.first_line = "#!/usr/bin/env ruby";
        let p = plan(&r, &all).unwrap();
        assert_eq!((shell(&p).1, p.source), ("/usr/bin/env ruby /p/script".into(), Source::Shebang));
        match plan(&r, &|_| false) {
            Err(RunError::Missing { tool, .. }) => assert_eq!(tool, "ruby"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pages_open_and_markdown_previews() {
        let user = UserRunners::new();
        assert_eq!(plan(&req(Path::new("/p/i.html"), "HTML", Path::new("/p"), &user), &all).unwrap().action, Action::Open("/p/i.html".into()));
        assert_eq!(plan(&req(Path::new("/p/r.md"), "Markdown", Path::new("/p"), &user), &all).unwrap().action, Action::Preview);
    }

    #[test]
    fn cargo_projects_run_the_target_the_file_belongs_to() {
        let root = temp("cargo");
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let user = UserRunners::new();
        for (file, build, run) in [
            ("src/main.rs", "cargo build", "cargo run -q"),
            ("src/util.rs", "cargo build", "cargo run -q"),
            ("src/bin/tool.rs", "cargo build --bin tool", "cargo run -q --bin tool"),
            ("src/bin/big/main.rs", "cargo build --bin big", "cargo run -q --bin big"),
            ("examples/demo.rs", "cargo build --example demo", "cargo run -q --example demo"),
            ("tests/it.rs", "cargo test --no-run --test it", "cargo test -q --test it"),
        ] {
            let f = root.join(file);
            let p = plan(&req(&f, "Rust", &root, &user), &all).unwrap();
            assert_eq!(shell(&p), (Some(build.to_string()), run.to_string()), "{file}");
            assert_eq!((p.cwd.as_path(), p.source), (root.as_path(), Source::Environment));
        }
        let f = root.join("src/main.rs");
        let mut r = req(&f, "Rust", &root, &user);
        r.args = "a b";
        assert_eq!(shell(&plan(&r, &all).unwrap()).1, "cargo run -q -- a b");
    }

    #[test]
    fn python_uses_the_projects_environment() {
        let root = temp("venv");
        std::fs::create_dir_all(root.join(".venv/bin")).unwrap();
        std::fs::write(root.join(".venv/bin/python"), "").unwrap();
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        let f = root.join("pkg/m.py");
        let user = UserRunners::new();
        let p = plan(&req(&f, "Python", &root, &user), &all).unwrap();
        assert_eq!(shell(&p).1, format!("{}/.venv/bin/python {}", root.display(), f.display()));
        assert_eq!(p.cwd, root.join("pkg"));
    }

    #[test]
    fn go_modules_build_the_package() {
        let root = temp("gomod");
        std::fs::write(root.join("go.mod"), "module x\n").unwrap();
        let user = UserRunners::new();
        let f = root.join("main.go");
        let p = plan(&req(&f, "Go", &root, &user), &all).unwrap();
        assert_eq!(shell(&p), (Some("go build -o /tmp/out/main .".into()), "/tmp/out/main".into()));
        let t = root.join("main_test.go");
        assert_eq!(shell(&plan(&req(&t, "Go", &root, &user), &all).unwrap()), (None, "go test -v .".into()));
    }

    #[test]
    fn make_projects_run_what_the_build_made() {
        let root = temp("make");
        std::fs::write(root.join("Makefile"), "all:\n\tcc -o app main.c\n").unwrap();
        let user = UserRunners::new();
        let f = root.join("main.c");
        let p = plan(&req(&f, "C", &root, &user), &all).unwrap();
        assert_eq!(shell(&p).0.as_deref(), Some("make"));
        assert_eq!(p.newest_in.as_deref(), Some(root.as_path()));
        assert!(finish(p).is_err(), "nothing was built");
    }

    #[test]
    fn untitled_text_gets_an_extension_that_runs() {
        assert_eq!(extension_for("Python"), Some("py"));
        assert_eq!(extension_for("C++"), Some("cpp"));
        assert_eq!(extension_for("Bourne Again Shell (bash)"), Some("sh"));
        assert_eq!(extension_for("Plain Text"), None);
    }

    #[test]
    fn quoting_leaves_plain_paths_alone() {
        assert_eq!(shell_quote("/a/b-c_d.py"), "/a/b-c_d.py");
        assert_eq!(shell_quote("/a b/it's"), r"'/a b/it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn repls_by_language() {
        let r = repl_for(Some(Path::new("a.py")), "Python", &|t| t == "python3").unwrap().unwrap();
        assert_eq!((r.tool.as_str(), r.command.as_str(), r.kind), ("python3", "python3 -q", ReplKind::Python));
        assert!(repl_for(None, "Ruby", &all).unwrap().is_ok());
        assert!(repl_for(Some(Path::new("a.md")), "Markdown", &all).is_none());
        match repl_for(Some(Path::new("a.py")), "Python", &|_| false) {
            Some(Err((tool, _))) => assert_eq!(tool, "python3"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn python_blocks_reach_the_repl_whole() {
        let src = "    for i in range(3):\n\n        print(i)\n    print('done')\n";
        assert_eq!(repl_input(ReplKind::Python, src), ("for i in range(3):\n    print(i)\n\nprint('done')".to_string(), 2));
        assert_eq!(repl_input(ReplKind::Python, "  x = 1\n"), ("x = 1".to_string(), 1));
        assert_eq!(repl_input(ReplKind::Python, "if x:\n    y()\nelse:\n    z()"), ("if x:\n    y()\nelse:\n    z()".to_string(), 2));
        assert_eq!(repl_input(ReplKind::Other, "  a\n\n  b\n"), ("a\n\nb".to_string(), 1));
    }

    #[test]
    fn install_hints_follow_the_system() {
        assert_eq!(tool_hint_for("ruby", "fedora", false).command.as_deref(), Some("sudo dnf install ruby"));
        assert_eq!(tool_hint_for("go", "debian", false).command.as_deref(), Some("sudo apt install golang-go"));
        assert_eq!(tool_hint_for("tsx", "arch", false).command.as_deref(), Some("npm install -g tsx"));
        assert_eq!(tool_hint_for("python3", "", false).command, None);
    }

    #[test]
    fn templates_parse() {
        let v: UserRunners = serde_json::from_str(&crate::settings::strip_jsonc(RUN_TEMPLATE)).unwrap();
        assert!(v.is_empty());
        let v: UserRunners = serde_json::from_str(r#"{ "Python": "python3 ${file}", ".c": { "build": "cc", "run": "${out}" } }"#).unwrap();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn every_runner_has_a_way() {
        for r in RUNNERS {
            assert!(!r.ways.is_empty() && !r.exts.is_empty(), "{:?}", r.exts);
        }
    }
}
