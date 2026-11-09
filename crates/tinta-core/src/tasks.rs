//! Build tasks: what a project offers, and reading errors out of what they print.

use crate::lsp::Severity;
use regex::Regex;
use serde::Deserialize;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Task {
    pub name: String,
    pub command: String,
    pub cwd: Option<String>,
    /// "build" makes it what Ctrl+Shift+B runs.
    pub group: String,
}


fn task(name: &str, command: &str, group: &str) -> Task {
    Task { name: name.into(), command: command.into(), cwd: None, group: group.into() }
}

pub const TASKS_PATH: &str = ".tinta/tasks.json";

/// The project's own `.tinta/tasks.json` first, unless the folder isn't trusted with it, then
/// what its files suggest.
pub fn detect(root: &Path, use_project_file: bool) -> (Vec<Task>, Option<String>) {
    let mut out = Vec::new();
    let mut problem = None;
    if use_project_file && let Ok(src) = std::fs::read_to_string(root.join(TASKS_PATH)) {
        match serde_json::from_str::<Vec<Task>>(&crate::settings::strip_jsonc(&src)) {
            Ok(list) => out.extend(list.into_iter().filter(|t| !t.command.is_empty())),
            Err(e) => problem = Some(format!(".tinta/tasks.json: {e}")),
        }
    }
    if root.join("Cargo.toml").exists() {
        out.push(task("cargo build", "cargo build", "build"));
        out.push(task("cargo check", "cargo check", ""));
        out.push(task("cargo test", "cargo test", "test"));
        out.push(task("cargo run", "cargo run", "run"));
        out.push(task("cargo clippy", "cargo clippy", ""));
    }
    if root.join("go.mod").exists() {
        out.push(task("go build", "go build ./...", "build"));
        out.push(task("go test", "go test ./...", "test"));
    }
    for name in ["Makefile", "makefile", "GNUmakefile"] {
        if let Ok(src) = std::fs::read_to_string(root.join(name)) {
            out.push(task("make", "make", "build"));
            out.extend(make_targets(&src).into_iter().take(16).map(|t| task(&format!("make {t}"), &format!("make {t}"), "")));
            break;
        }
    }
    for name in ["justfile", "Justfile"] {
        if let Ok(src) = std::fs::read_to_string(root.join(name)) {
            out.extend(just_recipes(&src).into_iter().take(16).map(|t| task(&format!("just {t}"), &format!("just {t}"), "")));
            break;
        }
    }
    if root.join("CMakeLists.txt").exists() {
        out.push(task("cmake", "cmake -B build && cmake --build build", "build"));
    }
    if root.join("meson.build").exists() {
        out.push(task("meson", "meson setup build --reconfigure && meson compile -C build", "build"));
    }
    if let Ok(src) = std::fs::read_to_string(root.join("package.json")) {
        let runner = if root.join("pnpm-lock.yaml").exists() {
            "pnpm run"
        } else if root.join("yarn.lock").exists() {
            "yarn"
        } else {
            "npm run"
        };
        out.extend(npm_scripts(&src).into_iter().map(|s| {
            let group = if s == "build" { "build" } else if s == "test" { "test" } else { "" };
            task(&format!("{runner} {s}"), &format!("{runner} {s}"), group)
        }));
    }
    if root.join("pyproject.toml").exists() || root.join("pytest.ini").exists() {
        out.push(task("pytest", "python3 -m pytest", "test"));
    }
    (out, problem)
}

/// What Ctrl+Shift+B runs: the first task marked as a build, or just the first.
pub fn default_build(tasks: &[Task]) -> Option<&Task> {
    tasks.iter().find(|t| t.group == "build").or(tasks.first())
}

fn make_targets(src: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^([A-Za-z0-9][A-Za-z0-9_-]*)\s*:([^=]|$)").unwrap());
    let mut out: Vec<String> = Vec::new();
    for line in src.lines() {
        if let Some(c) = re.captures(line) {
            let t = c[1].to_string();
            if !out.contains(&t) {
                out.push(t);
            }
        }
    }
    out
}

fn just_recipes(src: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"^@?([A-Za-z0-9][A-Za-z0-9_-]*)[^:=]*:([^=]|$)").unwrap());
    src.lines().filter_map(|l| re.captures(l).map(|c| c[1].to_string())).collect()
}

fn npm_scripts(src: &str) -> Vec<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(src) else { return Vec::new() };
    v.get("scripts").and_then(|s| s.as_object()).map(|o| o.keys().cloned().collect()).unwrap_or_default()
}

// Reading output ----------------------------------------------------------------------------

/// Removes colour codes and other escapes, and resolves carriage returns (progress bars).
pub fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('@'..='~').contains(&n) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\u{7}' {
                            break;
                        }
                        if n == '\u{1b}' {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            },
            '\r' => {
                // What comes after overwrites the line, unless it's just the end of a CRLF.
                if chars.peek().is_some() {
                    out.clear();
                }
            }
            c if c.is_control() && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// A place in a file, mentioned in some text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub path: String,
    /// 0-based.
    pub line: usize,
    pub col: usize,
    /// Where in the text the mention is, in chars.
    pub start: usize,
    pub end: usize,
}

fn location_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // path:line[:col], path(line,col) as TypeScript prints it, and Python's traceback form.
    RE.get_or_init(|| {
        Regex::new(
            r#"(?x)
            File\ "(?P<py>[^"]+)",\ line\ (?P<pyl>\d+)
            | (?P<ts>[\w./~+@-]+\.\w+)\((?P<tsl>\d+),(?P<tsc>\d+)\)
            | (?P<p>/?(?:[\w.~+@-]+/)*[\w.~+@-]*\.?[\w+-]+):(?P<l>\d+)(?::(?P<c>\d+))?
            "#,
        )
        .unwrap()
    })
}

/// Every `file:line:col` in a line of text.
pub fn find_locations(text: &str) -> Vec<Location> {
    let mut out = Vec::new();
    for c in location_re().captures_iter(text) {
        let m = c.get(0).unwrap();
        // The tail of something longer, like the `/localhost:80` in a URL.
        if text[..m.start()].ends_with(['/', ':']) {
            continue;
        }
        let (path, line, col) = if let Some(p) = c.name("py") {
            (p.as_str(), c.name("pyl"), None)
        } else if let Some(p) = c.name("ts") {
            (p.as_str(), c.name("tsl"), c.name("tsc"))
        } else {
            (c.name("p").map_or("", |p| p.as_str()), c.name("l"), c.name("c"))
        };
        // `localhost:8080`, `12:30:45`: a path has a slash or an extension, and isn't all digits.
        let looks_like_path = (path.contains('/') || path.rsplit_once('.').is_some_and(|(_, e)| !e.is_empty() && !e.chars().all(|c| c.is_ascii_digit())))
            && !path.chars().all(|c| c.is_ascii_digit() || c == '.');
        if !looks_like_path || path.starts_with("http") {
            continue;
        }
        let Some(line) = line.and_then(|l| l.as_str().parse::<usize>().ok()) else { continue };
        let col = col.and_then(|l| l.as_str().parse::<usize>().ok()).unwrap_or(1);
        out.push(Location {
            path: path.to_string(),
            line: line.saturating_sub(1),
            col: col.saturating_sub(1),
            start: text[..m.start()].chars().count(),
            end: text[..m.end()].chars().count(),
        });
    }
    out
}

/// The location under a column, if any: what Ctrl+click in the terminal follows.
pub fn location_at(text: &str, col: usize) -> Option<Location> {
    find_locations(text).into_iter().find(|l| l.start <= col && col < l.end)
}

/// A URL under a column.
pub fn url_at(text: &str, col: usize) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"https?://[^\s"'<>)\]]+"#).unwrap());
    re.find_iter(text)
        .find(|m| {
            let (a, b) = (text[..m.start()].chars().count(), text[..m.end()].chars().count());
            a <= col && col < b
        })
        .map(|m| m.as_str().trim_end_matches(['.', ',', ';']).to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub location: Location,
    pub severity: Severity,
    pub message: String,
}

/// Reads compiler output line by line. rustc says what's wrong a line before saying where.
#[derive(Debug, Default)]
pub struct ProblemParser {
    pending: Option<(Severity, String)>,
}

fn head_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^(error|warning)(?:\[[^\]]*\])?: (.*)$").unwrap())
}

fn inline_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // gcc, clang, tsc, go vet: the message on the same line as the place.
    RE.get_or_init(|| Regex::new(r"(?i)[:\s](fatal error|error|warning)(?:\s+\w+\d+)?:\s*(.*)$").unwrap())
}

impl ProblemParser {
    pub fn feed(&mut self, line: &str) -> Option<Problem> {
        if let Some(c) = head_re().captures(line) {
            let sev = if &c[1] == "error" { Severity::Error } else { Severity::Warning };
            let msg = c[2].to_string();
            // "warning: `x` (lib) generated 3 warnings" and friends point nowhere.
            self.pending = Some((sev, msg));
            return None;
        }
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("--> ")
            && let Some((sev, message)) = self.pending.take()
            && let Some(location) = find_locations(rest).into_iter().next()
        {
            return Some(Problem { location, severity: sev, message });
        }
        if trimmed.is_empty() {
            self.pending = None;
        }
        let location = find_locations(line).into_iter().next()?;
        if line.trim_start().starts_with("File \"") {
            return Some(Problem { location, severity: Severity::Error, message: "Traceback".to_string() });
        }
        let tail = line.chars().skip(location.end).collect::<String>();
        let c = inline_re().captures(&tail)?;
        let severity = if c[1].eq_ignore_ascii_case("warning") { Severity::Warning } else { Severity::Error };
        Some(Problem { location, severity, message: c[2].trim().to_string() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("tinta-tasks-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn detects_project_tasks() {
        let d = dir("detect");
        std::fs::write(d.join("Cargo.toml"), "").unwrap();
        std::fs::write(d.join("Makefile"), "VAR := 1\nall: x\n\tcc\nclean:\n\trm\n.PHONY: all\n").unwrap();
        std::fs::write(d.join("package.json"), r#"{"scripts":{"build":"x","lint":"y"}}"#).unwrap();
        let (t, err) = detect(&d, true);
        assert!(err.is_none());
        let names: Vec<_> = t.iter().map(|x| x.name.as_str()).collect();
        for want in ["cargo build", "cargo test", "make", "make all", "make clean", "npm run build", "npm run lint"] {
            assert!(names.contains(&want), "{want} in {names:?}");
        }
        assert!(!names.contains(&"make VAR"));
        assert_eq!(default_build(&t).unwrap().name, "cargo build");
    }

    #[test]
    fn the_projects_own_tasks_come_first() {
        let d = dir("own");
        std::fs::create_dir_all(d.join(".tinta")).unwrap();
        std::fs::write(d.join("Cargo.toml"), "").unwrap();
        std::fs::write(d.join(".tinta/tasks.json"), r#"[ // mine
            { "name": "release", "command": "cargo build --release", "group": "build" },
        ]"#).unwrap();
        let (t, err) = detect(&d, true);
        assert!(err.is_none(), "{err:?}");
        assert_eq!(default_build(&t).unwrap().name, "release");
        // A folder not trusted with its own file gets only what its files suggest.
        let (t, _) = detect(&d, false);
        assert_eq!(default_build(&t).unwrap().name, "cargo build");
        std::fs::write(d.join(".tinta/tasks.json"), "{ nope").unwrap();
        assert!(detect(&d, true).1.is_some());
    }

    #[test]
    fn cleans_terminal_codes() {
        assert_eq!(clean_line("\u{1b}[1;31merror\u{1b}[0m: x"), "error: x");
        assert_eq!(clean_line("10%\r50%\r100% done"), "100% done");
        assert_eq!(clean_line("linha\r"), "linha");
        assert_eq!(clean_line("\u{1b}]8;;http://a\u{7}link\u{1b}]8;;\u{7}"), "link");
    }

    #[test]
    fn finds_locations_in_the_usual_forms() {
        let one = |s: &str| find_locations(s).into_iter().next().map(|l| (l.path, l.line, l.col));
        assert_eq!(one("  --> src/main.rs:4:5"), Some(("src/main.rs".into(), 3, 4)));
        assert_eq!(one("a.c:10:2: error: x"), Some(("a.c".into(), 9, 1)));
        assert_eq!(one("./pkg/a.go:7: bad"), Some(("./pkg/a.go".into(), 6, 0)));
        assert_eq!(one("/abs/p/a.py:3"), Some(("/abs/p/a.py".into(), 2, 0)));
        assert_eq!(one("src/x.ts(3,5): error TS1: y"), Some(("src/x.ts".into(), 2, 4)));
        assert_eq!(one(r#"  File "m/a.py", line 12, in f"#), Some(("m/a.py".into(), 11, 0)));
        // Things that look like it and aren't.
        assert_eq!(one("at 12:30:45 today"), None);
        assert_eq!(one("http://localhost:8080/x"), None);
        assert_eq!(one("listening on 127.0.0.1:80"), None);
    }

    #[test]
    fn location_under_a_column() {
        let t = "erro em src/a.rs:3:1 agora";
        assert_eq!(location_at(t, 10).unwrap().path, "src/a.rs");
        assert!(location_at(t, 2).is_none());
        assert!(location_at(t, 22).is_none());
        assert_eq!(url_at("veja https://a.b/c, ok", 8).as_deref(), Some("https://a.b/c"));
        assert!(url_at("veja https://a.b/c, ok", 1).is_none());
    }

    #[test]
    fn reads_rustc_and_gcc_errors() {
        let mut p = ProblemParser::default();
        assert!(p.feed("   Compiling a v0.1.0 (/tmp/a)").is_none());
        assert!(p.feed("error[E0308]: mismatched types").is_none());
        let got = p.feed(" --> src/main.rs:4:18").unwrap();
        assert_eq!(got.message, "mismatched types");
        assert_eq!(got.severity, Severity::Error);
        assert_eq!((got.location.line, got.location.col), (3, 17));
        assert!(p.feed("warning: `a` (bin) generated 1 warning").is_none());
        assert!(p.feed("").is_none());
        assert!(p.feed("error: could not compile `a`").is_none());

        let mut p = ProblemParser::default();
        let got = p.feed("erro.c:8:33: error: use of undeclared identifier 'nada'").unwrap();
        assert_eq!(got.message, "use of undeclared identifier 'nada'");
        let got = p.feed("a.c:2:1: warning: unused [-Wunused]").unwrap();
        assert_eq!(got.severity, Severity::Warning);
        assert!(p.feed("a.c:2:1: note: declared here").is_none());
        let got = p.feed("src/x.ts(3,5): error TS2322: nope").unwrap();
        assert_eq!(got.message, "nope");
        let got = p.feed(r#"  File "m/a.py", line 12, in f"#).unwrap();
        assert_eq!(got.location.path, "m/a.py");
    }
}
