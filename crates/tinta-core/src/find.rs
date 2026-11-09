//! Search within a document and across a folder.

use crate::buffer::Document;
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use regex::{Regex, RegexBuilder};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub pattern: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

impl Query {
    pub fn compile(&self) -> Result<Regex, String> {
        let mut p = if self.regex { self.pattern.clone() } else { regex::escape(&self.pattern) };
        if self.whole_word {
            p = format!(r"\b(?:{p})\b");
        }
        RegexBuilder::new(&p)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .build()
            .map_err(|e| e.to_string())
    }
}

/// All matches as char ranges. Stops at `limit` so a one-letter query stays responsive.
pub fn find_all(doc: &Document, re: &Regex, limit: usize) -> Vec<(usize, usize)> {
    let text = doc.rope.to_string();
    find_in_str(&text, re, limit)
}

fn find_in_str(text: &str, re: &Regex, limit: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    // Matches come in order: chars are counted on from the last one instead of looked up.
    let (mut byte, mut ch) = (0usize, 0usize);
    for m in re.find_iter(text) {
        if m.start() == m.end() {
            continue;
        }
        let start = ch + text[byte..m.start()].chars().count();
        let end = start + text[m.start()..m.end()].chars().count();
        (byte, ch) = (m.end(), end);
        out.push((start, end));
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Index of the first match at or after `pos`, wrapping to the start.
pub fn next_match(matches: &[(usize, usize)], pos: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    Some(matches.partition_point(|m| m.0 < pos) % matches.len())
}

/// Index of the last match ending at or before `pos`, wrapping to the end.
pub fn prev_match(matches: &[(usize, usize)], pos: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let i = matches.partition_point(|m| m.1 <= pos);
    Some(if i == 0 { matches.len() - 1 } else { i - 1 })
}

/// Turns `\n` and `\t` in a replacement into the real thing and leaves `$1` to the regex.
pub fn unescape_replacement(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some(o) => {
                    out.push('\\');
                    out.push(o);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The text replacing one match. Plain queries take the replacement literally.
pub fn expand(re: &Regex, query: &Query, matched: &str, replacement: &str) -> String {
    if !query.regex {
        return replacement.to_string();
    }
    let rep = unescape_replacement(replacement);
    match re.captures(matched) {
        Some(caps) => {
            let mut out = String::new();
            caps.expand(&rep, &mut out);
            out
        }
        None => rep,
    }
}

// Across files --------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    pub line: usize,
    /// Char columns.
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub matches: Vec<LineMatch>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchScope {
    /// Comma-separated globs to keep.
    pub include: String,
    /// Comma-separated globs to leave out.
    pub exclude: String,
    pub hidden: bool,
}

const MAX_FILE: u64 = 16 * 1024 * 1024;

fn walker(root: &Path, scope: &SearchScope) -> Result<ignore::Walk, String> {
    let mut b = WalkBuilder::new(root);
    b.hidden(!scope.hidden).git_ignore(true).git_global(true).parents(true);
    b.filter_entry(|e| e.file_name() != ".git");
    let mut ov = OverrideBuilder::new(root);
    let mut any = false;
    for glob in scope.include.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        ov.add(glob).map_err(|e| e.to_string())?;
        any = true;
    }
    for glob in scope.exclude.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        ov.add(&format!("!{glob}")).map_err(|e| e.to_string())?;
        any = true;
    }
    if any {
        b.overrides(ov.build().map_err(|e| e.to_string())?);
    }
    Ok(b.build())
}

/// Walks the folder (honouring .gitignore), handing each file with matches to `sink`.
pub fn search_files(
    root: &Path,
    query: &Query,
    scope: &SearchScope,
    cancel: &Arc<AtomicBool>,
    limit: usize,
    mut sink: impl FnMut(FileMatches),
) -> Result<usize, String> {
    let re = query.compile()?;
    let mut total = 0;
    for entry in walker(root, scope)?.flatten() {
        if cancel.load(Ordering::Relaxed) || total >= limit {
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.metadata().map(|m| m.len() > MAX_FILE).unwrap_or(true) {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else { continue };
        if bytes.iter().take(8192).any(|b| *b == 0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else { continue };
        let mut matches = Vec::new();
        for (ln, line) in text.lines().enumerate() {
            for m in re.find_iter(line) {
                if m.start() == m.end() {
                    continue;
                }
                let start = line[..m.start()].chars().count();
                let end = start + line[m.start()..m.end()].chars().count();
                let shown: String = line.chars().take(400).collect();
                matches.push(LineMatch { line: ln, start, end, text: shown });
                total += 1;
            }
        }
        if !matches.is_empty() {
            sink(FileMatches { path: entry.path().to_path_buf(), matches });
        }
    }
    Ok(total)
}

/// Replaces in a file on disk. Returns how many replacements were made.
pub fn replace_in_file(path: &Path, query: &Query, replacement: &str) -> Result<usize, String> {
    let re = query.compile()?;
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let n = re.find_iter(&text).filter(|m| m.start() != m.end()).count();
    if n == 0 {
        return Ok(0);
    }
    let out = if query.regex {
        re.replace_all(&text, unescape_replacement(replacement).as_str()).into_owned()
    } else {
        re.replace_all(&text, regex::NoExpand(replacement)).into_owned()
    };
    crate::buffer::atomic_write(path, out.as_bytes()).map_err(|e| e.to_string())?;
    Ok(n)
}

/// Files under a folder, relative to it, for quick open.
pub fn list_files(root: &Path, hidden: bool, cancel: &AtomicBool, limit: usize) -> Vec<String> {
    let scope = SearchScope { hidden, ..Default::default() };
    let Ok(walk) = walker(root, &scope) else { return Vec::new() };
    let mut out = Vec::new();
    for e in walk.flatten() {
        if cancel.load(Ordering::Relaxed) || out.len() >= limit {
            break;
        }
        if e.file_type().is_some_and(|t| t.is_file())
            && let Ok(rel) = e.path().strip_prefix(root)
        {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
    out.sort_unstable();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(p: &str) -> Query {
        Query { pattern: p.into(), ..Default::default() }
    }

    #[test]
    fn literal_query_is_escaped_and_case_folds() {
        let d = Document::new("a.b axb A.B\n");
        let re = q("a.b").compile().unwrap();
        assert_eq!(find_all(&d, &re, 100), vec![(0, 3), (8, 11)]);
    }

    #[test]
    fn options_change_matches() {
        let d = Document::new("foo foobar Foo\n");
        let mut qq = q("foo");
        qq.whole_word = true;
        assert_eq!(find_all(&d, &qq.compile().unwrap(), 100).len(), 2);
        qq.case_sensitive = true;
        assert_eq!(find_all(&d, &qq.compile().unwrap(), 100).len(), 1);
    }

    #[test]
    fn bad_regex_is_an_error_not_a_panic() {
        let mut qq = q("(");
        qq.regex = true;
        assert!(qq.compile().is_err());
    }

    #[test]
    fn offsets_are_chars_for_non_ascii() {
        let d = Document::new("ação ação\n");
        let re = q("ção").compile().unwrap();
        assert_eq!(find_all(&d, &re, 100), vec![(1, 4), (6, 9)]);
    }

    #[test]
    fn next_and_prev_wrap() {
        let m = [(0, 1), (5, 6), (9, 10)];
        assert_eq!(next_match(&m, 2), Some(1));
        assert_eq!(next_match(&m, 10), Some(0));
        assert_eq!(prev_match(&m, 5), Some(0));
        assert_eq!(prev_match(&m, 0), Some(2));
        assert_eq!(next_match(&[], 0), None);
    }

    #[test]
    fn replacement_expands_groups_only_in_regex_mode() {
        let mut qq = q(r"(\w+)@(\w+)");
        qq.regex = true;
        let re = qq.compile().unwrap();
        assert_eq!(expand(&re, &qq, "a@b", "$2 at $1\\n"), "b at a\n");
        let plain = q("x");
        let re = plain.compile().unwrap();
        assert_eq!(expand(&re, &plain, "x", "$1"), "$1");
    }

    #[test]
    fn searches_a_folder_respecting_gitignore_and_globs() {
        let dir = std::env::temp_dir().join(format!("tinta-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.txt\n").unwrap();
        std::fs::write(dir.join("ignored.txt"), "needle\n").unwrap();
        std::fs::write(dir.join("src/a.rs"), "x\nprefix needle ação needle\n").unwrap();
        std::fs::write(dir.join("b.txt"), "needle\n").unwrap();
        std::fs::write(dir.join("bin"), b"needle\0\0").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let mut got = Vec::new();
        let n = search_files(&dir, &q("needle"), &SearchScope::default(), &cancel, 1000, |f| got.push(f)).unwrap();
        assert_eq!(n, 3);
        assert_eq!(got.len(), 2);
        let a = got.iter().find(|f| f.path.ends_with("src/a.rs")).unwrap();
        assert_eq!(a.matches[0].line, 1);
        assert_eq!((a.matches[1].start, a.matches[1].end), (19, 25));

        let scope = SearchScope { include: "*.rs".into(), ..Default::default() };
        let mut got = Vec::new();
        search_files(&dir, &q("needle"), &scope, &cancel, 1000, |f| got.push(f)).unwrap();
        assert_eq!(got.len(), 1);
        let scope = SearchScope { exclude: "*.rs".into(), ..Default::default() };
        let mut got = Vec::new();
        search_files(&dir, &q("needle"), &scope, &cancel, 1000, |f| got.push(f)).unwrap();
        assert!(got.iter().all(|f| !f.path.ends_with("a.rs")));

        let files = list_files(&dir, false, &AtomicBool::new(false), 100);
        assert!(files.contains(&"src/a.rs".to_string()) && !files.contains(&"ignored.txt".to_string()));

        assert_eq!(replace_in_file(&dir.join("b.txt"), &q("needle"), "$1").unwrap(), 1);
        assert_eq!(std::fs::read_to_string(dir.join("b.txt")).unwrap(), "$1\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
