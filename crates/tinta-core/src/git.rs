//! Git, through the `git` command: no libgit2 to link, and it respects the user's own config.

use similar::{ChangeTag, DiffTag, TextDiff};
use std::path::{Path, PathBuf};
use std::process::Stdio;

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let out = crate::host::command("git", args, Some(root), &[("GIT_OPTIONAL_LOCKS", "0"), ("GIT_TERMINAL_PROMPT", "0")])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn repo_root(path: &Path) -> Option<PathBuf> {
    let dir = if path.is_dir() { path } else { path.parent()? };
    git(dir, &["rev-parse", "--show-toplevel"]).ok().map(|s| PathBuf::from(s.trim()))
}

pub fn branch(root: &Path) -> Option<String> {
    let b = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]).ok();
    match b.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
        Some(s) => Some(s),
        None => git(root, &["rev-parse", "--short", "HEAD"]).ok().map(|s| s.trim().to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    /// Relative to the repository's root.
    pub path: String,
    pub index: char,
    pub worktree: char,
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        !matches!(self.index, ' ' | '?' | '!')
    }
    pub fn is_unstaged(&self) -> bool {
        self.worktree != ' '
    }
    pub fn is_conflict(&self) -> bool {
        self.index == 'U' || self.worktree == 'U' || (self.index == 'A' && self.worktree == 'A') || (self.index == 'D' && self.worktree == 'D')
    }
}

pub fn status(root: &Path) -> Result<Vec<StatusEntry>, String> {
    let out = git(root, &["status", "--porcelain=v1", "-z", "--untracked-files=all"])?;
    Ok(parse_status(&out))
}

fn parse_status(out: &str) -> Vec<StatusEntry> {
    let mut entries = Vec::new();
    let mut it = out.split('\0');
    while let Some(item) = it.next() {
        if item.len() < 4 {
            continue;
        }
        let mut chars = item.chars();
        let index = chars.next().unwrap();
        let worktree = chars.next().unwrap();
        entries.push(StatusEntry { path: item[3..].to_string(), index, worktree });
        // Renames carry the old name as an extra field.
        if index == 'R' || index == 'C' {
            it.next();
        }
    }
    entries
}

pub fn file_at_head(root: &Path, rel: &str) -> Option<String> {
    git(root, &["show", &format!("HEAD:{rel}")]).ok()
}

/// What's staged for a file. Falls back on nothing for a file git doesn't know yet.
pub fn file_at_index(root: &Path, rel: &str) -> Option<String> {
    git(root, &["show", &format!(":{rel}")]).ok()
}

pub fn stage(root: &Path, rel: &str) -> Result<(), String> {
    git(root, &["add", "--", rel]).map(|_| ())
}

pub fn unstage(root: &Path, rel: &str) -> Result<(), String> {
    git(root, &["restore", "--staged", "--", rel]).map(|_| ())
}

pub fn stage_all(root: &Path) -> Result<(), String> {
    git(root, &["add", "-A"]).map(|_| ())
}

/// Throws away changes to a file. Untracked files go too.
pub fn discard(root: &Path, entry: &StatusEntry) -> Result<(), String> {
    if entry.index == '?' {
        return std::fs::remove_file(root.join(&entry.path)).map_err(|e| e.to_string());
    }
    git(root, &["restore", "--worktree", "--", &entry.path]).map(|_| ())
}

pub fn commit(root: &Path, message: &str) -> Result<String, String> {
    git(root, &["commit", "-m", message])
}

pub fn push(root: &Path) -> Result<String, String> {
    git(root, &["push"])
}

pub fn pull(root: &Path) -> Result<String, String> {
    git(root, &["pull", "--ff-only"])
}

pub fn fetch(root: &Path) -> Result<String, String> {
    git(root, &["fetch"])
}

pub fn branches(root: &Path) -> Result<Vec<String>, String> {
    let out = git(root, &["branch", "--format=%(refname:short)"])?;
    Ok(out.lines().map(str::to_string).collect())
}

pub fn checkout(root: &Path, branch: &str) -> Result<(), String> {
    git(root, &["checkout", branch]).map(|_| ())
}

pub fn create_branch(root: &Path, branch: &str) -> Result<(), String> {
    git(root, &["checkout", "-b", branch]).map(|_| ())
}

pub fn diff(root: &Path, rel: &str, staged: bool) -> Result<String, String> {
    if staged {
        git(root, &["diff", "--cached", "--", rel])
    } else {
        git(root, &["diff", "--", rel])
    }
}

pub fn log(root: &Path, limit: usize) -> Result<String, String> {
    git(root, &["log", "--graph", "--date=short", &format!("-{limit}"), "--format=%h %ad %an  %s%d"])
}

/// Ahead and behind counts relative to the upstream.
pub fn ahead_behind(root: &Path) -> Option<(usize, usize)> {
    let out = git(root, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).ok()?;
    let mut it = out.split_whitespace();
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    pub date: String,
    pub summary: String,
}

/// Blame for one line, 0-based. Uncommitted lines come back as `None`.
pub fn blame_line(root: &Path, rel: &str, line: usize) -> Option<BlameLine> {
    let range = format!("{},{}", line + 1, line + 1);
    let out = git(root, &["blame", "--porcelain", "--date=short", "-L", &range, "--", rel]).ok()?;
    let mut lines = out.lines();
    let hash = lines.next()?.split_whitespace().next()?.to_string();
    if hash.bytes().all(|b| b == b'0') {
        return None;
    }
    let mut author = String::new();
    let mut time = 0i64;
    let mut summary = String::new();
    for l in lines {
        if let Some(a) = l.strip_prefix("author ") {
            author = a.to_string();
        } else if let Some(t) = l.strip_prefix("author-time ") {
            time = t.parse().unwrap_or(0);
        } else if let Some(s) = l.strip_prefix("summary ") {
            summary = s.to_string();
        }
    }
    Some(BlameLine { hash: hash[..8.min(hash.len())].to_string(), author, date: civil_date(time), summary })
}

/// `YYYY-MM-DD` from Unix time, without a date crate.
fn civil_date(secs: i64) -> String {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMark {
    Added,
    Modified,
    /// Lines were removed just above this one.
    Deleted,
}

/// A changed region: line ranges in the base text and the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub old: std::ops::Range<usize>,
    pub new: std::ops::Range<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineDiff {
    /// (line, mark), ascending.
    pub marks: Vec<(u32, LineMark)>,
    pub hunks: Vec<Hunk>,
}

impl LineDiff {
    pub fn mark_at(&self, line: usize) -> Option<LineMark> {
        let i = self.marks.binary_search_by_key(&(line as u32), |m| m.0).ok()?;
        Some(self.marks[i].1)
    }

    pub fn next_hunk(&self, line: usize) -> Option<&Hunk> {
        self.hunks.iter().find(|h| h.new.start > line).or(self.hunks.first())
    }

    pub fn prev_hunk(&self, line: usize) -> Option<&Hunk> {
        self.hunks.iter().rev().find(|h| h.new.start < line).or(self.hunks.last())
    }

    pub fn hunk_at(&self, line: usize) -> Option<&Hunk> {
        self.hunks.iter().find(|h| h.new.contains(&line) || (h.new.is_empty() && h.new.start == line))
    }
}

/// What changed between the committed file and the buffer, for the gutter.
pub fn line_diff(base: &str, current: &str) -> LineDiff {
    // Runs on the UI thread while typing pauses: a file rewritten from top to bottom gets a
    // rougher answer instead of a stall.
    let diff = TextDiff::configure()
        .deadline(std::time::Instant::now() + std::time::Duration::from_millis(150))
        .diff_lines(base, current);
    let mut out = LineDiff::default();
    for op in diff.ops() {
        let (old, new) = (op.old_range(), op.new_range());
        match op.tag() {
            DiffTag::Equal => {}
            DiffTag::Insert => {
                out.marks.extend(new.clone().map(|l| (l as u32, LineMark::Added)));
                out.hunks.push(Hunk { old, new });
            }
            DiffTag::Delete => {
                out.marks.push((new.start as u32, LineMark::Deleted));
                out.hunks.push(Hunk { old, new });
            }
            DiffTag::Replace => {
                out.marks.extend(new.clone().map(|l| (l as u32, LineMark::Modified)));
                out.hunks.push(Hunk { old, new });
            }
        }
    }
    out.marks.sort_by_key(|m| m.0);
    out.marks.dedup_by_key(|m| m.0);
    out
}

/// Base text of a hunk, to put back when reverting it.
pub fn hunk_base_text(base: &str, hunk: &Hunk) -> String {
    let mut s = String::new();
    for l in base.split_inclusive('\n').skip(hunk.old.start).take(hunk.old.len()) {
        s.push_str(l);
    }
    s
}

/// A readable diff of two texts, used by "compare with saved".
pub fn unified(old_name: &str, new_name: &str, old: &str, new: &str) -> String {
    let diff = TextDiff::from_lines(old, new);
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    for group in diff.grouped_ops(3) {
        let first = &group[0];
        let last = &group[group.len() - 1];
        out.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            first.old_range().start + 1,
            last.old_range().end - first.old_range().start,
            first.new_range().start + 1,
            last.new_range().end - first.new_range().start
        ));
        for op in group {
            for change in diff.iter_changes(&op) {
                let sign = match change.tag() {
                    ChangeTag::Delete => '-',
                    ChangeTag::Insert => '+',
                    ChangeTag::Equal => ' ',
                };
                out.push(sign);
                out.push_str(change.value());
                if !change.value().ends_with('\n') {
                    out.push('\n');
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_with_renames() {
        let out = " M a.rs\0?? b.rs\0R  new.rs\0old.rs\0MM c d.rs\0";
        let e = parse_status(out);
        assert_eq!(e.len(), 4);
        assert_eq!(e[0], StatusEntry { path: "a.rs".into(), index: ' ', worktree: 'M' });
        assert!(e[0].is_unstaged() && !e[0].is_staged());
        assert_eq!(e[2].path, "new.rs");
        assert!(e[2].is_staged());
        assert_eq!(e[3].path, "c d.rs");
    }

    #[test]
    fn line_diff_marks_each_kind() {
        let d = line_diff("a\nb\nc\nd\n", "a\nB\nc\nnew\nd\n");
        assert_eq!(d.mark_at(1), Some(LineMark::Modified));
        assert_eq!(d.mark_at(3), Some(LineMark::Added));
        assert_eq!(d.mark_at(0), None);
        assert_eq!(d.hunks.len(), 2);
        let d = line_diff("a\nb\nc\n", "a\nc\n");
        assert_eq!(d.mark_at(1), Some(LineMark::Deleted));
        assert_eq!(hunk_base_text("a\nb\nc\n", &d.hunks[0]), "b\n");
    }

    #[test]
    fn a_file_rewritten_throughout_does_not_stall_the_gutter() {
        let base: String = (0..60_000).map(|i| format!("line {i}\n")).collect();
        let current: String = (0..60_000).map(|i| format!("other {}\n", i * 7 % 60_000)).collect();
        let t = std::time::Instant::now();
        let d = line_diff(&base, &current);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
        assert!(!d.marks.is_empty());
        // Small changes still come out exact.
        let mut one = base.clone();
        one.insert_str(base.find("line 30000").unwrap(), "new\n");
        let d = line_diff(&base, &one);
        assert_eq!(d.marks, vec![(30_000, LineMark::Added)]);
    }

    #[test]
    fn hunk_navigation_wraps() {
        let d = line_diff("a\nb\nc\nd\ne\nf\n", "A\nb\nc\nd\ne\nF\n");
        assert_eq!(d.next_hunk(0).unwrap().new.start, 5);
        assert_eq!(d.next_hunk(5).unwrap().new.start, 0);
        assert_eq!(d.prev_hunk(5).unwrap().new.start, 0);
        assert_eq!(d.prev_hunk(0).unwrap().new.start, 5);
    }

    #[test]
    fn date_conversion() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(1_700_000_000), "2023-11-14");
    }

    #[test]
    fn real_repository_flow() {
        let dir = std::env::temp_dir().join(format!("tinta-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let g = |a: &[&str]| git(&dir, a).unwrap();
        g(&["init", "-q", "-b", "main"]);
        g(&["config", "user.email", "t@example.com"]);
        g(&["config", "user.name", "T"]);
        std::fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
        stage(&dir, "a.txt").unwrap();
        commit(&dir, "first").unwrap();
        assert_eq!(branch(&dir).as_deref(), Some("main"));
        assert_eq!(file_at_head(&dir, "a.txt").as_deref(), Some("one\ntwo\n"));
        let b = blame_line(&dir, "a.txt", 1).unwrap();
        assert_eq!(b.summary, "first");
        assert_eq!(b.author, "T");

        std::fs::write(dir.join("a.txt"), "one\nTWO\n").unwrap();
        std::fs::write(dir.join("new.txt"), "x\n").unwrap();
        let st = status(&dir).unwrap();
        assert_eq!(st.len(), 2);
        assert!(diff(&dir, "a.txt", false).unwrap().contains("+TWO"));
        stage(&dir, "a.txt").unwrap();
        assert!(status(&dir).unwrap().iter().any(|e| e.path == "a.txt" && e.is_staged()));
        unstage(&dir, "a.txt").unwrap();
        let st = status(&dir).unwrap();
        let entry = st.iter().find(|e| e.path == "a.txt").unwrap();
        assert!(!entry.is_staged());
        discard(&dir, entry).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "one\ntwo\n");
        discard(&dir, st.iter().find(|e| e.path == "new.txt").unwrap()).unwrap();
        assert!(status(&dir).unwrap().is_empty());
        create_branch(&dir, "dev").unwrap();
        assert_eq!(branches(&dir).unwrap().len(), 2);
        checkout(&dir, "main").unwrap();
        assert!(log(&dir, 5).unwrap().contains("first"));
        assert!(repo_root(&dir.join("a.txt")).is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unified_diff_has_headers_and_changes() {
        let u = unified("a", "b", "x\ny\n", "x\nz\n");
        assert!(u.contains("--- a") && u.contains("-y") && u.contains("+z") && u.contains("@@"));
    }
}
