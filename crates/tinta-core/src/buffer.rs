//! A document: the rope, its undo history and how it maps to a file on disk.

use crate::cursor::{Cursors, Selection};
use crate::language::{self, Language};
use crate::syntax::Highlighter;
use encoding_rs::Encoding;
use ropey::{Rope, RopeSlice};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Indent {
    pub spaces: bool,
    pub width: usize,
}

impl Indent {
    pub fn unit(self) -> String {
        if self.spaces { " ".repeat(self.width.max(1)) } else { "\t".into() }
    }
}

/// A replacement expressed in the coordinates of the text before the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

impl Edit {
    pub fn new(start: usize, end: usize, text: impl Into<String>) -> Self {
        Self { start, end, text: text.into() }
    }
    pub fn insert(at: usize, text: impl Into<String>) -> Self {
        Self::new(at, at, text)
    }
    pub fn delete(start: usize, end: usize) -> Self {
        Self::new(start, end, "")
    }
}

/// Which way a position inside or at the edge of an edit goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Before,
    After,
}

/// Translates offsets from the old text to the new one.
#[derive(Debug, Clone, Default)]
pub struct EditMap {
    /// (start, old_len, new_len), ascending.
    items: Vec<(usize, usize, usize)>,
}

impl EditMap {
    pub fn map(&self, pos: usize, assoc: Assoc) -> usize {
        let mut delta: isize = 0;
        for &(start, old, new) in &self.items {
            let end = start + old;
            if pos < start {
                break;
            }
            let inside_or_edge = pos < end || (pos == end && old == 0) || (pos == end && pos == start);
            if inside_or_edge {
                let base = (start as isize + delta) as usize;
                return match assoc {
                    Assoc::Before => base,
                    Assoc::After => base + new,
                };
            }
            delta += new as isize - old as isize;
        }
        (pos as isize + delta) as usize
    }

    /// Position after the text inserted by the edit number `i`, for carets that follow typed text.
    pub fn after_edit(&self, i: usize) -> usize {
        let delta: isize =
            self.items[..i].iter().map(|&(_, old, new)| new as isize - old as isize).sum();
        let (start, _, new) = self.items[i];
        (start as isize + delta) as usize + new
    }

    pub fn before_edit(&self, i: usize) -> usize {
        self.after_edit(i) - self.items[i].2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Debug, Clone)]
struct Change {
    at: usize,
    removed: String,
    inserted: String,
}

#[derive(Debug, Clone)]
struct Transaction {
    changes: Vec<Change>,
    before: Cursors,
    after: Cursors,
    kind: EditKind,
    when: Instant,
    state: u64,
    prev_state: u64,
}

/// A change as LSP describes it: positions in the text just before it, in UTF-16 columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspChange {
    pub start: crate::lsp::Pos,
    pub end: crate::lsp::Pos,
    pub text: String,
}

/// Something that changed in the text; other views and overlays translate themselves with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChangeRec {
    pub at: usize,
    pub removed: usize,
    pub inserted: usize,
    pub start_line: usize,
    pub removed_lines: usize,
    pub inserted_lines: usize,
    /// The edit began at the start of its line.
    pub starts_line: bool,
    /// What it removed ended exactly at the start of a line.
    pub ends_at_line_start: bool,
    /// ...or at the end of one, so nothing of that line is left.
    pub ends_at_line_end: bool,
    pub inserted_ends_newline: bool,
}

impl ChangeRec {
    /// Where the code on a line went, for marks that belong to that code. `None`: it's gone.
    pub fn map_line(&self, line: usize) -> Option<usize> {
        let (s, r, i) = (self.start_line, self.removed_lines, self.inserted_lines);
        if line < s {
            return Some(line);
        }
        if line > s + r {
            return Some(line + i - r);
        }
        if r == 0 {
            // Enter at the very start of the line pushes it down; anywhere else it stays.
            return Some(if self.starts_line && self.removed == 0 && self.inserted_ends_newline { line + i } else { line });
        }
        if line == s + r {
            // Backspace at the start of a line joins it to the one above, and it lives on there.
            return if self.ends_at_line_start || !self.ends_at_line_end { Some(s + i) } else { None };
        }
        // The head of the first line survives, unless the edit started at its first char.
        (line == s && !self.starts_line).then_some(s)
    }
}

pub struct Document {
    pub rope: Rope,
    pub path: Option<PathBuf>,
    pub encoding: &'static Encoding,
    pub has_bom: bool,
    pub eol: LineEnding,
    pub indent: Indent,
    pub language: &'static Language,
    pub readonly: bool,
    pub disk_mtime: Option<SystemTime>,
    pub revision: u64,
    pub highlighter: Option<Highlighter>,
    pub structure: Option<crate::structure::Structure>,
    /// Keep a record of changes for a language server that takes them one at a time.
    pub track_lsp: bool,
    lsp_log: Vec<LspChange>,
    lsp_overflow: bool,
    /// Display name for buffers with no file.
    pub scratch_name: String,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    state: u64,
    next_state: u64,
    saved_state: Option<u64>,
    log: Vec<ChangeRec>,
    /// Stops coalescing after a cursor move, so undo does not swallow two separate edits.
    break_coalescing: bool,
    /// Everything until `end_group` undoes as one step.
    group_open: bool,
    group_state: Option<u64>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document").field("path", &self.path).field("rev", &self.revision).finish()
    }
}

pub fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
}

impl Document {
    pub fn new(text: &str) -> Self {
        let norm = normalize_newlines(text);
        let rope = Rope::from_str(&norm);
        Self {
            rope,
            path: None,
            encoding: encoding_rs::UTF_8,
            has_bom: false,
            eol: if text.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf },
            indent: Indent { spaces: true, width: 4 },
            language: language::plain(),
            readonly: false,
            disk_mtime: None,
            revision: 0,
            highlighter: None,
            structure: None,
            track_lsp: false,
            lsp_log: Vec::new(),
            lsp_overflow: false,
            scratch_name: String::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            state: 0,
            next_state: 1,
            saved_state: Some(0),
            log: Vec::new(),
            break_coalescing: false,
            group_open: false,
            group_state: None,
        }
    }

    /// Reads a file, working out encoding, line endings, language and indentation.
    pub fn open(path: &Path, default_indent: Indent) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        let (text, encoding, has_bom) = decode(&bytes)?;
        let mut doc = Self::new(&text);
        doc.encoding = encoding;
        doc.has_bom = has_bom;
        doc.path = Some(path.to_path_buf());
        doc.language = language::detect(path, doc.rope.line(0).as_str().unwrap_or(""));
        doc.indent = detect_indent(&doc.rope).unwrap_or(default_indent);
        let meta = std::fs::metadata(path)?;
        doc.readonly = meta.permissions().readonly();
        doc.disk_mtime = meta.modified().ok();
        Ok(doc)
    }

    pub fn file_name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            None if self.scratch_name.is_empty() => "Untitled".into(),
            None => self.scratch_name.clone(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_state != Some(self.state)
    }

    pub fn mark_saved(&mut self) {
        self.saved_state = Some(self.state);
    }

    /// The text as it goes to disk: encoding, BOM and line endings restored.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut text = self.rope.to_string();
        if self.eol == LineEnding::CrLf {
            text = text.replace('\n', "\r\n");
        }
        let mut out = Vec::with_capacity(text.len() + 3);
        if self.encoding == encoding_rs::UTF_8 {
            if self.has_bom {
                out.extend_from_slice(b"\xEF\xBB\xBF");
            }
            out.extend_from_slice(text.as_bytes());
        } else if self.encoding == encoding_rs::UTF_16LE || self.encoding == encoding_rs::UTF_16BE {
            // encoding_rs never encodes to UTF-16 on its own.
            let be = self.encoding == encoding_rs::UTF_16BE;
            out.extend_from_slice(if be { b"\xFE\xFF" } else { b"\xFF\xFE" });
            for u in text.encode_utf16() {
                out.extend_from_slice(&if be { u.to_be_bytes() } else { u.to_le_bytes() });
            }
        } else {
            let (bytes, _, _) = self.encoding.encode(&text);
            out.extend_from_slice(&bytes);
        }
        out
    }

    /// Writes through a temporary file so a crash cannot leave half a file behind.
    pub fn save_to(&mut self, path: &Path) -> std::io::Result<()> {
        let bytes = self.to_bytes();
        atomic_write(path, &bytes)?;
        self.path = Some(path.to_path_buf());
        self.disk_mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        self.mark_saved();
        Ok(())
    }

    /// Replaces everything with the file's contents, as one undoable step.
    pub fn reload(&mut self, cursors: &mut Cursors) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else { return Ok(()) };
        let bytes = std::fs::read(&path)?;
        let (text, encoding, has_bom) = decode(&bytes)?;
        let text = normalize_newlines(&text).into_owned();
        self.encoding = encoding;
        self.has_bom = has_bom;
        self.replace_all(&text, cursors);
        self.mark_saved();
        self.disk_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Ok(())
    }

    /// Replaces the whole text with a minimal diff so cursors and undo survive formatters.
    pub fn replace_all(&mut self, text: &str, cursors: &mut Cursors) {
        let old = self.rope.to_string();
        if old == text {
            return;
        }
        let mut edits = Vec::new();
        let diff = similar::TextDiff::from_lines(old.as_str(), text);
        let mut old_chars = 0usize;
        for change in diff.ops() {
            let (old_range, new_range) = (change.old_range(), change.new_range());
            let n: usize = diff.old_slices()[old_range.clone()].iter().map(|l| l.chars().count()).sum();
            if change.tag() != similar::DiffTag::Equal {
                let new_text: String = diff.new_slices()[new_range].concat();
                edits.push(Edit::new(old_chars, old_chars + n, new_text));
            }
            old_chars += n;
        }
        self.apply(edits, cursors, EditKind::Other, |m, c| {
            c.map(|s| Selection::new(m.map(s.anchor, Assoc::Before), m.map(s.head, Assoc::Before)));
        });
    }

    // Text access --------------------------------------------------------------------------

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Chars in a line, without its newline.
    pub fn line_len(&self, line: usize) -> usize {
        // A line past the end (a stale location: a build error, a bookmark) is empty, not a crash.
        let Some(slice) = self.rope.get_line(line) else { return 0 };
        let n = slice.len_chars();
        if n > 0 && slice.char(n - 1) == '\n' { n - 1 } else { n }
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line.min(self.rope.len_lines()))
    }

    pub fn line_end(&self, line: usize) -> usize {
        self.line_start(line) + self.line_len(line)
    }

    pub fn char_to_line(&self, pos: usize) -> usize {
        self.rope.char_to_line(pos.min(self.rope.len_chars()))
    }

    /// (line, column in chars).
    pub fn line_col(&self, pos: usize) -> (usize, usize) {
        let pos = pos.min(self.rope.len_chars());
        let line = self.rope.char_to_line(pos);
        (line, pos - self.rope.line_to_char(line))
    }

    pub fn pos(&self, line: usize, col: usize) -> usize {
        if line >= self.len_lines() {
            return self.len_chars();
        }
        self.line_start(line) + col.min(self.line_len(line))
    }

    pub fn line_text(&self, line: usize) -> Cow<'_, str> {
        let slice = self.line_slice(line);
        match slice.as_str() {
            Some(s) => Cow::Borrowed(s),
            None => Cow::Owned(slice.to_string()),
        }
    }

    /// The line without its newline.
    pub fn line_slice(&self, line: usize) -> RopeSlice<'_> {
        let start = self.line_start(line);
        self.rope.slice(start..start + self.line_len(line))
    }

    pub fn slice_string(&self, start: usize, end: usize) -> String {
        self.rope.slice(start..end).to_string()
    }

    pub fn char_at(&self, pos: usize) -> Option<char> {
        (pos < self.rope.len_chars()).then(|| self.rope.char(pos))
    }

    /// Leading whitespace of a line.
    pub fn line_indent(&self, line: usize) -> String {
        self.line_slice(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    }

    pub fn line_is_blank(&self, line: usize) -> bool {
        self.line_slice(line).chars().all(char::is_whitespace)
    }

    // Editing ------------------------------------------------------------------------------

    pub fn state_id(&self) -> u64 {
        self.state
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Makes the edits until `end_group` one undo step, like everything typed in Vim's insert mode.
    pub fn begin_group(&mut self) {
        self.group_open = true;
        self.group_state = None;
        self.break_coalescing = true;
    }

    pub fn end_group(&mut self) {
        self.group_open = false;
        self.group_state = None;
        self.break_coalescing = true;
    }

    /// Next edit starts a new undo step.
    pub fn break_undo(&mut self) {
        self.break_coalescing = true;
    }

    /// The changes since the last call, in order. `None` when there were too many to be worth it,
    /// and the whole text should be sent instead.
    pub fn take_lsp_changes(&mut self) -> Option<Vec<LspChange>> {
        let overflow = std::mem::take(&mut self.lsp_overflow);
        let list = std::mem::take(&mut self.lsp_log);
        (!overflow).then_some(list)
    }

    /// Brings the syntax tree up to date with the text.
    pub fn sync_structure(&mut self) {
        let rev = self.revision;
        if let Some(st) = &mut self.structure {
            st.update(&self.rope, rev);
        }
    }

    /// Drains the record of what changed since the last call.
    pub fn take_log(&mut self) -> Vec<ChangeRec> {
        std::mem::take(&mut self.log)
    }

    fn raw_change(&mut self, at: usize, remove: usize, insert: &str) -> String {
        if self.track_lsp {
            // Past this many, sending the whole text is cheaper.
            if self.lsp_log.len() < 512 {
                let change = LspChange {
                    start: crate::lsp::to_lsp(&self.rope, at),
                    end: crate::lsp::to_lsp(&self.rope, at + remove),
                    text: insert.to_string(),
                };
                self.lsp_log.push(change);
            } else {
                self.lsp_overflow = true;
            }
        }
        let tree_edit = self.structure.as_ref().map(|_| crate::structure::EditStart::new(&self.rope, at, remove));
        let start_line = self.rope.char_to_line(at);
        let removed: String = self.rope.slice(at..at + remove).to_string();
        let removed_lines = removed.bytes().filter(|b| *b == b'\n').count();
        let inserted_lines = insert.bytes().filter(|b| *b == b'\n').count();
        let starts_line = self.rope.line_to_char(self.rope.char_to_line(at)) == at;
        let end = at + remove;
        let end_line = self.rope.char_to_line(end);
        let ends_at_line_start = self.rope.line_to_char(end_line) == end;
        let ends_at_line_end = end >= self.rope.len_chars() || self.rope.char(end) == '\n';
        if remove > 0 {
            self.rope.remove(at..at + remove);
        }
        if !insert.is_empty() {
            self.rope.insert(at, insert);
        }
        self.log.push(ChangeRec {
            starts_line,
            ends_at_line_start,
            ends_at_line_end,
            inserted_ends_newline: insert.ends_with('\n'),
            at,
            removed: remove,
            inserted: insert.chars().count(),
            start_line,
            removed_lines,
            inserted_lines,
        });
        if let Some(h) = &mut self.highlighter {
            h.on_change(start_line, removed_lines, inserted_lines);
        }
        if let (Some(st), Some(e)) = (&mut self.structure, tree_edit) {
            st.edit(e.finish(&self.rope, insert.len()));
        }
        self.revision += 1;
        removed
    }

    /// Applies edits (non-overlapping, any order) as one transaction. `after` receives a map of
    /// old to new offsets and the cursors to update.
    pub fn apply(
        &mut self,
        mut edits: Vec<Edit>,
        cursors: &mut Cursors,
        kind: EditKind,
        after: impl FnOnce(&EditMap, &mut Cursors),
    ) -> EditMap {
        // A range from stale positions (text that changed under a popup, a server's late reply)
        // is cut to the text instead of taking the editor down.
        let len = self.rope.len_chars();
        for e in &mut edits {
            e.end = e.end.min(len);
            e.start = e.start.min(e.end);
        }
        edits.sort_by_key(|e| (e.start, e.end));
        // Overlaps would make the later edits land in the wrong place.
        let mut reach = 0;
        edits.retain(|e| {
            let ok = e.start >= reach;
            if ok {
                reach = e.end;
            }
            ok
        });
        let before = cursors.clone();
        let map = EditMap {
            items: edits
                .iter()
                .map(|e| (e.start, e.end - e.start, e.text.chars().count()))
                .collect(),
        };
        let mut changes = Vec::with_capacity(edits.len());
        for e in edits.iter().rev() {
            if e.start == e.end && e.text.is_empty() {
                continue;
            }
            let removed = self.raw_change(e.start, e.end - e.start, &e.text);
            changes.push(Change { at: e.start, removed, inserted: e.text.clone() });
        }
        after(&map, cursors);
        cursors.clamp(self.rope.len_chars());
        if changes.is_empty() {
            return map;
        }
        self.redo.clear();
        let now = Instant::now();
        let single_char_edit = changes.len() == 1
            && changes[0].inserted.chars().count() <= 1
            && !changes[0].inserted.contains('\n')
            && changes[0].removed.chars().count() <= 1;
        let coalesce = !self.break_coalescing
            && kind != EditKind::Other
            && single_char_edit
            && !cursors.is_multi()
            && self.undo.last().is_some_and(|t| {
                t.kind == kind
                    && now.duration_since(t.when) < Duration::from_millis(1200)
                    && self.saved_state != Some(self.state)
                    && t.after == before
            });
        self.break_coalescing = false;
        let grouped = self.group_open && self.group_state == Some(self.state) && !self.undo.is_empty();
        if coalesce || grouped {
            let t = self.undo.last_mut().unwrap();
            t.changes.extend(changes);
            t.after = cursors.clone();
            t.when = now;
        } else {
            let prev_state = self.state;
            self.state = self.next_state;
            self.next_state += 1;
            self.undo.push(Transaction {
                changes,
                before,
                after: cursors.clone(),
                kind,
                when: now,
                state: self.state,
                prev_state,
            });
            if self.group_open {
                self.group_state = Some(self.state);
            }
        }
        map
    }

    pub fn undo(&mut self, cursors: &mut Cursors) -> bool {
        let Some(t) = self.undo.pop() else { return false };
        self.group_open = false;
        self.group_state = None;
        for c in t.changes.iter().rev() {
            self.raw_change(c.at, c.inserted.chars().count(), &c.removed);
        }
        *cursors = t.before.clone();
        cursors.clamp(self.rope.len_chars());
        self.state = t.prev_state;
        self.redo.push(t);
        self.break_coalescing = true;
        true
    }

    pub fn redo(&mut self, cursors: &mut Cursors) -> bool {
        let Some(t) = self.redo.pop() else { return false };
        for c in &t.changes {
            self.raw_change(c.at, c.removed.chars().count(), &c.inserted);
        }
        *cursors = t.after.clone();
        cursors.clamp(self.rope.len_chars());
        self.state = t.state;
        self.undo.push(t);
        self.break_coalescing = true;
        true
    }
}

/// Writes through a sibling temp file and renames, keeping the original's permissions.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // Follow symlinks, or the rename would replace the link instead of its target.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = target.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.tinta-{}", std::process::id()));
    let perms = std::fs::metadata(&target).ok().map(|m| m.permissions());
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Some(p) = perms.clone() {
            std::fs::set_permissions(&tmp, p)?;
        }
        std::fs::rename(&tmp, &target)
    };
    match write() {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.kind(), std::io::ErrorKind::PermissionDenied) && target.exists() => {
            // The directory may be read-only while the file itself is writable.
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(&target, bytes)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn decode(bytes: &[u8]) -> std::io::Result<(String, &'static Encoding, bool)> {
    if let Some((enc, bom)) = Encoding::for_bom(bytes) {
        let (text, _) = enc.decode_without_bom_handling(&bytes[bom..]);
        return Ok((text.into_owned(), enc, enc == encoding_rs::UTF_8));
    }
    let probe = &bytes[..bytes.len().min(8192)];
    if probe.contains(&0) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "binary file"));
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => Ok((s.to_string(), encoding_rs::UTF_8, false)),
        Err(_) => {
            let mut det = chardetng::EncodingDetector::new();
            det.feed(bytes, true);
            let enc = det.guess(None, true);
            let (text, _, _) = enc.decode(bytes);
            Ok((text.into_owned(), enc, false))
        }
    }
}

/// Looks at how lines step in. `None` when the file says nothing about it.
pub fn detect_indent(rope: &Rope) -> Option<Indent> {
    let mut tabs = 0usize;
    let mut steps = [0usize; 9];
    let mut prev = 0usize;
    for line in rope.lines().take(2000) {
        let mut spaces = 0usize;
        let mut first = None;
        for c in line.chars() {
            match c {
                ' ' => spaces += 1,
                '\t' => {
                    first = Some('\t');
                    break;
                }
                _ => {
                    first = Some(c);
                    break;
                }
            }
        }
        match first {
            Some('\t') => tabs += 1,
            Some('\n') | None => {}
            Some(_) => {
                let d = spaces.abs_diff(prev);
                if (2..=8).contains(&d) {
                    steps[d] += 1;
                }
                prev = spaces;
            }
        }
    }
    let (width, count) = steps.iter().enumerate().max_by_key(|(_, c)| **c).map(|(w, c)| (w, *c))?;
    if tabs == 0 && count == 0 {
        return None;
    }
    if tabs > count {
        Some(Indent { spaces: false, width: 4 })
    } else {
        Some(Indent { spaces: true, width })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_at(doc: &mut Document, cur: &mut Cursors, at: usize, s: &str) {
        doc.apply(vec![Edit::insert(at, s)], cur, EditKind::Typing, |m, c| {
            c.set_single(Selection::cursor(m.after_edit(0)));
        });
    }

    #[test]
    fn edits_map_positions_and_undo_restores() {
        let mut doc = Document::new("hello world");
        let mut cur = Cursors::single(Selection::cursor(5));
        let map = doc.apply(
            vec![Edit::new(0, 5, "bye"), Edit::insert(11, "!")],
            &mut cur,
            EditKind::Other,
            |m, c| c.map(|s| Selection::cursor(m.map(s.head, Assoc::After))),
        );
        assert_eq!(doc.rope.to_string(), "bye world!");
        assert_eq!(map.map(6, Assoc::After), 4);
        assert_eq!(cur.head(), 3);
        assert!(doc.undo(&mut cur));
        assert_eq!(doc.rope.to_string(), "hello world");
        assert_eq!(cur.head(), 5);
        assert!(doc.redo(&mut cur));
        assert_eq!(doc.rope.to_string(), "bye world!");
    }

    #[test]
    fn typing_coalesces_into_one_step_and_dirty_tracks_state() {
        let mut doc = Document::new("");
        let mut cur = Cursors::default();
        assert!(!doc.is_dirty());
        for (i, ch) in "abc".chars().enumerate() {
            type_at(&mut doc, &mut cur, i, &ch.to_string());
        }
        assert!(doc.is_dirty());
        doc.undo(&mut cur);
        assert_eq!(doc.rope.to_string(), "");
        assert!(!doc.is_dirty());
        doc.redo(&mut cur);
        assert_eq!(doc.rope.to_string(), "abc");
        assert!(doc.is_dirty());
    }

    #[test]
    fn saving_breaks_coalescing_so_undo_can_reach_the_saved_state() {
        let mut doc = Document::new("");
        let mut cur = Cursors::default();
        type_at(&mut doc, &mut cur, 0, "a");
        doc.mark_saved();
        type_at(&mut doc, &mut cur, 1, "b");
        assert!(doc.is_dirty());
        doc.undo(&mut cur);
        assert_eq!(doc.rope.to_string(), "a");
        assert!(!doc.is_dirty());
    }

    #[test]
    fn crlf_and_encodings_round_trip() {
        let dir = std::env::temp_dir().join(format!("tinta-buf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.txt");
        std::fs::write(&p, b"caf\xE9\r\nol\xE1\r\n").unwrap();
        let mut doc = Document::open(&p, Indent { spaces: true, width: 4 }).unwrap();
        assert_eq!(doc.eol, LineEnding::CrLf);
        assert_ne!(doc.encoding, encoding_rs::UTF_8);
        assert_eq!(doc.rope.to_string(), "café\nolá\n");
        doc.save_to(&p).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"caf\xE9\r\nol\xE1\r\n");

        std::fs::write(&p, b"\xFF\xFEo\0i\0\n\0").unwrap();
        let mut doc = Document::open(&p, Indent { spaces: true, width: 4 }).unwrap();
        assert_eq!(doc.rope.to_string(), "oi\n");
        doc.save_to(&p).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"\xFF\xFEo\0i\0\n\0");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The record of changes, applied to a copy the way a language server would, has to give the
    /// same text: through multi-cursor edits, undo, non-ASCII and characters over one UTF-16 unit.
    #[test]
    fn lsp_changes_replay_to_the_same_text() {
        fn apply(shadow: &mut String, c: &LspChange) {
            let rope = Rope::from_str(shadow);
            let a = rope.char_to_byte(crate::lsp::from_lsp(&rope, c.start));
            let b = rope.char_to_byte(crate::lsp::from_lsp(&rope, c.end));
            shadow.replace_range(a..b, &c.text);
        }
        let start = "ação 😀 linha\n\tsegunda 日本\nfim\n";
        let mut doc = Document::new(start);
        doc.track_lsp = true;
        let mut shadow = start.to_string();
        let mut cur = Cursors::default();
        let check = |doc: &mut Document, shadow: &mut String| {
            for c in doc.take_lsp_changes().unwrap() {
                apply(shadow, &c);
            }
            assert_eq!(*shadow, doc.rope.to_string());
        };
        doc.apply(vec![Edit::insert(7, "X😀"), Edit::new(0, 4, "é"), Edit::insert(14, "\nnova\n")], &mut cur, EditKind::Other, |_, _| {});
        check(&mut doc, &mut shadow);
        doc.apply(vec![Edit::delete(1, 6)], &mut cur, EditKind::Other, |_, _| {});
        check(&mut doc, &mut shadow);
        doc.undo(&mut cur);
        doc.undo(&mut cur);
        check(&mut doc, &mut shadow);
        assert_eq!(shadow, start);
        doc.redo(&mut cur);
        check(&mut doc, &mut shadow);
        doc.replace_all("outro texto\ncom 😀\n", &mut cur);
        check(&mut doc, &mut shadow);
    }

    #[test]
    fn too_many_lsp_changes_ask_for_the_whole_text() {
        let mut doc = Document::new("");
        doc.track_lsp = true;
        let mut cur = Cursors::default();
        for i in 0..600 {
            doc.apply(vec![Edit::insert(i, "x")], &mut cur, EditKind::Other, |_, _| {});
        }
        assert!(doc.take_lsp_changes().is_none());
        doc.apply(vec![Edit::insert(0, "y")], &mut cur, EditKind::Other, |_, _| {});
        assert_eq!(doc.take_lsp_changes().unwrap().len(), 1);
    }

    #[test]
    fn binary_files_are_refused() {
        assert!(decode(b"abc\0def").is_err());
    }

    #[test]
    fn indent_is_detected() {
        let r = Rope::from_str("fn a() {\n  x;\n  if y {\n    z;\n  }\n}\n");
        assert_eq!(detect_indent(&r), Some(Indent { spaces: true, width: 2 }));
        let r = Rope::from_str("a\n\tb\n\t\tc\n");
        assert_eq!(detect_indent(&r), Some(Indent { spaces: false, width: 4 }));
        assert_eq!(detect_indent(&Rope::from_str("a\nb\n")), None);
    }

    #[test]
    fn replace_all_keeps_undo_and_a_minimal_diff() {
        let mut doc = Document::new("a\nb\nc\n");
        let mut cur = Cursors::single(Selection::cursor(4));
        doc.replace_all("a\nB\nc\n", &mut cur);
        assert_eq!(doc.rope.to_string(), "a\nB\nc\n");
        assert_eq!(doc.undo.last().unwrap().changes.len(), 1);
        doc.undo(&mut cur);
        assert_eq!(doc.rope.to_string(), "a\nb\nc\n");
    }

    /// Ranges from stale positions are cut to the text, and overlapping ones don't double up.
    #[test]
    fn edits_out_of_range_or_overlapping_are_made_safe() {
        let mut doc = Document::new("hello\n");
        let mut cur = Cursors::single(Selection::cursor(0));
        doc.apply(vec![Edit::new(4, 99, "!")], &mut cur, EditKind::Other, |_, _| {});
        assert_eq!(doc.rope.to_string(), "hell!");
        // Backwards: an insert where it ends.
        doc.apply(vec![Edit::new(9, 3, "x")], &mut cur, EditKind::Other, |_, _| {});
        assert_eq!(doc.rope.to_string(), "helxl!");
        doc.apply(vec![Edit::new(0, 3, "A"), Edit::new(1, 2, "B")], &mut cur, EditKind::Other, |_, _| {});
        assert_eq!(doc.rope.to_string(), "Axl!");
        // Two cursors typing at one spot still both type.
        doc.apply(vec![Edit::insert(0, "1"), Edit::insert(0, "2")], &mut cur, EditKind::Other, |_, _| {});
        assert_eq!(doc.rope.to_string().len(), 6);
        assert_eq!(doc.line_len(99), 0);
        assert_eq!(doc.line_start(99), doc.len_chars());
        while doc.undo(&mut cur) {}
        assert_eq!(doc.rope.to_string(), "hello\n");
    }
}
