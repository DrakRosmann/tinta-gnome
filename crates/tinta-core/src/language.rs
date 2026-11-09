//! What the editor knows about each language beyond its grammar.

use crate::syntax::syntax_set;
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Language {
    /// Name of the syntect grammar.
    pub name: String,
    /// `languageId` for LSP, when it differs from nothing at all.
    pub lsp_id: Option<&'static str>,
    pub line_comment: Option<&'static str>,
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Pressing Enter after one of these indents the next line.
    pub indent_after: &'static [char],
    pub auto_quotes: &'static [char],
}

struct Row {
    names: &'static [&'static str],
    lsp: Option<&'static str>,
    line: Option<&'static str>,
    block: Option<(&'static str, &'static str)>,
    colon: bool,
}

const C_BLOCK: Option<(&str, &str)> = Some(("/*", "*/"));
const XML_BLOCK: Option<(&str, &str)> = Some(("<!--", "-->"));

const fn row(
    names: &'static [&'static str],
    lsp: Option<&'static str>,
    line: Option<&'static str>,
    block: Option<(&'static str, &'static str)>,
) -> Row {
    Row { names, lsp, line, block, colon: false }
}

/// Grammar names come from the bundled syntect definitions.
const TABLE: &[Row] = &[
    row(&["Rust"], Some("rust"), Some("//"), C_BLOCK),
    row(&["C"], Some("c"), Some("//"), C_BLOCK),
    row(&["C++"], Some("cpp"), Some("//"), C_BLOCK),
    row(&["Objective-C"], Some("objective-c"), Some("//"), C_BLOCK),
    row(&["Objective-C++"], Some("objective-cpp"), Some("//"), C_BLOCK),
    row(&["C#"], Some("csharp"), Some("//"), C_BLOCK),
    row(&["Java"], Some("java"), Some("//"), C_BLOCK),
    row(&["Kotlin"], Some("kotlin"), Some("//"), C_BLOCK),
    row(&["Scala"], Some("scala"), Some("//"), C_BLOCK),
    row(&["Groovy", "Gradle"], Some("groovy"), Some("//"), C_BLOCK),
    row(&["Go"], Some("go"), Some("//"), C_BLOCK),
    row(&["Swift"], Some("swift"), Some("//"), C_BLOCK),
    row(&["Dart"], Some("dart"), Some("//"), C_BLOCK),
    row(&["Zig"], Some("zig"), Some("//"), None),
    row(&["D"], Some("d"), Some("//"), C_BLOCK),
    row(&["JavaScript", "JavaScript (Babel)"], Some("javascript"), Some("//"), C_BLOCK),
    row(&["JSX"], Some("javascriptreact"), Some("//"), C_BLOCK),
    row(&["TypeScript"], Some("typescript"), Some("//"), C_BLOCK),
    row(&["TypeScriptReact", "TSX"], Some("typescriptreact"), Some("//"), C_BLOCK),
    row(&["CSS"], Some("css"), None, C_BLOCK),
    row(&["SCSS", "Sass"], Some("scss"), Some("//"), C_BLOCK),
    row(&["LESS"], Some("less"), Some("//"), C_BLOCK),
    row(&["HTML", "HTML (Rails)", "HTML (Jinja2)", "Vue Component", "Svelte"], Some("html"), None, XML_BLOCK),
    row(&["XML", "XSL", "SVG"], Some("xml"), None, XML_BLOCK),
    row(&["Markdown", "MultiMarkdown"], Some("markdown"), None, XML_BLOCK),
    row(&["PHP"], Some("php"), Some("//"), C_BLOCK),
    row(&["Bourne Again Shell (bash)", "Shell-Unix-Generic"], Some("shellscript"), Some("#"), None),
    row(&["fish"], Some("fish"), Some("#"), None),
    row(&["PowerShell"], Some("powershell"), Some("#"), Some(("<#", "#>"))),
    row(&["Ruby", "Ruby on Rails"], Some("ruby"), Some("#"), None),
    row(&["Perl"], Some("perl"), Some("#"), None),
    row(&["R"], Some("r"), Some("#"), None),
    row(&["Julia"], Some("julia"), Some("#"), Some(("#=", "=#"))),
    row(&["Nix"], Some("nix"), Some("#"), C_BLOCK),
    row(&["TOML"], Some("toml"), Some("#"), None),
    row(&["YAML"], Some("yaml"), Some("#"), None),
    row(&["Dockerfile"], Some("dockerfile"), Some("#"), None),
    row(&["Makefile"], Some("makefile"), Some("#"), None),
    row(&["CMake", "CMakeCache"], Some("cmake"), Some("#"), None),
    row(&["Meson"], Some("meson"), Some("#"), None),
    row(&["Terraform", "HCL"], Some("terraform"), Some("#"), C_BLOCK),
    row(&["INI", "Git Config", "Git Ignore", "Git Attributes", "Git Commit", "Properties"], None, Some("#"), None),
    row(&["Elixir"], Some("elixir"), Some("#"), None),
    row(&["SQL", "PostgreSQL", "MySQL"], Some("sql"), Some("--"), C_BLOCK),
    row(&["Lua"], Some("lua"), Some("--"), Some(("--[[", "]]"))),
    row(&["Haskell"], Some("haskell"), Some("--"), Some(("{-", "-}"))),
    row(&["Elm"], Some("elm"), Some("--"), Some(("{-", "-}"))),
    row(&["Ada"], Some("ada"), Some("--"), None),
    row(&["VHDL"], Some("vhdl"), Some("--"), None),
    row(&["Verilog", "SystemVerilog"], Some("verilog"), Some("//"), C_BLOCK),
    row(&["Erlang"], Some("erlang"), Some("%"), None),
    row(&["LaTeX", "TeX", "BibTeX"], Some("latex"), Some("%"), None),
    row(&["MATLAB"], Some("matlab"), Some("%"), Some(("%{", "%}"))),
    row(&["Lisp", "Emacs Lisp", "Clojure"], Some("clojure"), Some(";"), None),
    row(&["Scheme", "Racket"], Some("scheme"), Some(";"), None),
    row(&["Assembly x86 (NASM)", "NASM", "Assembly (x86_64)"], None, Some(";"), None),
    row(&["OCaml", "OCamllex", "OCamlyacc"], Some("ocaml"), None, Some(("(*", "*)"))),
    row(&["Pascal"], Some("pascal"), Some("//"), Some(("{", "}"))),
    row(&["VimL"], Some("vim"), Some("\""), None),
    row(&["JSON"], Some("json"), None, None),
    row(&["Protocol Buffer", "Protocol Buffer (TEXT)"], Some("proto"), Some("//"), C_BLOCK),
    row(&["GraphQL"], Some("graphql"), Some("#"), None),
    row(&["Solidity"], Some("solidity"), Some("//"), C_BLOCK),
    row(&["WGSL", "GLSL"], Some("glsl"), Some("//"), C_BLOCK),
    row(&["Diff"], None, None, None),
];

fn languages() -> &'static Vec<Language> {
    static LANGS: OnceLock<Vec<Language>> = OnceLock::new();
    LANGS.get_or_init(|| {
        let mut out = Vec::new();
        out.push(Language {
            name: "Plain Text".into(),
            lsp_id: None,
            line_comment: None,
            block_comment: None,
            indent_after: &['{', '[', '('],
            auto_quotes: &['"'],
        });
        // Python gets its own row: it indents after a colon.
        let py = Row { names: &["Python"], lsp: Some("python"), line: Some("#"), block: None, colon: true };
        let mut names: Vec<&str> = syntax_set().syntaxes().iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        for name in names {
            if name == "Plain Text" {
                continue;
            }
            let row = if py.names.contains(&name) {
                Some(&py)
            } else {
                TABLE.iter().find(|r| r.names.contains(&name))
            };
            let quotes: &'static [char] = match name {
                "Rust" => &['"'],
                "Lisp" | "Emacs Lisp" | "Clojure" | "Scheme" => &['"'],
                _ => &['"', '\'', '`'],
            };
            out.push(Language {
                name: name.to_string(),
                lsp_id: row.and_then(|r| r.lsp),
                line_comment: row.and_then(|r| r.line),
                block_comment: row.and_then(|r| r.block),
                indent_after: if row.is_some_and(|r| r.colon) { &[':', '{', '[', '('] } else { &['{', '[', '('] },
                auto_quotes: quotes,
            });
        }
        out
    })
}

pub fn plain() -> &'static Language {
    &languages()[0]
}

pub fn all() -> &'static [Language] {
    languages()
}

pub fn by_name(name: &str) -> &'static Language {
    languages().iter().find(|l| l.name.eq_ignore_ascii_case(name)).unwrap_or_else(plain)
}

/// The programming language most of these files are in, for a new file in that folder. Notes,
/// data and configuration files don't count: a C project with a README is a C project.
pub fn dominant<'a>(paths: impl IntoIterator<Item = &'a str>) -> Option<&'static Language> {
    const NOT_CODE: &[&str] = &[
        "Plain Text", "Markdown", "MultiMarkdown", "JSON", "YAML", "TOML", "XML", "INI", "CSV", "TSV", "Makefile",
        "CMake", "Diff", "Git Ignore", "Git Attributes", "Git Config", "Git Commit", "reStructuredText", "LaTeX",
        "Dockerfile", "Properties", "SVG",
    ];
    let mut counts: Vec<(&'static Language, usize)> = Vec::new();
    for p in paths.into_iter().take(20_000) {
        let path = Path::new(p);
        if path.extension().is_none() {
            continue;
        }
        let l = detect(path, "");
        if std::ptr::eq(l, plain()) || NOT_CODE.iter().any(|n| l.name.eq_ignore_ascii_case(n)) {
            continue;
        }
        match counts.iter_mut().find(|(k, _)| std::ptr::eq(*k, l)) {
            Some((_, n)) => *n += 1,
            None => counts.push((l, 1)),
        }
    }
    // The first seen wins a tie: max_by_key would take the last.
    counts.into_iter().fold(None, |best: Option<(&'static Language, usize)>, (l, n)| match best {
        Some((_, b)) if b >= n => best,
        _ => Some((l, n)),
    }).map(|(l, _)| l)
}

/// Works out the language from the file name, or failing that its first line (a shebang, say).
pub fn detect(path: &Path, first_line: &str) -> &'static Language {
    let ss = syntax_set();
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    // Cases the grammars leave out or get wrong.
    let special = match (name, ext) {
        ("Cargo.lock", _) | ("uv.lock", _) | ("poetry.lock", _) => Some("TOML"),
        ("Justfile" | "justfile", _) => Some("Makefile"),
        (_, "h" | "hh" | "hpp" | "hxx") => Some("C++"),
        (_, "tsx") => Some("TypeScriptReact"),
        (_, "ts" | "mts" | "cts") => Some("TypeScript"),
        (_, "jsx") => Some("JSX"),
        (_, "mjs" | "cjs") => Some("JavaScript"),
        (_, "fish") => Some("fish"),
        (_, "svelte") => Some("Svelte"),
        (_, "vue") => Some("Vue Component"),
        _ => None,
    };
    if let Some(s) = special
        && ss.find_syntax_by_name(s).is_some()
    {
        return by_name(s);
    }
    let syntax = ss
        .find_syntax_by_token(name)
        .filter(|_| ext.is_empty())
        .or_else(|| ss.find_syntax_by_extension(name))
        .or_else(|| ss.find_syntax_by_extension(ext))
        .or_else(|| ss.find_syntax_by_first_line(first_line));
    match syntax {
        Some(s) => by_name(&s.name),
        None => plain(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_of_c_files_is_a_c_folder() {
        let files = ["README.md", "prova.c", "exerc1.c", "exerc2.c", "util.h", ".vscode/settings.json", "Makefile"];
        assert_eq!(dominant(files).map(|l| l.name.as_str()), Some("C"));
        assert_eq!(dominant(["notes.md", "data.json"]).map(|l| l.name.as_str()), None);
        assert_eq!(dominant(["a.py", "b.rs"]).map(|l| l.name.as_str()), Some("Python"));
    }

    #[test]
    fn detects_common_languages() {
        for (file, want) in [
            ("a.rs", "Rust"),
            ("a.py", "Python"),
            ("a.toml", "TOML"),
            ("Cargo.lock", "TOML"),
            ("a.ts", "TypeScript"),
            ("a.tsx", "TypeScriptReact"),
            ("a.c", "C"),
            ("a.h", "C++"),
            ("Makefile", "Makefile"),
            ("Dockerfile", "Dockerfile"),
            ("a.md", "Markdown"),
            ("a.json", "JSON"),
            ("a.sh", "Bourne Again Shell (bash)"),
            ("a.go", "Go"),
            ("a.html", "HTML"),
            ("a.yml", "YAML"),
            ("a.nix", "Nix"),
            ("a.xyz123", "Plain Text"),
        ] {
            assert_eq!(detect(Path::new(file), "").name, want, "{file}");
        }
    }

    #[test]
    fn detects_shebang() {
        assert_eq!(detect(Path::new("script"), "#!/usr/bin/env python3\n").name, "Python");
    }

    #[test]
    fn every_table_row_with_comments_resolves_for_core_languages() {
        for n in ["Rust", "Python", "C", "C++", "Go", "JavaScript", "TypeScript", "Lua", "Bourne Again Shell (bash)"] {
            let l = by_name(n);
            assert_eq!(l.name, n);
            assert!(l.line_comment.is_some(), "{n}");
            assert!(l.lsp_id.is_some(), "{n}");
        }
    }
}
