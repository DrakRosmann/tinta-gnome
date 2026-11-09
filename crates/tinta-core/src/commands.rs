//! Everything that moves cursors or edits text. Works for any number of cursors.

use crate::buffer::{Assoc, Document, Edit, EditKind};
use crate::cursor::{Cursors, Selection};
use crate::display::{DisplayMap, char_to_vcol};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    SubwordLeft,
    SubwordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp(usize),
    PageDown(usize),
    ParagraphUp,
    ParagraphDown,
}

#[derive(Debug, Clone, Copy)]
pub struct EditOpts {
    pub auto_pairs: bool,
    pub auto_indent: bool,
    pub surround: bool,
}

impl Default for EditOpts {
    fn default() -> Self {
        Self { auto_pairs: true, auto_indent: true, surround: true }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Word,
    Space,
    Newline,
    Punct,
}

fn class(c: char) -> CharClass {
    // Nearly all text is ASCII: answered without the Unicode tables.
    if c.is_ascii() {
        return match c {
            '\n' => CharClass::Newline,
            'a'..='z' | 'A'..='Z' | '0'..='9' | '_' => CharClass::Word,
            ' ' | '\t' | '\r' | '\x0b' | '\x0c' => CharClass::Space,
            _ => CharClass::Punct,
        };
    }
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

pub fn is_word_char(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    c.is_alphanumeric()
}

pub fn word_left(doc: &Document, mut pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    // A line break is a stop of its own.
    if doc.rope.char(pos - 1) == '\n' {
        return pos - 1;
    }
    while pos > 0 && class(doc.rope.char(pos - 1)) == CharClass::Space {
        pos -= 1;
    }
    if pos == 0 {
        return 0;
    }
    let c = class(doc.rope.char(pos - 1));
    if c == CharClass::Newline {
        return pos;
    }
    while pos > 0 && class(doc.rope.char(pos - 1)) == c {
        pos -= 1;
    }
    pos
}

pub fn word_right(doc: &Document, mut pos: usize) -> usize {
    let n = doc.len_chars();
    if pos >= n {
        return n;
    }
    if doc.rope.char(pos) == '\n' {
        return pos + 1;
    }
    while pos < n && class(doc.rope.char(pos)) == CharClass::Space {
        pos += 1;
    }
    if pos >= n {
        return n;
    }
    let c = class(doc.rope.char(pos));
    if c == CharClass::Newline {
        return pos;
    }
    while pos < n && class(doc.rope.char(pos)) == c {
        pos += 1;
    }
    pos
}

/// Stops inside words too: at `_` and at camelCase humps.
fn subword_left(doc: &Document, pos: usize) -> usize {
    let stop = word_left(doc, pos);
    let mut p = pos;
    if p == 0 {
        return 0;
    }
    while p > stop {
        let c = doc.rope.char(p - 1);
        if p != pos && p > stop && (c == '_' || (c.is_lowercase() && doc.rope.char(p).is_uppercase())) {
            return p;
        }
        p -= 1;
        if p != stop && doc.rope.char(p).is_uppercase() && doc.rope.char(p - 1).is_lowercase() {
            return p;
        }
    }
    stop
}

fn subword_right(doc: &Document, pos: usize) -> usize {
    let stop = word_right(doc, pos);
    let mut p = pos + 1;
    while p < stop {
        let prev = doc.rope.char(p - 1);
        let c = doc.rope.char(p);
        if (prev != '_' && c == '_') || (prev.is_lowercase() && c.is_uppercase()) {
            return p;
        }
        p += 1;
    }
    stop
}

/// Next grapheme boundary, so accents and emoji move as one.
fn grapheme_right(doc: &Document, pos: usize) -> usize {
    if pos >= doc.len_chars() {
        return pos;
    }
    let (line, col) = doc.line_col(pos);
    if col >= doc.line_len(line) {
        return pos + 1;
    }
    let text = doc.line_slice(line).to_string();
    let mut c = 0;
    for g in text.graphemes(true) {
        let n = g.chars().count();
        if c + n > col {
            return doc.line_start(line) + c + n;
        }
        c += n;
    }
    pos + 1
}

fn grapheme_left(doc: &Document, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let (line, col) = doc.line_col(pos);
    if col == 0 {
        return pos - 1;
    }
    let text = doc.line_slice(line).to_string();
    let mut c = 0;
    for g in text.graphemes(true) {
        let n = g.chars().count();
        if c + n >= col {
            return doc.line_start(line) + c;
        }
        c += n;
    }
    pos - 1
}

/// Home goes to the first non-blank char, and from there to column 0.
fn smart_home(doc: &Document, map: &DisplayMap, pos: usize) -> usize {
    let (row, _) = map.pos_to_row(doc, pos);
    let r = map.row(doc, row);
    let line = r.line as usize;
    let start = doc.line_start(line);
    if r.start > 0 {
        let row_start = start + r.start as usize;
        return if pos == row_start { doc.line_start(line) } else { row_start };
    }
    let first = doc.line_slice(line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
    let target = start + first;
    if pos == target { start } else { target }
}

fn row_end(doc: &Document, map: &DisplayMap, pos: usize) -> usize {
    let (row, _) = map.pos_to_row(doc, pos);
    map.row_to_pos(doc, row, usize::MAX / 2)
}

fn vertical(doc: &Document, map: &DisplayMap, s: Selection, delta: isize) -> (usize, Option<u32>) {
    let (row, v) = map.pos_to_row(doc, s.head);
    let r = map.row(doc, row);
    let goal = s.goal.map(|g| g as usize).unwrap_or(v + r.pad as usize);
    let count = map.row_count(doc) as isize;
    let target = row as isize + delta;
    if target < 0 {
        return (0, None);
    }
    if target >= count {
        return (doc.len_chars(), None);
    }
    let tr = map.row(doc, target as usize);
    let pos = map.row_to_pos(doc, target as usize, goal.saturating_sub(tr.pad as usize));
    (pos, Some(goal as u32))
}

fn paragraph(doc: &Document, pos: usize, down: bool) -> usize {
    let mut line = doc.char_to_line(pos) as isize;
    let total = doc.len_lines() as isize;
    let step = if down { 1 } else { -1 };
    line += step;
    while line >= 0 && line < total && doc.line_is_blank(line as usize) {
        line += step;
    }
    while line >= 0 && line < total && !doc.line_is_blank(line as usize) {
        line += step;
    }
    if line < 0 {
        0
    } else if line >= total {
        doc.len_chars()
    } else {
        doc.line_start(line as usize)
    }
}

pub fn move_cursors(doc: &Document, cur: &mut Cursors, map: &DisplayMap, m: Motion, extend: bool) {
    cur.map(|s| {
        // Plain arrows on a selection collapse it to the edge.
        if !extend && !s.is_empty() {
            match m {
                Motion::Left => return Selection::cursor(s.start()),
                Motion::Right => return Selection::cursor(s.end()),
                _ => {}
            }
        }
        let (head, goal) = match m {
            Motion::Left => (grapheme_left(doc, s.head), None),
            Motion::Right => (grapheme_right(doc, s.head), None),
            Motion::WordLeft => (word_left(doc, s.head), None),
            Motion::WordRight => (word_right(doc, s.head), None),
            Motion::SubwordLeft => (subword_left(doc, s.head), None),
            Motion::SubwordRight => (subword_right(doc, s.head), None),
            Motion::Up => vertical(doc, map, s, -1),
            Motion::Down => vertical(doc, map, s, 1),
            Motion::PageUp(n) => vertical(doc, map, s, -(n as isize)),
            Motion::PageDown(n) => vertical(doc, map, s, n as isize),
            Motion::LineStart => (smart_home(doc, map, s.head), None),
            Motion::LineEnd => (row_end(doc, map, s.head), None),
            Motion::DocStart => (0, None),
            Motion::DocEnd => (doc.len_chars(), None),
            Motion::ParagraphUp => (paragraph(doc, s.head, false), None),
            Motion::ParagraphDown => (paragraph(doc, s.head, true), None),
        };
        Selection { anchor: if extend { s.anchor } else { head }, head, goal }
    });
}

/// Replaces selections one by one. `f` returns the edit and where the selection goes, as offsets
/// from the edit's start in the new text.
fn per_selection(
    doc: &mut Document,
    cur: &mut Cursors,
    kind: EditKind,
    mut f: impl FnMut(&Document, Selection) -> (Edit, usize, usize),
) {
    let mut edits = Vec::with_capacity(cur.len());
    let mut offs = Vec::with_capacity(cur.len());
    let mut last_end = 0;
    for s in cur.iter() {
        let (mut e, a, h) = f(doc, *s);
        // Two cursors close together may reach for the same text.
        e.start = e.start.max(last_end);
        e.end = e.end.max(e.start);
        last_end = e.end;
        edits.push(e);
        offs.push((a, h));
    }
    let primary = cur.primary_index();
    doc.apply(edits, cur, kind, |m, c| {
        let sels = offs
            .iter()
            .enumerate()
            .map(|(i, (a, h))| {
                let base = m.before_edit(i);
                Selection::new(base + a, base + h)
            })
            .collect();
        *c = Cursors::from_vec(sels, primary);
    });
}

fn closer(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// Inserts typed text: auto-closes pairs, types over closers, surrounds selections.
pub fn type_text(doc: &mut Document, cur: &mut Cursors, text: &str, opts: EditOpts) {
    let single = {
        let mut it = text.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    };
    let quotes = doc.language.auto_quotes;
    per_selection(doc, cur, EditKind::Typing, |doc, s| {
        if let Some(c) = single
            && opts.auto_pairs
        {
            let is_quote = quotes.contains(&c);
            let close = closer(c).or(is_quote.then_some(c));
            let next = doc.char_at(s.end());
            let prev = if s.start() > 0 { doc.char_at(s.start() - 1) } else { None };
            if !s.is_empty() {
                if let Some(cl) = close.filter(|_| opts.surround) {
                    let inner = doc.slice_string(s.start(), s.end());
                    let n = inner.chars().count();
                    return (
                        Edit::new(s.start(), s.end(), format!("{c}{inner}{cl}")),
                        1,
                        1 + n,
                    );
                }
            } else {
                // Type over the closer that auto-pairing put there.
                if (matches!(c, ')' | ']' | '}') || is_quote) && next == Some(c) {
                    return (Edit::insert(s.head, ""), 1, 1);
                }
                if let Some(cl) = close {
                    let next_ok = next.is_none_or(|n| n.is_whitespace() || matches!(n, ')' | ']' | '}' | ',' | ';' | ':' | '.'));
                    let prev_ok = !is_quote || !prev.is_some_and(|p| p.is_alphanumeric() || p == c || p == '\\');
                    if next_ok && prev_ok {
                        return (Edit::insert(s.head, format!("{c}{cl}")), 1, 1);
                    }
                }
            }
        }
        let n = text.chars().count();
        (Edit::new(s.start(), s.end(), text), n, n)
    });
}

/// Plain insert at every selection, no pairing. For paste, completions and tests.
pub fn insert_plain(doc: &mut Document, cur: &mut Cursors, text: &str) {
    let n = text.chars().count();
    per_selection(doc, cur, EditKind::Other, |_, s| (Edit::new(s.start(), s.end(), text), n, n));
}

/// One text per cursor (multi-cursor copy pasted back into as many cursors).
pub fn paste_each(doc: &mut Document, cur: &mut Cursors, parts: &[String]) {
    let mut i = 0;
    per_selection(doc, cur, EditKind::Other, |_, s| {
        let t = parts.get(i).cloned().unwrap_or_default();
        i += 1;
        let n = t.chars().count();
        (Edit::new(s.start(), s.end(), t), n, n)
    });
}

/// Pasting text copied as a whole line goes above the cursor's line, as in VS Code and Sublime.
pub fn paste_lines(doc: &mut Document, cur: &mut Cursors, text: &str) {
    let n = text.chars().count();
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        if s.is_empty() {
            let (line, col) = doc.line_col(s.head);
            let start = doc.line_start(line);
            (Edit::insert(start, text), n + col, n + col)
        } else {
            (Edit::new(s.start(), s.end(), text), n, n)
        }
    });
}

pub fn newline(doc: &mut Document, cur: &mut Cursors, opts: EditOpts) {
    let unit = doc.indent.unit();
    let indent_after = doc.language.indent_after;
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        let (line, col) = doc.line_col(s.start());
        if !opts.auto_indent {
            return (Edit::new(s.start(), s.end(), "\n"), 1, 1);
        }
        let indent = doc.line_indent(line);
        // Cursor inside the leading whitespace: don't invent extra indentation.
        let indent: String = indent.chars().take(col).collect();
        let before: String = doc.line_slice(line).chars().take(col).collect();
        let last = before.trim_end().chars().last();
        let next = doc.char_at(s.end());
        let opens = last.is_some_and(|c| indent_after.contains(&c));
        let closes = opens
            && last.and_then(closer).is_some()
            && next.is_some_and(|n| last.and_then(closer) == Some(n));
        if closes {
            // { | }  becomes three lines with the cursor in the middle.
            let middle = format!("\n{indent}{unit}");
            let n = middle.chars().count();
            let text = format!("{middle}\n{indent}");
            (Edit::new(s.start(), s.end(), text), n, n)
        } else {
            let extra = if opens { unit.as_str() } else { "" };
            let text = format!("\n{indent}{extra}");
            let n = text.chars().count();
            (Edit::new(s.start(), s.end(), text), n, n)
        }
    });
}

fn is_pair(a: char, b: char, quotes: &[char]) -> bool {
    closer(a) == Some(b) || (a == b && quotes.contains(&a))
}

pub fn backspace(doc: &mut Document, cur: &mut Cursors, opts: EditOpts) {
    let width = doc.indent.width.max(1);
    let spaces = doc.indent.spaces;
    let quotes = doc.language.auto_quotes;
    per_selection(doc, cur, EditKind::Deleting, |doc, s| {
        if !s.is_empty() {
            return (Edit::delete(s.start(), s.end()), 0, 0);
        }
        if s.head == 0 {
            return (Edit::insert(0, ""), 0, 0);
        }
        let (line, col) = doc.line_col(s.head);
        let prev = doc.rope.char(s.head - 1);
        if opts.auto_pairs
            && let Some(next) = doc.char_at(s.head)
            && is_pair(prev, next, quotes)
        {
            return (Edit::delete(s.head - 1, s.head + 1), 0, 0);
        }
        // Inside leading spaces, one stroke goes back one tab stop.
        if spaces && col > 0 && prev == ' ' {
            let before: String = doc.line_slice(line).chars().take(col).collect();
            if before.chars().all(|c| c == ' ') {
                let back = match col % width {
                    0 => width,
                    n => n,
                };
                return (Edit::delete(s.head - back, s.head), 0, 0);
            }
        }
        (Edit::delete(grapheme_left(doc, s.head), s.head), 0, 0)
    });
}

pub fn delete_forward(doc: &mut Document, cur: &mut Cursors) {
    per_selection(doc, cur, EditKind::Deleting, |doc, s| {
        if !s.is_empty() {
            return (Edit::delete(s.start(), s.end()), 0, 0);
        }
        (Edit::delete(s.head, grapheme_right(doc, s.head)), 0, 0)
    });
}

pub fn delete_word(doc: &mut Document, cur: &mut Cursors, forward: bool) {
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        if !s.is_empty() {
            return (Edit::delete(s.start(), s.end()), 0, 0);
        }
        if forward {
            (Edit::delete(s.head, word_right(doc, s.head)), 0, 0)
        } else {
            (Edit::delete(word_left(doc, s.head), s.head), 0, 0)
        }
    });
}

pub fn delete_to_line_edge(doc: &mut Document, cur: &mut Cursors, forward: bool) {
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        let line = doc.char_to_line(s.head);
        if forward {
            (Edit::delete(s.head, doc.line_end(line)), 0, 0)
        } else {
            (Edit::delete(doc.line_start(line), s.head), 0, 0)
        }
    });
}

/// Line ranges covered by the selections, merged. A selection ending at column 0 stops short of
/// that line.
pub fn line_blocks(doc: &Document, cur: &Cursors) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for s in cur.iter() {
        let l0 = doc.char_to_line(s.start());
        let mut l1 = doc.char_to_line(s.end());
        if !s.is_empty() && l1 > l0 && doc.line_start(l1) == s.end() {
            l1 -= 1;
        }
        match out.last_mut() {
            Some(last) if last.1 + 1 >= l0 => last.1 = last.1.max(l1),
            _ => out.push((l0, l1)),
        }
    }
    out
}

pub fn delete_lines(doc: &mut Document, cur: &mut Cursors) {
    let blocks = line_blocks(doc, cur);
    let total = doc.len_lines();
    let mut edits = Vec::new();
    let mut anchors = Vec::new();
    for &(l0, l1) in &blocks {
        let (start, end) = if l1 + 1 < total {
            (doc.line_start(l0), doc.line_start(l1 + 1))
        } else if l0 > 0 {
            (doc.line_end(l0 - 1), doc.len_chars())
        } else {
            (0, doc.len_chars())
        };
        edits.push(Edit::delete(start, end));
        anchors.push(end);
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        let sels: Vec<_> = (0..anchors.len()).map(|i| Selection::cursor(m.after_edit(i))).collect();
        *c = Cursors::from_vec(sels, 0);
    });
    let n = doc.len_chars();
    cur.clamp(n);
}

/// Sublime's Ctrl+Shift+D: copies the selection, or the line when nothing is selected.
pub fn duplicate(doc: &mut Document, cur: &mut Cursors) {
    if cur.iter().all(|s| s.is_empty()) {
        return copy_lines(doc, cur, true);
    }
    let primary = cur.primary_index();
    let spans: Vec<(usize, usize, bool)> = cur.iter().map(|s| (s.start(), s.end(), s.reversed())).collect();
    let edits = spans
        .iter()
        .filter(|(a, b, _)| a != b)
        .map(|&(a, b, _)| Edit::insert(b, doc.slice_string(a, b)))
        .collect();
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        let sels = spans
            .iter()
            .map(|&(a, b, rev)| {
                let len = b - a;
                let end = m.map(b, Assoc::After);
                let start = end - len;
                if rev { Selection::new(end, start) } else { Selection::new(start, end) }
            })
            .collect();
        *c = Cursors::from_vec(sels, primary);
    });
}

/// Copies selected lines below (cursors follow) or above (cursors stay).
pub fn copy_lines(doc: &mut Document, cur: &mut Cursors, down: bool) {
    let blocks = line_blocks(doc, cur);
    let mut edits = Vec::new();
    for &(l0, l1) in &blocks {
        let start = doc.line_start(l0);
        let text = doc.slice_string(start, doc.line_end(l1));
        edits.push(Edit::insert(start, format!("{text}\n")));
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        c.map(|s| {
            let a = if down { Assoc::After } else { Assoc::Before };
            Selection::new(m.map(s.anchor, a), m.map(s.head, a))
        });
    });
}

pub fn move_lines(doc: &mut Document, cur: &mut Cursors, down: bool) {
    let blocks = line_blocks(doc, cur);
    let total = doc.len_lines();
    let primary = cur.primary_index();
    let sels: Vec<Selection> = cur.as_slice().to_vec();
    let mut edits = Vec::new();
    let mut new_sels = sels.clone();
    for &(l0, l1) in &blocks {
        // Stops at the edge: the last line of a file ending in "\n" is the empty one.
        let (a, b) = if down {
            if l1 + 1 >= total {
                continue;
            }
            (l0, l1 + 1)
        } else {
            if l0 == 0 {
                continue;
            }
            (l0 - 1, l1)
        };
        let start = doc.line_start(a);
        let end = doc.line_end(b);
        let block = doc.slice_string(doc.line_start(l0), doc.line_end(l1));
        let (other, shift): (String, isize) = if down {
            let o = doc.line_slice(l1 + 1).to_string();
            let n = o.chars().count() as isize + 1;
            (o, n)
        } else {
            let o = doc.line_slice(l0 - 1).to_string();
            let n = o.chars().count() as isize + 1;
            (o, -n)
        };
        let text = if down { format!("{other}\n{block}") } else { format!("{block}\n{other}") };
        edits.push(Edit::new(start, end, text));
        for s in &mut new_sels {
            let lo = doc.line_start(l0);
            let hi = doc.line_end(l1);
            if s.start() >= lo && s.end() <= hi + 1 {
                s.anchor = (s.anchor as isize + shift) as usize;
                s.head = (s.head as isize + shift) as usize;
            }
        }
    }
    if edits.is_empty() {
        return;
    }
    doc.apply(edits, cur, EditKind::Other, |_, c| *c = Cursors::from_vec(new_sels, primary));
}

pub fn indent_lines(doc: &mut Document, cur: &mut Cursors) {
    let unit = doc.indent.unit();
    let mut edits = Vec::new();
    for (l0, l1) in line_blocks(doc, cur) {
        for l in l0..=l1 {
            if !doc.line_is_blank(l) || l0 == l1 {
                edits.push(Edit::insert(doc.line_start(l), unit.clone()));
            }
        }
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        c.map(|s| Selection::new(m.map(s.anchor, Assoc::After), m.map(s.head, Assoc::After)));
    });
}

pub fn outdent_lines(doc: &mut Document, cur: &mut Cursors) {
    let width = doc.indent.width.max(1);
    let mut edits = Vec::new();
    for (l0, l1) in line_blocks(doc, cur) {
        for l in l0..=l1 {
            let start = doc.line_start(l);
            let mut n = 0;
            for c in doc.line_slice(l).chars().take(width) {
                match c {
                    ' ' => n += 1,
                    '\t' => {
                        n += 1;
                        break;
                    }
                    _ => break,
                }
            }
            if n > 0 {
                edits.push(Edit::delete(start, start + n));
            }
        }
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        c.map(|s| Selection::new(m.map(s.anchor, Assoc::Before), m.map(s.head, Assoc::Before)));
    });
}

/// Tab: indents selected lines, or inserts up to the next tab stop.
pub fn tab(doc: &mut Document, cur: &mut Cursors) {
    if cur.iter().any(|s| doc.char_to_line(s.start()) != doc.char_to_line(s.end())) {
        return indent_lines(doc, cur);
    }
    let spaces = doc.indent.spaces;
    let width = doc.indent.width.max(1);
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        if !spaces {
            return (Edit::new(s.start(), s.end(), "\t"), 1, 1);
        }
        let (line, col) = doc.line_col(s.start());
        let v = char_to_vcol(doc.line_slice(line), col, width);
        let n = width - v % width;
        (Edit::new(s.start(), s.end(), " ".repeat(n)), n, n)
    });
}

pub fn toggle_line_comment(doc: &mut Document, cur: &mut Cursors) {
    let Some(token) = doc.language.line_comment else {
        return toggle_block_comment(doc, cur);
    };
    let spaced = format!("{token} ");
    let blocks = line_blocks(doc, cur);
    let mut edits = Vec::new();
    for (l0, l1) in blocks {
        let lines: Vec<usize> = (l0..=l1).filter(|l| !doc.line_is_blank(*l) || l0 == l1).collect();
        let is_commented = |l: usize| doc.line_text(l).trim_start().starts_with(token);
        let all = lines.iter().all(|l| is_commented(*l)) && !lines.is_empty();
        if all {
            for &l in &lines {
                let text = doc.line_text(l);
                let ws = text.chars().take_while(|c| c.is_whitespace()).count();
                let rest: String = text.chars().skip(ws).collect();
                let n = if rest.starts_with(&spaced) { spaced.chars().count() } else { token.chars().count() };
                let at = doc.line_start(l) + ws;
                edits.push(Edit::delete(at, at + n));
            }
        } else {
            let min_ws = lines
                .iter()
                .map(|l| doc.line_slice(*l).chars().take_while(|c| *c == ' ' || *c == '\t').count())
                .min()
                .unwrap_or(0);
            for &l in &lines {
                edits.push(Edit::insert(doc.line_start(l) + min_ws, spaced.clone()));
            }
        }
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        c.map(|s| {
            let a = if s.anchor == s.head { Assoc::After } else { Assoc::Before };
            Selection { anchor: m.map(s.anchor, a), head: m.map(s.head, Assoc::After), goal: None }
        });
    });
}

pub fn toggle_block_comment(doc: &mut Document, cur: &mut Cursors) {
    let Some((open, close)) = doc.language.block_comment else { return };
    let (no, nc) = (open.chars().count(), close.chars().count());
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        let text = doc.slice_string(s.start(), s.end());
        if text.starts_with(open) && text.ends_with(close) && text.chars().count() >= no + nc {
            let inner: String = text.chars().skip(no).take(text.chars().count() - no - nc).collect();
            let n = inner.chars().count();
            return (Edit::new(s.start(), s.end(), inner), 0, n);
        }
        let n = text.chars().count();
        (Edit::new(s.start(), s.end(), format!("{open}{text}{close}")), no, no + n)
    });
}

pub fn insert_line_below(doc: &mut Document, cur: &mut Cursors) {
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        let line = doc.char_to_line(s.end());
        let t = format!("\n{}", doc.line_indent(line));
        let n = t.chars().count();
        (Edit::insert(doc.line_end(line), t), n, n)
    });
}

pub fn insert_line_above(doc: &mut Document, cur: &mut Cursors) {
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        let line = doc.char_to_line(s.start());
        let indent = doc.line_indent(line);
        let n = indent.chars().count();
        (Edit::insert(doc.line_start(line), format!("{indent}\n")), n, n)
    });
}

pub fn join_lines(doc: &mut Document, cur: &mut Cursors) {
    let mut edits = Vec::new();
    for (l0, l1) in line_blocks(doc, cur) {
        let last = if l1 == l0 { l0 + 1 } else { l1 };
        for l in l0..last.min(doc.len_lines() - 1) {
            let end = doc.line_end(l);
            let next = doc.line_slice(l + 1);
            let ws = next.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            let glue = if next.len_chars() == ws { "" } else { " " };
            edits.push(Edit::new(end, end + 1 + ws, glue));
        }
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        c.map(|s| Selection::new(m.map(s.anchor, Assoc::Before), m.map(s.head, Assoc::Before)));
    });
}

/// Applies `f` to the selected text of each selection, or the word under each cursor.
pub fn transform_selections(doc: &mut Document, cur: &mut Cursors, f: impl Fn(&str) -> String) {
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        let (a, b) = if s.is_empty() { word_at(doc, s.head) } else { (s.start(), s.end()) };
        let out = f(&doc.slice_string(a, b));
        let n = out.chars().count();
        (Edit::new(a, b, out), 0, n)
    });
}

pub fn to_upper(s: &str) -> String {
    s.to_uppercase()
}

pub fn to_lower(s: &str) -> String {
    s.to_lowercase()
}

pub fn to_title(s: &str) -> String {
    let mut out = String::new();
    let mut start = true;
    for c in s.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
        start = !c.is_alphanumeric();
    }
    out
}

fn split_identifier(s: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in s.chars() {
        if c == '_' || c == '-' || c == ' ' {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            parts.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_numeric();
        cur.push(c);
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

pub fn to_snake(s: &str) -> String {
    split_identifier(s).iter().map(|p| p.to_lowercase()).collect::<Vec<_>>().join("_")
}

pub fn to_kebab(s: &str) -> String {
    split_identifier(s).iter().map(|p| p.to_lowercase()).collect::<Vec<_>>().join("-")
}

pub fn to_camel(s: &str) -> String {
    let mut out = String::new();
    for (i, p) in split_identifier(s).iter().enumerate() {
        let low = p.to_lowercase();
        if i == 0 {
            out.push_str(&low);
        } else {
            let mut cs = low.chars();
            if let Some(c) = cs.next() {
                out.extend(c.to_uppercase());
                out.push_str(cs.as_str());
            }
        }
    }
    out
}

pub fn to_pascal(s: &str) -> String {
    let mut out = String::new();
    for p in split_identifier(s) {
        let low = p.to_lowercase();
        let mut cs = low.chars();
        if let Some(c) = cs.next() {
            out.extend(c.to_uppercase());
            out.push_str(cs.as_str());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOp {
    SortAsc,
    SortDesc,
    SortCaseless,
    Reverse,
    Unique,
    Shuffle,
}

/// Works on the selected lines, or the whole file when just a cursor sits in it.
pub fn line_op(doc: &mut Document, cur: &mut Cursors, op: LineOp) {
    let mut blocks = line_blocks(doc, cur);
    if blocks.len() == 1 && blocks[0].0 == blocks[0].1 {
        let mut last = doc.len_lines() - 1;
        if last > 0 && doc.line_len(last) == 0 {
            last -= 1;
        }
        blocks = vec![(0, last)];
    }
    let mut edits = Vec::new();
    for (l0, l1) in blocks {
        let mut lines: Vec<String> = (l0..=l1).map(|l| doc.line_slice(l).to_string()).collect();
        match op {
            LineOp::SortAsc => lines.sort(),
            LineOp::SortDesc => lines.sort_by(|a, b| b.cmp(a)),
            LineOp::SortCaseless => lines.sort_by_key(|l| l.to_lowercase()),
            LineOp::Reverse => lines.reverse(),
            LineOp::Unique => {
                let mut seen = std::collections::HashSet::new();
                lines.retain(|l| seen.insert(l.clone()));
            }
            LineOp::Shuffle => {
                // No need for a real RNG crate here: the clock is random enough to mix lines.
                let mut x = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.subsec_nanos() as u64)
                    .unwrap_or(1)
                    | 1;
                for i in (1..lines.len()).rev() {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    lines.swap(i, (x % (i as u64 + 1)) as usize);
                }
            }
        }
        edits.push(Edit::new(doc.line_start(l0), doc.line_end(l1), lines.join("\n")));
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        c.map(|s| Selection::new(m.map(s.anchor, Assoc::Before), m.map(s.head, Assoc::After)));
    });
}

pub fn trim_trailing(doc: &mut Document, cur: &mut Cursors) {
    let mut edits = Vec::new();
    for l in 0..doc.len_lines() {
        let n = doc.line_len(l);
        let keep = {
            let slice = doc.line_slice(l);
            let mut k = n;
            while k > 0 && matches!(slice.char(k - 1), ' ' | '\t') {
                k -= 1;
            }
            k
        };
        if keep < n {
            let start = doc.line_start(l);
            edits.push(Edit::delete(start + keep, start + n));
        }
    }
    doc.apply(edits, cur, EditKind::Other, |m, c| {
        c.map(|s| Selection::new(m.map(s.anchor, Assoc::Before), m.map(s.head, Assoc::Before)));
    });
}

pub fn ensure_final_newline(doc: &mut Document, cur: &mut Cursors) {
    let n = doc.len_chars();
    if n > 0 && doc.rope.char(n - 1) != '\n' {
        doc.apply(vec![Edit::insert(n, "\n")], cur, EditKind::Other, |m, c| {
            c.map(|s| Selection::new(m.map(s.anchor, Assoc::Before), m.map(s.head, Assoc::Before)));
        });
    }
}

pub fn transpose(doc: &mut Document, cur: &mut Cursors) {
    per_selection(doc, cur, EditKind::Other, |doc, s| {
        let n = doc.len_chars();
        let at = if s.head >= n || doc.rope.char(s.head) == '\n' { s.head.saturating_sub(1) } else { s.head };
        if at == 0 || at + 1 > n {
            return (Edit::insert(s.head, ""), 0, 0);
        }
        let (a, b) = (doc.rope.char(at - 1), doc.rope.char(at));
        if a == '\n' || b == '\n' {
            return (Edit::insert(s.head, ""), 0, 0);
        }
        (Edit::new(at - 1, at + 1, format!("{b}{a}")), 2, 2)
    });
}

// Selection commands --------------------------------------------------------------------------

pub fn word_at(doc: &Document, pos: usize) -> (usize, usize) {
    let n = doc.len_chars();
    let pos = pos.min(n);
    let probe = if pos < n && is_word_char(doc.rope.char(pos)) {
        pos
    } else if pos > 0 && is_word_char(doc.rope.char(pos - 1)) {
        pos - 1
    } else {
        return (pos, pos);
    };
    let (mut a, mut b) = (probe, probe + 1);
    while a > 0 && is_word_char(doc.rope.char(a - 1)) {
        a -= 1;
    }
    while b < n && is_word_char(doc.rope.char(b)) {
        b += 1;
    }
    (a, b)
}

/// The word a double click selects; on punctuation and spaces, the run of that class.
pub fn word_or_run_at(doc: &Document, pos: usize) -> (usize, usize) {
    let (a, b) = word_at(doc, pos);
    if a != b {
        return (a, b);
    }
    let n = doc.len_chars();
    if pos >= n {
        return (pos, pos);
    }
    let c = class(doc.rope.char(pos));
    if c == CharClass::Newline {
        return (pos, pos);
    }
    let (mut a, mut b) = (pos, pos + 1);
    while a > 0 && class(doc.rope.char(a - 1)) == c {
        a -= 1;
    }
    while b < n && class(doc.rope.char(b)) == c {
        b += 1;
    }
    (a, b)
}

pub fn select_all(doc: &Document, cur: &mut Cursors) {
    cur.set_single(Selection::new(0, doc.len_chars()));
}

pub fn select_line(doc: &Document, cur: &mut Cursors) {
    let total = doc.len_lines();
    cur.map(|s| {
        let l0 = doc.char_to_line(s.start());
        let l1 = doc.char_to_line(s.end());
        let start = doc.line_start(l0);
        let end = if l1 + 1 < total { doc.line_start(l1 + 1) } else { doc.len_chars() };
        Selection::new(start, end)
    });
}

pub fn select_word(doc: &Document, cur: &mut Cursors) {
    cur.map(|s| {
        if s.is_empty() {
            let (a, b) = word_at(doc, s.head);
            Selection::new(a, b)
        } else {
            s
        }
    });
}

fn find_str_from(hay: &str, needle: &str, from_byte: usize) -> Option<usize> {
    hay.get(from_byte..)?.find(needle).map(|i| i + from_byte)
}

/// Ctrl+D: selects the word, then adds its next occurrence as another selection.
pub fn add_next_occurrence(doc: &Document, cur: &mut Cursors) -> bool {
    let p = cur.primary();
    if p.is_empty() {
        let (a, b) = word_at(doc, p.head);
        if a == b {
            return false;
        }
        cur.map(|s| if s == p { Selection::new(a, b) } else { s });
        return true;
    }
    let needle = doc.slice_string(p.start(), p.end());
    let hay = doc.rope.to_string();
    let from_byte = doc.rope.char_to_byte(p.end());
    let mut search_from = from_byte;
    let mut wrapped = false;
    loop {
        let found = find_str_from(&hay, &needle, search_from);
        match found {
            Some(b) => {
                let start = doc.rope.byte_to_char(b);
                let end = start + needle.chars().count();
                // Already selected? Keep going.
                if cur.iter().any(|s| s.start() == start && s.end() == end) {
                    search_from = b + needle.len().max(1);
                    if wrapped && b >= from_byte {
                        return false;
                    }
                    continue;
                }
                cur.add(Selection::new(start, end));
                return true;
            }
            None if !wrapped => {
                wrapped = true;
                search_from = 0;
            }
            None => return false,
        }
    }
}

/// Ctrl+K Ctrl+D: drops the newest selection and moves on to the following occurrence.
pub fn skip_occurrence(doc: &Document, cur: &mut Cursors) -> bool {
    let p = cur.primary();
    if p.is_empty() {
        return add_next_occurrence(doc, cur);
    }
    let was_multi = cur.is_multi();
    let added = add_next_occurrence(doc, cur);
    if added {
        let new_primary = cur.primary();
        let sels: Vec<Selection> = cur.iter().copied().filter(|s| *s != p).collect();
        if !sels.is_empty() {
            let idx = sels.iter().position(|s| *s == new_primary).unwrap_or(0);
            *cur = Cursors::from_vec(sels, idx);
        }
    }
    added || was_multi
}

/// Ctrl+Shift+L: a selection on every occurrence of the current one.
pub fn select_all_occurrences(doc: &Document, cur: &mut Cursors) -> bool {
    let p = cur.primary();
    let (a, b) = if p.is_empty() { word_at(doc, p.head) } else { (p.start(), p.end()) };
    if a == b {
        return false;
    }
    let needle = doc.slice_string(a, b);
    let hay = doc.rope.to_string();
    let n = needle.chars().count();
    let mut sels = Vec::new();
    let mut primary = 0;
    for (byte, _) in hay.match_indices(&needle) {
        let s = doc.rope.byte_to_char(byte);
        if s == a {
            primary = sels.len();
        }
        sels.push(Selection::new(s, s + n));
    }
    if sels.is_empty() {
        return false;
    }
    *cur = Cursors::from_vec(sels, primary);
    true
}

/// A cursor at the end of every line a selection touches.
pub fn split_into_lines(doc: &Document, cur: &mut Cursors) {
    let mut out = Vec::new();
    for s in cur.iter() {
        let (l0, l1) = (doc.char_to_line(s.start()), doc.char_to_line(s.end()));
        if l0 == l1 {
            out.push(*s);
            continue;
        }
        for l in l0..=l1 {
            let a = doc.line_start(l).max(s.start());
            let b = doc.line_end(l).min(s.end());
            if l == l1 && a == b {
                continue;
            }
            out.push(Selection::new(a, b));
        }
    }
    let n = out.len();
    *cur = Cursors::from_vec(out, n.saturating_sub(1));
}

pub fn add_cursor_vertical(doc: &Document, cur: &mut Cursors, map: &DisplayMap, down: bool) {
    let edge = if down { cur.iter().last() } else { cur.iter().next() }.copied().unwrap();
    let (pos, goal) = vertical(doc, map, Selection { goal: edge.goal, ..Selection::cursor(edge.head) }, if down { 1 } else { -1 });
    if pos == edge.head {
        return;
    }
    let (row_new, _) = map.pos_to_row(doc, pos);
    let (row_old, _) = map.pos_to_row(doc, edge.head);
    if row_new == row_old {
        return;
    }
    cur.add(Selection { anchor: pos, head: pos, goal });
}

/// Column (box) selection from an anchor to a head, both as (row, visual column).
pub fn column_select(
    doc: &Document,
    map: &DisplayMap,
    anchor: (usize, usize),
    head: (usize, usize),
) -> Cursors {
    let (r0, r1) = (anchor.0.min(head.0), anchor.0.max(head.0));
    let mut sels = Vec::new();
    let down = head.0 >= anchor.0;
    for row in r0..=r1 {
        let a = map.row_to_pos(doc, row, anchor.1);
        let h = map.row_to_pos(doc, row, head.1);
        // Rows too short for the box get no selection, unless the box has no width.
        if a == h && anchor.1 != head.1 {
            continue;
        }
        sels.push(Selection::new(a, h));
    }
    if sels.is_empty() {
        sels.push(Selection::cursor(map.row_to_pos(doc, head.0, head.1)));
    }
    let primary = if down { sels.len() - 1 } else { 0 };
    Cursors::from_vec(sels, primary)
}

/// Offset of the bracket matching the one at or just before `pos`.
pub fn matching_bracket(doc: &Document, pos: usize) -> Option<(usize, usize)> {
    let n = doc.len_chars();
    let try_at = |p: usize| -> Option<(usize, usize)> {
        let c = doc.char_at(p)?;
        let (open, close, forward) = match c {
            '(' => ('(', ')', true),
            '[' => ('[', ']', true),
            '{' => ('{', '}', true),
            ')' => ('(', ')', false),
            ']' => ('[', ']', false),
            '}' => ('{', '}', false),
            _ => return None,
        };
        if noncode_at(doc, p) {
            return None;
        }
        let mut depth = 0i32;
        let mut budget = 400_000usize;
        if forward {
            let mut q = p;
            let mut chars = doc.rope.chars_at(p);
            while q < n && budget > 0 {
                let ch = chars.next()?;
                budget -= 1;
                if (ch == open || ch == close) && !noncode_at(doc, q) {
                    depth += if ch == open { 1 } else { -1 };
                    if depth == 0 {
                        return Some((p, q));
                    }
                }
                q += 1;
            }
        } else {
            let mut q = p + 1;
            let mut chars = doc.rope.chars_at(p + 1);
            while q > 0 && budget > 0 {
                let ch = chars.prev()?;
                q -= 1;
                budget -= 1;
                if (ch == open || ch == close) && !noncode_at(doc, q) {
                    depth += if ch == close { 1 } else { -1 };
                    if depth == 0 {
                        return Some((p, q));
                    }
                }
            }
        }
        None
    };
    try_at(pos).or_else(|| pos.checked_sub(1).and_then(try_at))
}

fn noncode_at(doc: &Document, pos: usize) -> bool {
    let Some(h) = &doc.highlighter else { return false };
    let (line, col) = doc.line_col(pos);
    h.is_noncode(line, col)
}

pub fn jump_to_bracket(doc: &Document, cur: &mut Cursors, extend: bool) {
    cur.map(|s| match matching_bracket(doc, s.head) {
        Some((_, m)) => {
            let head = m;
            Selection { anchor: if extend { s.anchor } else { head }, head, goal: None }
        }
        None => s,
    });
}

/// Innermost bracket pair around the range, as (open, close) offsets.
pub fn enclosing_brackets(doc: &Document, start: usize, end: usize) -> Option<(usize, usize)> {
    let mut p = start;
    let mut depth: [i32; 3] = [0; 3];
    let idx = |c| match c {
        '(' | ')' => 0,
        '[' | ']' => 1,
        _ => 2,
    };
    let mut budget = 400_000;
    while p > 0 && budget > 0 {
        p -= 1;
        budget -= 1;
        let c = doc.rope.char(p);
        if !matches!(c, '(' | ')' | '[' | ']' | '{' | '}') || noncode_at(doc, p) {
            continue;
        }
        if matches!(c, ')' | ']' | '}') {
            depth[idx(c)] += 1;
        } else if depth[idx(c)] > 0 {
            depth[idx(c)] -= 1;
        } else if let Some((_, q)) = matching_bracket_exact(doc, p)
            && q >= end
        {
            return Some((p, q));
        }
    }
    None
}

fn matching_bracket_exact(doc: &Document, p: usize) -> Option<(usize, usize)> {
    matching_bracket(doc, p).filter(|(a, _)| *a == p)
}

/// Grows word → inside brackets → with brackets → lines → everything.
pub fn expand_selection(doc: &Document, s: Selection) -> Selection {
    let (a, b) = (s.start(), s.end());
    // With a syntax tree, the next node out. Otherwise, brackets and lines.
    if let Some(st) = doc.structure.as_ref().filter(|st| st.is_current(doc.revision)) {
        let (ba, bb) = (doc.rope.char_to_byte(a), doc.rope.char_to_byte(b));
        if let Some((na, nb)) = st.expand(ba, bb) {
            return Selection::new(doc.rope.byte_to_char(na), doc.rope.byte_to_char(nb));
        }
    }
    if a == b {
        let (wa, wb) = word_or_run_at(doc, a);
        if wa != wb {
            return Selection::new(wa, wb);
        }
    }
    if let Some((open, close)) = enclosing_brackets(doc, a, b) {
        if a == open + 1 && b == close {
            return Selection::new(open, close + 1);
        }
        return Selection::new(open + 1, close);
    }
    let (l0, l1) = (doc.char_to_line(a), doc.char_to_line(b));
    let (la, lb) = (doc.line_start(l0), doc.line_end(l1));
    if a != la || b != lb {
        return Selection::new(la, lb);
    }
    Selection::new(0, doc.len_chars())
}

/// Text for the clipboard: per-cursor parts, and whether it was taken as whole lines.
pub fn copy_text(doc: &Document, cur: &Cursors) -> (Vec<String>, bool) {
    if cur.iter().all(|s| s.is_empty()) {
        let mut parts = Vec::new();
        for (l0, l1) in line_blocks(doc, cur) {
            let mut t = doc.slice_string(doc.line_start(l0), doc.line_end(l1));
            t.push('\n');
            parts.push(t);
        }
        return (parts, true);
    }
    (cur.iter().map(|s| doc.slice_string(s.start(), s.end())).collect(), false)
}

pub fn cut(doc: &mut Document, cur: &mut Cursors) -> (Vec<String>, bool) {
    let out = copy_text(doc, cur);
    if out.1 {
        delete_lines(doc, cur);
    } else {
        per_selection(doc, cur, EditKind::Other, |_, s| (Edit::delete(s.start(), s.end()), 0, 0));
    }
    out
}

/// Words in the document starting with `prefix`, nearest to the cursor first.
pub fn words_with_prefix(doc: &Document, prefix: &str, cursor: usize, limit: usize) -> Vec<String> {
    if prefix.is_empty() {
        return Vec::new();
    }
    // Compared a char at a time while scanning: a word is only built once it has the prefix, so
    // the scan allocates for the few candidates and not for every word around the cursor.
    let lower: Vec<char> = prefix.chars().flat_map(char::to_lowercase).collect();
    let prefix_len = prefix.len();
    let mut seen = std::collections::HashMap::<String, usize>::new();
    let mut word = String::new();
    let mut start = 0;
    let mut in_word = false;
    // How many of the prefix's lowercase chars the current word has matched; None once it can't.
    let mut matched: Option<usize> = Some(0);
    // Only around the cursor: that's where the useful words are, and it keeps huge files fast.
    const WINDOW: usize = 300_000;
    let from = cursor.saturating_sub(WINDOW);
    let to = (cursor + WINDOW).min(doc.len_chars());
    let mut finish = |word: &mut String, start: usize, end: usize, matched: Option<usize>| {
        if matched == Some(lower.len()) {
            // The word being typed right now doesn't complete itself.
            let is_current = start <= cursor && cursor <= end;
            if !is_current && word.len() > prefix_len {
                let d = start.abs_diff(cursor);
                match seen.get_mut(word.as_str()) {
                    Some(x) => *x = (*x).min(d),
                    None => {
                        seen.insert(word.clone(), d);
                    }
                }
            }
        }
        word.clear();
    };
    let mut i = from;
    let (chunks, chunk_byte, _, _) = doc.rope.chunks_at_char(from);
    let mut skip = doc.rope.char_to_byte(from) - chunk_byte;
    'outer: for chunk in chunks {
        let piece = &chunk[skip..];
        skip = 0;
        for c in piece.chars() {
            if i >= to {
                break 'outer;
            }
            if is_word_char(c) {
                if !in_word {
                    in_word = true;
                    start = i;
                    matched = Some(0);
                }
                if let Some(k) = matched {
                    if k < lower.len() {
                        let mut lc = c.to_lowercase();
                        let mut k = k;
                        let mut ok = true;
                        for l in lc.by_ref() {
                            if k == lower.len() {
                                break;
                            }
                            if lower[k] == l {
                                k += 1;
                            } else {
                                ok = false;
                                break;
                            }
                        }
                        matched = ok.then_some(k);
                    }
                    if matched.is_some() {
                        word.push(c);
                    }
                }
            } else if in_word {
                in_word = false;
                finish(&mut word, start, i, matched);
            }
            i += 1;
        }
    }
    if in_word {
        finish(&mut word, start, i.min(to), matched);
    }
    let mut v: Vec<(String, usize)> = seen.into_iter().collect();
    v.sort_unstable_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
    v.truncate(limit);
    v.into_iter().map(|(w, _)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Folds;
    use crate::language;
    use crate::syntax::{Highlighter, theme_by_name};
    use std::time::Duration;

    /// Builds a document from text where `|` marks cursors and `[`..`]`-like markers are not used.
    fn setup(src: &str) -> (Document, Cursors) {
        let mut text = String::new();
        let mut sels = Vec::new();
        let mut i = 0;
        for c in src.chars() {
            if c == '|' {
                sels.push(Selection::cursor(i));
            } else {
                text.push(c);
                i += 1;
            }
        }
        if sels.is_empty() {
            sels.push(Selection::cursor(0));
        }
        (Document::new(&text), Cursors::from_vec(sels, 0))
    }

    fn rust(src: &str) -> (Document, Cursors) {
        let (mut d, c) = setup(src);
        d.language = language::by_name("Rust");
        (d, c)
    }

    fn show(d: &Document, c: &Cursors) -> String {
        let mut s = d.rope.to_string();
        let mut positions: Vec<_> = c.iter().map(|x| x.head).collect();
        positions.sort();
        for p in positions.iter().rev() {
            let b = d.rope.char_to_byte(*p);
            s.insert(b, '|');
        }
        s
    }

    fn map(d: &Document) -> DisplayMap {
        DisplayMap::new(d, &Folds::default(), None, 4)
    }

    #[test]
    fn typing_pairs_and_typing_over() {
        let (mut d, mut c) = rust("|");
        let o = EditOpts::default();
        type_text(&mut d, &mut c, "(", o);
        assert_eq!(show(&d, &c), "(|)");
        type_text(&mut d, &mut c, "a", o);
        type_text(&mut d, &mut c, ")", o);
        assert_eq!(show(&d, &c), "(a)|");
        // Next to a word nothing auto-closes.
        let (mut d, mut c) = rust("|word");
        type_text(&mut d, &mut c, "(", o);
        assert_eq!(show(&d, &c), "(|word");
        // Rust lifetimes and apostrophes don't pair.
        let (mut d, mut c) = rust("|");
        type_text(&mut d, &mut c, "'", o);
        assert_eq!(show(&d, &c), "'|");
    }

    #[test]
    fn surrounds_selection() {
        let (mut d, _) = rust("hello");
        let mut c = Cursors::single(Selection::new(0, 5));
        type_text(&mut d, &mut c, "[", EditOpts::default());
        assert_eq!(d.rope.to_string(), "[hello]");
        assert_eq!(c.primary(), Selection::new(1, 6));
    }

    #[test]
    fn multi_cursor_typing_keeps_all_cursors() {
        let (mut d, mut c) = setup("a|\nb|\nc|\n");
        type_text(&mut d, &mut c, "X", EditOpts::default());
        assert_eq!(show(&d, &c), "aX|\nbX|\ncX|\n");
        backspace(&mut d, &mut c, EditOpts::default());
        assert_eq!(show(&d, &c), "a|\nb|\nc|\n");
    }

    #[test]
    fn backspace_removes_pair_and_indent_stop() {
        let (mut d, mut c) = rust("(|)");
        backspace(&mut d, &mut c, EditOpts::default());
        assert_eq!(show(&d, &c), "|");
        let (mut d, mut c) = setup("      |x");
        backspace(&mut d, &mut c, EditOpts::default());
        assert_eq!(show(&d, &c), "    |x");
    }

    #[test]
    fn close_cursors_do_not_overlap_on_backspace() {
        let (mut d, mut c) = setup("    |  |x");
        backspace(&mut d, &mut c, EditOpts::default());
        assert!(d.rope.to_string().ends_with('x'));
    }

    #[test]
    fn newline_indents_and_splits_braces() {
        let (mut d, mut c) = rust("    fn a() {|}");
        newline(&mut d, &mut c, EditOpts::default());
        assert_eq!(show(&d, &c), "    fn a() {\n        |\n    }");
        let (mut d, mut c) = rust("    x;|");
        newline(&mut d, &mut c, EditOpts::default());
        assert_eq!(show(&d, &c), "    x;\n    |");
        let (mut d, mut c) = setup("def a():|");
        d.language = language::by_name("Python");
        newline(&mut d, &mut c, EditOpts::default());
        assert_eq!(show(&d, &c), "def a():\n    |");
    }

    #[test]
    fn word_motions() {
        let (d, _) = setup("foo.bar  baz\nqux");
        assert_eq!(word_right(&d, 0), 3);
        assert_eq!(word_right(&d, 3), 4);
        assert_eq!(word_right(&d, 7), 12);
        assert_eq!(word_right(&d, 12), 13);
        assert_eq!(word_left(&d, 12), 9);
        assert_eq!(word_left(&d, 13), 12);
        let (d, _) = setup("fooBar_baz");
        assert_eq!(subword_right(&d, 0), 3);
        assert_eq!(subword_right(&d, 3), 6);
        assert_eq!(subword_left(&d, 6), 3);
    }

    #[test]
    fn vertical_keeps_goal_column_through_short_lines() {
        let (d, mut c) = setup("long line|\nx\nlong line\n");
        let m = map(&d);
        move_cursors(&d, &mut c, &m, Motion::Down, false);
        assert_eq!(d.line_col(c.head()), (1, 1));
        move_cursors(&d, &mut c, &m, Motion::Down, false);
        assert_eq!(d.line_col(c.head()), (2, 9));
        move_cursors(&d, &mut c, &m, Motion::Up, false);
        move_cursors(&d, &mut c, &m, Motion::Up, false);
        move_cursors(&d, &mut c, &m, Motion::Up, false);
        assert_eq!(c.head(), 0);
    }

    #[test]
    fn smart_home_toggles() {
        let (d, mut c) = setup("    abc|");
        let m = map(&d);
        move_cursors(&d, &mut c, &m, Motion::LineStart, false);
        assert_eq!(c.head(), 4);
        move_cursors(&d, &mut c, &m, Motion::LineStart, false);
        assert_eq!(c.head(), 0);
    }

    #[test]
    fn graphemes_move_as_one() {
        let (d, mut c) = setup("e\u{301}x");
        let m = map(&d);
        move_cursors(&d, &mut c, &m, Motion::Right, false);
        assert_eq!(c.head(), 2);
        move_cursors(&d, &mut c, &m, Motion::Left, false);
        assert_eq!(c.head(), 0);
    }

    #[test]
    fn line_moves_and_copies() {
        let (mut d, mut c) = setup("a\nb|b\nc");
        move_lines(&mut d, &mut c, false);
        assert_eq!(show(&d, &c), "b|b\na\nc");
        move_lines(&mut d, &mut c, true);
        move_lines(&mut d, &mut c, true);
        assert_eq!(show(&d, &c), "a\nc\nb|b");
        move_lines(&mut d, &mut c, true); // at the end: nothing
        assert_eq!(show(&d, &c), "a\nc\nb|b");
        let (mut d, mut c) = setup("x|y\nz");
        copy_lines(&mut d, &mut c, true);
        assert_eq!(show(&d, &c), "xy\nx|y\nz");
        let (mut d, mut c) = setup("a\nb|\nc\n");
        delete_lines(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "a\nc\n");
        let (mut d, mut c) = setup("a\nb|");
        delete_lines(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "a");
    }

    #[test]
    fn indent_and_outdent() {
        let (mut d, _) = setup("a\nb\n");
        let mut c = Cursors::single(Selection::new(0, 3));
        indent_lines(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "    a\n    b\n");
        outdent_lines(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "a\nb\n");
        let (mut d, mut c) = setup("ab|");
        tab(&mut d, &mut c);
        assert_eq!(show(&d, &c), "ab  |");
    }

    #[test]
    fn comments_toggle_both_ways() {
        let (mut d, _) = rust("    a\n\n    b\n");
        let mut c = Cursors::single(Selection::new(0, 13));
        toggle_line_comment(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "    // a\n\n    // b\n");
        let n = d.len_chars();
        let mut c = Cursors::single(Selection::new(0, n));
        toggle_line_comment(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "    a\n\n    b\n");
        let (mut d, _) = rust("abc");
        let mut c = Cursors::single(Selection::new(0, 3));
        toggle_block_comment(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "/*abc*/");
        assert_eq!(d.slice_string(c.primary().start(), c.primary().end()), "abc");
    }

    #[test]
    fn ctrl_d_walks_occurrences_and_wraps() {
        let (d, mut c) = setup("foo bar |foo baz foo");
        assert!(add_next_occurrence(&d, &mut c));
        assert_eq!(c.primary(), Selection::new(8, 11));
        assert!(add_next_occurrence(&d, &mut c));
        assert_eq!(c.primary(), Selection::new(16, 19));
        assert!(add_next_occurrence(&d, &mut c));
        assert_eq!(c.primary(), Selection::new(0, 3));
        assert_eq!(c.len(), 3);
        assert!(!add_next_occurrence(&d, &mut c));
    }

    #[test]
    fn skip_occurrence_moves_on() {
        let (d, mut c) = setup("|foo foo foo");
        add_next_occurrence(&d, &mut c);
        add_next_occurrence(&d, &mut c);
        skip_occurrence(&d, &mut c);
        let starts: Vec<_> = c.iter().map(|s| s.start()).collect();
        assert_eq!(starts, vec![0, 8]);
    }

    #[test]
    fn select_all_occurrences_and_split_lines() {
        let (d, mut c) = setup("ab |ab ab");
        select_all_occurrences(&d, &mut c);
        assert_eq!(c.len(), 3);
        let (d, _) = setup("one\ntwo\nthree");
        let mut c = Cursors::single(Selection::new(1, 9));
        split_into_lines(&d, &mut c);
        let got: Vec<_> = c.iter().map(|s| d.slice_string(s.start(), s.end())).collect();
        assert_eq!(got, vec!["ne", "two", "t"]);
    }

    #[test]
    fn cursors_above_and_below_and_column_select() {
        let (d, mut c) = setup("abc\nab|c\nabc");
        let m = map(&d);
        add_cursor_vertical(&d, &mut c, &m, true);
        add_cursor_vertical(&d, &mut c, &m, false);
        assert_eq!(c.len(), 3);
        assert!(c.iter().all(|s| d.line_col(s.head).1 == 2));
        let (d, _) = setup("abcd\nx\nabcd");
        let m = map(&d);
        let box_sel = column_select(&d, &m, (0, 1), (2, 3));
        let got: Vec<_> = box_sel.iter().map(|s| d.slice_string(s.start(), s.end())).collect();
        assert_eq!(got, vec!["bc", "bc"]);
    }

    #[test]
    fn bracket_matching_ignores_strings() {
        let (mut d, _) = rust("fn a() { let s = \"}\"; }\n");
        let mut h = Highlighter::new("Rust", theme_by_name("COSMIC", true)).unwrap();
        while !h.is_done(d.len_lines()) {
            h.advance(&d.rope, Duration::from_millis(50));
        }
        d.highlighter = Some(h);
        assert_eq!(matching_bracket(&d, 7), Some((7, 22)));
        assert_eq!(matching_bracket(&d, 23), Some((22, 7)));
    }

    #[test]
    fn expand_selection_grows_outward() {
        let (d, _) = rust("f(a, foo)");
        let s = expand_selection(&d, Selection::cursor(6));
        assert_eq!(d.slice_string(s.start(), s.end()), "foo");
        let s = expand_selection(&d, s);
        assert_eq!(d.slice_string(s.start(), s.end()), "a, foo");
        let s = expand_selection(&d, s);
        assert_eq!(d.slice_string(s.start(), s.end()), "(a, foo)");
        let s = expand_selection(&d, s);
        assert_eq!(d.slice_string(s.start(), s.end()), "f(a, foo)");
    }

    #[test]
    fn identifiers_change_case() {
        assert_eq!(to_snake("fooBarBaz"), "foo_bar_baz");
        assert_eq!(to_camel("foo_bar-baz"), "fooBarBaz");
        assert_eq!(to_pascal("foo_bar"), "FooBar");
        assert_eq!(to_kebab("FooBar"), "foo-bar");
        assert_eq!(to_title("olá mundo"), "Olá Mundo");
    }

    #[test]
    fn sort_unique_trim_join() {
        let (mut d, mut c) = setup("c\na\nb\na\n|");
        line_op(&mut d, &mut c, LineOp::SortAsc);
        assert_eq!(d.rope.to_string(), "a\na\nb\nc\n");
        line_op(&mut d, &mut c, LineOp::Unique);
        assert_eq!(d.rope.to_string(), "a\nb\nc\n");
        let (mut d, mut c) = setup("a  \nb\t\n|");
        trim_trailing(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "a\nb\n");
        let (mut d, mut c) = setup("a|\n   b\nc");
        join_lines(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "a b\nc");
    }

    #[test]
    fn cut_without_selection_takes_the_line_and_paste_puts_it_above() {
        let (mut d, mut c) = setup("a\nb|\nc");
        let (parts, whole) = cut(&mut d, &mut c);
        assert!(whole);
        assert_eq!(parts, vec!["b\n"]);
        assert_eq!(d.rope.to_string(), "a\nc");
        paste_lines(&mut d, &mut c, &parts[0]);
        assert_eq!(d.rope.to_string(), "a\nb\nc");
    }

    #[test]
    fn multi_cursor_paste_distributes() {
        let (mut d, mut c) = setup("|\n|\n");
        paste_each(&mut d, &mut c, &["one".into(), "two".into()]);
        assert_eq!(show(&d, &c), "one|\ntwo|\n");
    }

    #[test]
    fn duplicate_selection_selects_the_copy() {
        let (mut d, _) = setup("abc");
        let mut c = Cursors::single(Selection::new(0, 2));
        duplicate(&mut d, &mut c);
        assert_eq!(d.rope.to_string(), "ababc");
        assert_eq!(c.primary(), Selection::new(2, 4));
    }

    #[test]
    fn word_completion_prefers_nearby_and_skips_itself() {
        let (d, _) = setup("format formula far\nfor|");
        let got = words_with_prefix(&d, "for", d.len_chars(), 10);
        assert_eq!(got, vec!["formula", "format"]);
    }

    #[test]
    fn undo_restores_multi_cursor_edit() {
        let (mut d, mut c) = setup("|a\n|b\n");
        type_text(&mut d, &mut c, "x", EditOpts::default());
        assert_eq!(d.rope.to_string(), "xa\nxb\n");
        d.undo(&mut c);
        assert_eq!(show(&d, &c), "|a\n|b\n");
    }

    #[test]
    fn utf8_positions_are_chars_not_bytes() {
        let (mut d, mut c) = setup("ação|");
        type_text(&mut d, &mut c, "é", EditOpts::default());
        assert_eq!(show(&d, &c), "açãoé|");
        backspace(&mut d, &mut c, EditOpts::default());
        assert_eq!(show(&d, &c), "ação|");
    }

    /// The scan compares as it goes; it must find what lowercasing every word would.
    #[test]
    fn words_with_prefix_matches_the_plain_way() {
        fn plain(text: &str, prefix: &str, cursor: usize) -> Vec<String> {
            let lower = prefix.to_lowercase();
            let chars: Vec<char> = text.chars().collect();
            let mut seen = std::collections::HashMap::<String, usize>::new();
            let mut i = 0;
            while i < chars.len() {
                if !is_word_char(chars[i]) {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < chars.len() && is_word_char(chars[i]) {
                    i += 1;
                }
                let w: String = chars[start..i].iter().collect();
                let current = start <= cursor && cursor <= i;
                if !current && w.len() > prefix.len() && w.to_lowercase().starts_with(&lower) {
                    let d = start.abs_diff(cursor);
                    seen.entry(w).and_modify(|x| *x = (*x).min(d)).or_insert(d);
                }
            }
            let mut v: Vec<(String, usize)> = seen.into_iter().collect();
            v.sort_by(|a, b| (a.1, &a.0).cmp(&(b.1, &b.0)));
            v.into_iter().map(|(w, _)| w).collect()
        }
        let text = "Ação acaso ACENO açúcar a_b ab9 Ab ábaco\nfn Fooo foo_bar FOOD fo\tfoé fooo\nİstanbul istmo ß straße\n";
        let (d, _) = setup(text);
        let n = d.len_chars();
        for prefix in ["a", "A", "ac", "Aç", "fo", "FOO", "i", "İ", "st", "x", "ab"] {
            for cursor in [0, 3, 10, n / 2, n] {
                assert_eq!(words_with_prefix(&d, prefix, cursor, 100), plain(text, prefix, cursor), "{prefix:?} at {cursor}");
            }
        }
    }

    #[test]
    fn ascii_and_unicode_classes_agree() {
        for c in (0u32..0x3000).filter_map(char::from_u32) {
            let slow = if c == '\n' {
                CharClass::Newline
            } else if c.is_whitespace() {
                CharClass::Space
            } else if c.is_alphanumeric() || c == '_' {
                CharClass::Word
            } else {
                CharClass::Punct
            };
            assert!(class(c) == slow, "{c:?}");
            assert_eq!(is_word_char(c), slow == CharClass::Word, "{c:?}");
        }
    }
}

