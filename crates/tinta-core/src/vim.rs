//! Vim's modal editing as a state machine over the same document and cursors as everything else.
//! Keys go in; what needs the application (saving, opening the command line) comes out as actions.

use crate::buffer::{Assoc, Document, Edit, EditKind};
use crate::commands::{self, EditOpts};
use crate::cursor::{Cursors, Selection};
use crate::display::{DisplayMap, char_to_vcol};
use regex::RegexBuilder;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Replace,
    Visual,
    VisualLine,
    VisualBlock,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Replace => "REPLACE",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::VisualBlock => "V-BLOCK",
        }
    }

    fn is_visual(self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine | Mode::VisualBlock)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Key {
    /// A typed character, when it is one.
    pub ch: Option<char>,
    /// `escape`, `enter`, `backspace`, `delete`, `tab`, arrows, `home`, `end`, `pageup`, `pagedown`.
    pub name: &'static str,
    pub ctrl: bool,
}

impl Key {
    pub fn ch(c: char) -> Self {
        Self { ch: Some(c), name: "", ctrl: false }
    }
    pub fn ctrl(c: char) -> Self {
        Self { ch: Some(c), name: "", ctrl: true }
    }
    pub fn named(name: &'static str) -> Self {
        Self { ch: None, name, ctrl: false }
    }
}

/// What the application has to do on Vim's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Save,
    SaveAll,
    Quit { force: bool },
    SaveQuit,
    QuitAll { force: bool },
    OpenFile(String),
    NewTab,
    CloseTab { force: bool },
    SplitRight,
    NextTab,
    PrevTab,
    NextGroup,
    PrevGroup,
    /// Opens the `:`, `/` or `?` line.
    CommandLine(char),
    /// Goes to the system clipboard (`"+y`).
    SetClipboard(String),
    /// Text wanted from the system clipboard (`"+p`).
    PasteClipboard { before: bool },
    /// A command from the editor's own table, by id.
    Run(&'static str),
    Center,
    ScrollTop,
    ScrollBottom,
    /// Highlight these matches, or none.
    Highlight(Option<String>),
    SetOption(String),
    Message(String),
}

#[derive(Debug, Default)]
pub struct Outcome {
    /// False: not Vim's, let the editor handle the key as usual.
    pub handled: bool,
    pub actions: Vec<Action>,
}

pub struct Ctx<'a> {
    pub doc: &'a mut Document,
    pub cur: &'a mut Cursors,
    pub map: &'a DisplayMap,
    pub page_rows: usize,
    /// The first row on screen, for H, M and L.
    pub first_row: usize,
    pub opts: EditOpts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Outdent,
    Upper,
    Lower,
    SwapCase,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exclusive,
    Inclusive,
    Line,
}

/// A key waiting for another one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    G,
    Z,
    Find { forward: bool, till: bool },
    Replace,
    Mark,
    GotoMark { line: bool },
    Register,
    TextObject { around: bool },
    Window,
    RecordInto,
    PlayFrom,
}

#[derive(Debug, Clone)]
struct Reg {
    text: String,
    linewise: bool,
}

pub struct Vim {
    pub mode: Mode,
    count: Option<usize>,
    op: Option<(Op, usize)>,
    pending: Pending,
    register: Option<char>,
    regs: HashMap<char, Reg>,
    marks: HashMap<char, usize>,
    /// Where visual mode started.
    anchor: usize,
    /// (row, visual column) of a block's anchor and head.
    block: ((usize, usize), (usize, usize)),
    last_find: Option<(char, bool, bool)>,
    pub last_search: Option<(String, bool)>,
    /// Goal column for j and k.
    goal: Option<usize>,
    last_change: Vec<Key>,
    change_keys: Vec<Key>,
    in_change: bool,
    replaying: bool,
    recording: Option<char>,
    macro_keys: Vec<Key>,
    macros: HashMap<char, Vec<Key>>,
    last_macro: Option<char>,
    /// `3ihello<esc>`: how many more times to repeat what was typed.
    insert_repeat: usize,
    insert_keys: Vec<Key>,
    last_visual: Option<(Mode, usize, usize)>,
}

impl Default for Vim {
    fn default() -> Self {
        Self::new()
    }
}

fn class(c: char, big: bool) -> u8 {
    if c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

fn opener(c: char) -> Option<(char, char)> {
    Some(match c {
        '(' | ')' | 'b' => ('(', ')'),
        '[' | ']' => ('[', ']'),
        '{' | '}' | 'B' => ('{', '}'),
        '<' | '>' => ('<', '>'),
        _ => return None,
    })
}

impl Vim {
    pub fn new() -> Self {
        Self {
            mode: Mode::Normal,
            count: None,
            op: None,
            pending: Pending::None,
            register: None,
            regs: HashMap::new(),
            marks: HashMap::new(),
            anchor: 0,
            block: ((0, 0), (0, 0)),
            last_find: None,
            last_search: None,
            goal: None,
            last_change: Vec::new(),
            change_keys: Vec::new(),
            in_change: false,
            replaying: false,
            recording: None,
            macro_keys: Vec::new(),
            macros: HashMap::new(),
            last_macro: None,
            insert_repeat: 0,
            insert_keys: Vec::new(),
            last_visual: None,
        }
    }

    /// What the bottom bar shows: mode, and keys typed so far.
    pub fn status(&self) -> String {
        let mut s = format!("-- {} --", self.mode.label());
        if let Some(r) = self.recording {
            s.push_str(&format!(" recording @{r}"));
        }
        if let Some(c) = self.count {
            s.push_str(&format!(" {c}"));
        }
        s
    }

    /// In the middle of something, like the `x` of `fx` to come.
    pub fn is_waiting(&self) -> bool {
        self.pending != Pending::None
    }

    pub fn is_insert(&self) -> bool {
        matches!(self.mode, Mode::Insert | Mode::Replace)
    }

    /// The caret sits on a char in Normal mode: never past the end of a line with text.
    pub fn clamp_caret(&self, ctx: &mut Ctx<'_>) {
        if self.mode == Mode::Normal {
            let p = clamp_normal(ctx.doc, ctx.cur.head());
            ctx.cur.set_single(Selection::cursor(p));
        }
    }

    /// The mouse selected something or moved the caret.
    pub fn after_mouse(&mut self, ctx: &mut Ctx<'_>) {
        if self.is_insert() {
            return;
        }
        let p = ctx.cur.primary();
        if p.is_empty() {
            if self.mode.is_visual() {
                self.mode = Mode::Normal;
            }
            self.clamp_caret(ctx);
        } else {
            self.mode = Mode::Visual;
            self.anchor = p.anchor;
            if p.head > p.anchor {
                ctx.cur.set_single(Selection::new(p.anchor, p.head));
            }
        }
    }

    pub fn key(&mut self, ctx: &mut Ctx<'_>, key: Key) -> Outcome {
        let mut out = Outcome { handled: true, actions: Vec::new() };
        // Stop recording before the q that stops it gets into the macro.
        if self.recording.is_some() && !self.replaying && self.pending == Pending::None && !self.is_insert() && key == Key::ch('q') {
            let r = self.recording.take().unwrap();
            self.macros.insert(r, std::mem::take(&mut self.macro_keys));
            return out;
        }
        if self.recording.is_some() && !self.replaying {
            self.macro_keys.push(key.clone());
        }
        if self.in_change && !self.replaying {
            self.change_keys.push(key.clone());
        }
        match self.mode {
            Mode::Insert | Mode::Replace => self.insert_key(ctx, key, &mut out),
            _ => self.normal_key(ctx, key, &mut out),
        }
        out
    }

    // Insert mode --------------------------------------------------------------------------

    fn insert_key(&mut self, ctx: &mut Ctx<'_>, key: Key, out: &mut Outcome) {
        let esc = key.name == "escape" || (key.ctrl && key.ch == Some('['));
        if esc {
            // 3ihello<esc> types the rest of it now.
            let repeat = std::mem::take(&mut self.insert_repeat);
            let keys = std::mem::take(&mut self.insert_keys);
            let was_replay = self.replaying;
            self.replaying = true;
            for _ in 0..repeat {
                for k in &keys {
                    self.apply_insert_key(ctx, k);
                }
            }
            self.replaying = was_replay;
            self.mode = Mode::Normal;
            ctx.doc.end_group();
            ctx.cur.map(|s| Selection::cursor(step_left_in_line(ctx.doc, s.head)));
            let sels: Vec<_> = ctx.cur.as_slice().to_vec();
            if sels.len() > 1 {
                ctx.cur.keep_primary();
            }
            self.marks.insert('^', ctx.cur.head());
            self.end_change();
            return;
        }
        if self.insert_repeat > 0 {
            self.insert_keys.push(key.clone());
        }
        // Normally the editor does the typing, with its suggestions and pairs. On a replay of `.`
        // or a macro there's no one else to do it.
        if self.replaying || self.mode == Mode::Replace {
            self.apply_insert_key(ctx, &key);
        } else {
            out.handled = false;
        }
    }

    fn apply_insert_key(&mut self, ctx: &mut Ctx<'_>, key: &Key) {
        match key.name {
            "enter" => commands::newline(ctx.doc, ctx.cur, ctx.opts),
            "backspace" => commands::backspace(ctx.doc, ctx.cur, ctx.opts),
            "delete" => commands::delete_forward(ctx.doc, ctx.cur),
            "tab" => commands::tab(ctx.doc, ctx.cur),
            _ => {
                let Some(c) = key.ch.filter(|_| !key.ctrl) else { return };
                if self.mode == Mode::Replace {
                    let h = ctx.cur.head();
                    if ctx.doc.char_at(h).is_some_and(|x| x != '\n') {
                        let mut t = String::new();
                        t.push(c);
                        ctx.doc.apply(vec![Edit::new(h, h + 1, t)], ctx.cur, EditKind::Other, |m, c| {
                            c.set_single(Selection::cursor(m.after_edit(0)));
                        });
                        return;
                    }
                }
                // The same way the editor types it, so `.` gives the same text, pairs included.
                commands::type_text(ctx.doc, ctx.cur, &c.to_string(), ctx.opts);
            }
        }
    }

    /// `.` repeats what the user typed through the editor, so it has to hear about it. The editor
    /// tells us the closing bracket it added itself by way of the keys that made it happen.
    fn begin_change(&mut self, keys: &[Key]) {
        if self.replaying {
            return;
        }
        self.in_change = true;
        self.change_keys = keys.to_vec();
    }

    fn end_change(&mut self) {
        if self.in_change {
            self.in_change = false;
            self.last_change = std::mem::take(&mut self.change_keys);
        }
    }

    fn enter_insert(&mut self, ctx: &mut Ctx<'_>, repeat: usize) {
        self.mode = Mode::Insert;
        self.insert_repeat = repeat.saturating_sub(1);
        self.insert_keys.clear();
        ctx.doc.begin_group();
    }

    // Normal and visual ----------------------------------------------------------------------

    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    fn reset(&mut self) {
        self.count = None;
        self.op = None;
        self.pending = Pending::None;
        self.register = None;
    }

    fn normal_key(&mut self, ctx: &mut Ctx<'_>, key: Key, out: &mut Outcome) {
        let pending = std::mem::replace(&mut self.pending, Pending::None);
        if key.name == "escape" || (key.ctrl && key.ch == Some('[')) {
            if self.mode.is_visual() {
                self.leave_visual(ctx);
            }
            self.reset();
            ctx.cur.keep_primary();
            let h = ctx.cur.head();
            ctx.cur.set_single(Selection::cursor(clamp_normal(ctx.doc, h)));
            out.actions.push(Action::Highlight(None));
            return;
        }
        if pending != Pending::None {
            self.pending_key(ctx, pending, key, out);
            return;
        }
        if key.ctrl {
            self.ctrl_key(ctx, key, out);
            return;
        }
        let c = match (key.ch, key.name) {
            (Some(c), _) => c,
            (None, "left" | "backspace") => 'h',
            (None, "right") => 'l',
            (None, "up") => 'k',
            (None, "down" | "enter") => 'j',
            (None, "home") => '0',
            (None, "end") => '$',
            (None, "pageup") => return self.ctrl_key(ctx, Key::ctrl('b'), out),
            (None, "pagedown") => return self.ctrl_key(ctx, Key::ctrl('f'), out),
            (None, "delete") => 'x',
            _ => {
                out.handled = false;
                return;
            }
        };
        // Counts. A 0 with no digits yet is a motion.
        if c.is_ascii_digit() && (c != '0' || self.count.is_some()) {
            self.count = Some(self.count.unwrap_or(0).saturating_mul(10).saturating_add(c.to_digit(10).unwrap() as usize));
            return;
        }
        match c {
            '"' => self.pending = Pending::Register,
            'g' => self.pending = Pending::G,
            'z' => self.pending = Pending::Z,
            'f' | 'F' | 't' | 'T' => self.pending = Pending::Find { forward: c.is_lowercase(), till: c.eq_ignore_ascii_case(&'t') },
            'r' => self.pending = Pending::Replace,
            'm' => self.pending = Pending::Mark,
            '\'' => self.pending = Pending::GotoMark { line: true },
            '`' => self.pending = Pending::GotoMark { line: false },
            'q' => self.pending = Pending::RecordInto,
            '@' => self.pending = Pending::PlayFrom,
            ':' => {
                out.actions.push(Action::CommandLine(':'));
                self.count = None;
            }
            '/' => out.actions.push(Action::CommandLine('/')),
            '?' => out.actions.push(Action::CommandLine('?')),
            'i' | 'a' if self.op.is_some() || self.mode.is_visual() => self.pending = Pending::TextObject { around: c == 'a' },
            _ => {
                if self.motion_key(ctx, c, out) {
                    return;
                }
                if self.mode.is_visual() {
                    self.visual_command(ctx, c, out);
                } else {
                    self.normal_command(ctx, c, out);
                }
            }
        }
    }

    /// Runs a motion, moving the caret or feeding the waiting operator. False if it isn't one.
    fn motion_key(&mut self, ctx: &mut Ctx<'_>, c: char, out: &mut Outcome) -> bool {
        let n = self.count.unwrap_or(1);
        let has_count = self.count.is_some();
        // In `cw`, `w` means up to the end of the word.
        let pos = self.caret(ctx);
        if matches!(c, 'w' | 'W') && matches!(self.op, Some((Op::Change, _))) && ctx.doc.char_at(pos).is_some_and(|x| !x.is_whitespace()) {
            // ...starting from this word's own end, which `e` on its last char would skip past.
            let big = c == 'W';
            let k = class(ctx.doc.rope.char(pos), big);
            let mut t = pos;
            while ctx.doc.char_at(t + 1).is_some_and(|x| class(x, big) == k) {
                t += 1;
            }
            if n > 1 {
                t = word_end(ctx.doc, t, n - 1, big);
            }
            self.count = None;
            self.apply_motion(ctx, pos, t, Kind::Inclusive, out);
            return true;
        }
        let m = c;
        let Some((target, kind)) = self.motion(ctx, m, n, has_count, pos) else {
            return matches!(c, 'h' | 'j' | 'k' | 'l' | 'w' | 'b' | 'e' | 'W' | 'B' | 'E' | '0' | '^' | '$' | 'G' | '%' | '{' | '}' | 'H' | 'M' | 'L' | 'n' | 'N' | '*' | '#' | ';' | ',' | '|' | '_' | '+' | '-')
                && {
                    self.reset();
                    true
                };
        };
        self.count = None;
        if matches!(c, '*' | '#') {
            let (a, b) = commands::word_at(ctx.doc, pos);
            out.actions.push(Action::Highlight(Some(regex::escape(&ctx.doc.slice_string(a, b)))));
        }
        if matches!(c, 'n' | 'N')
            && let Some((q, _)) = &self.last_search
        {
            out.actions.push(Action::Highlight(Some(q.clone())));
        }
        self.apply_motion(ctx, pos, target, kind, out);
        true
    }

    fn apply_motion(&mut self, ctx: &mut Ctx<'_>, from: usize, target: usize, kind: Kind, out: &mut Outcome) {
        if let Some((op, op_count)) = self.op.take() {
            let _ = op_count;
            let (a, b) = if target < from { (target, from) } else { (from, target) };
            let linewise = kind == Kind::Line;
            let (a, b) = if linewise {
                line_span(ctx.doc, a, b)
            } else if kind == Kind::Inclusive {
                (a, (b + 1).min(ctx.doc.len_chars()))
            } else {
                // `dw` at the end of a line stops short of its newline.
                (a, b)
            };
            self.apply_op(ctx, op, a, b, linewise, out);
            return;
        }
        if !matches!(kind, Kind::Line) || !self.mode.is_visual() {
            // j and k keep their column; everything else sets a new one.
        }
        self.set_caret(ctx, target);
    }

    fn caret(&self, ctx: &Ctx<'_>) -> usize {
        if self.mode == Mode::VisualBlock {
            return ctx.map.row_to_pos(ctx.doc, self.block.1.0, self.block.1.1);
        }
        if self.mode == Mode::VisualLine {
            return self.block.1.0;
        }
        if self.mode.is_visual() {
            // The selection ends just past the char under the caret when it runs forward.
            let p = ctx.cur.primary();
            if p.head > self.anchor { p.head - 1 } else { p.head }
        } else {
            ctx.cur.head()
        }
    }

    fn set_caret(&mut self, ctx: &mut Ctx<'_>, pos: usize) {
        match self.mode {
            Mode::Visual => {
                // The anchor can sit at the very end, or past it once the text got shorter.
                let len = ctx.doc.len_chars();
                let anchor = self.anchor.min(len);
                let s = if pos >= anchor {
                    Selection::new(anchor, (pos + 1).min(len))
                } else {
                    Selection::new((anchor + 1).min(len), pos)
                };
                ctx.cur.set_single(s);
            }
            Mode::VisualLine => {
                let (la, lb) = (ctx.doc.char_to_line(self.anchor), ctx.doc.char_to_line(pos));
                let total = ctx.doc.len_lines();
                let line_end = |l: usize| if l + 1 < total { ctx.doc.line_start(l + 1) } else { ctx.doc.len_chars() };
                let s = if lb >= la {
                    Selection::new(ctx.doc.line_start(la), line_end(lb).max(pos))
                } else {
                    Selection::new(line_end(la), ctx.doc.line_start(lb))
                };
                self.block.1 = (pos, 0);
                ctx.cur.set_single(s);
            }
            Mode::VisualBlock => {
                let (row, v) = ctx.map.pos_to_row(ctx.doc, pos);
                self.block.1 = (row, v);
                *ctx.cur = commands::column_select(ctx.doc, ctx.map, self.block.0, (row, v + 1));
            }
            _ => {
                let p = clamp_normal(ctx.doc, pos);
                ctx.cur.set_single(Selection::cursor(p));
            }
        }
    }

    /// Where a motion lands. `None` means it failed, as `fx` with no x does.
    fn motion(&mut self, ctx: &mut Ctx<'_>, c: char, n: usize, has_count: bool, pos: usize) -> Option<(usize, Kind)> {
        let doc = &*ctx.doc;
        let total = doc.len_chars();
        let (line, col) = doc.line_col(pos);
        if !matches!(c, 'j' | 'k' | '-' | '+') {
            self.goal = None;
        }
        Some(match c {
            'h' => (pos.saturating_sub(n.min(col)), Kind::Exclusive),
            'l' => {
                // An operator can reach just past the last char: `dl` on it deletes it.
                let last = doc.line_len(line);
                let max = if self.op.is_some() || self.mode.is_visual() { last } else { last.saturating_sub(1) };
                (doc.line_start(line) + (col + n).min(max.max(col)), Kind::Exclusive)
            }
            'j' | 'k' | '+' | '-' => {
                let down = matches!(c, 'j' | '+');
                let target = if down { line + n } else { line.checked_sub(n)? };
                if target >= doc.len_lines() || (down && target == doc.len_lines() - 1 && doc.line_len(target) == 0 && target != line + 1) {
                    return None;
                }
                if matches!(c, '+' | '-') {
                    return Some((first_nonblank(doc, target), Kind::Line));
                }
                let tab = doc.indent.width.max(1);
                let goal = *self.goal.get_or_insert_with(|| char_to_vcol(doc.line_slice(line), col, tab));
                (doc.line_start(target) + crate::display::vcol_to_char(doc.line_slice(target), goal, tab), Kind::Line)
            }
            'w' | 'W' => (word_forward(doc, pos, n, c == 'W'), Kind::Exclusive),
            'b' | 'B' => (word_back(doc, pos, n, c == 'B'), Kind::Exclusive),
            'e' | 'E' => (word_end(doc, pos, n, c == 'E'), Kind::Inclusive),
            '0' => (doc.line_start(line), Kind::Exclusive),
            '^' | '_' => (first_nonblank(doc, line), Kind::Exclusive),
            '$' => {
                let l = (line + n - 1).min(doc.len_lines() - 1);
                let end = doc.line_end(l);
                (if end > doc.line_start(l) { end - 1 } else { end }, Kind::Inclusive)
            }
            '|' => (doc.line_start(line) + (n - 1).min(doc.line_len(line)), Kind::Exclusive),
            'G' => {
                let last = last_text_line(doc);
                let l = if has_count { (n - 1).min(last) } else { last };
                (first_nonblank(doc, l), Kind::Line)
            }
            'H' | 'M' | 'L' => {
                let rows = ctx.map.row_count(doc).max(1);
                let top = ctx.first_row;
                let bottom = (top + ctx.page_rows.max(1) - 1).min(rows - 1);
                let row = match c {
                    'H' => (top + n - 1).min(bottom),
                    'M' => top + (bottom - top) / 2,
                    _ => bottom.saturating_sub(n - 1).max(top),
                };
                let target = (ctx.map.row(doc, row).line as usize).min(last_text_line(doc));
                (first_nonblank(doc, target), Kind::Line)
            }
            '{' | '}' => {
                let mut l = line as isize;
                let step = if c == '}' { 1isize } else { -1 };
                for _ in 0..n {
                    while l >= 0 && (l as usize) < doc.len_lines() && doc.line_is_blank(l as usize) {
                        l += step;
                    }
                    while l >= 0 && (l as usize) < doc.len_lines() && !doc.line_is_blank(l as usize) {
                        l += step;
                    }
                }
                let target = if l < 0 { 0 } else if l as usize >= doc.len_lines() { total } else { doc.line_start(l as usize) };
                (target, Kind::Exclusive)
            }
            '%' => {
                let line_end = doc.line_end(line);
                let mut p = pos;
                while p < line_end && !matches!(doc.char_at(p), Some('(' | ')' | '[' | ']' | '{' | '}')) {
                    p += 1;
                }
                let (_, m) = commands::matching_bracket(doc, p).filter(|(a, _)| *a == p)?;
                (m, Kind::Inclusive)
            }
            ';' | ',' => {
                let (ch, forward, till) = self.last_find?;
                let forward = if c == ',' { !forward } else { forward };
                (find_in_line(doc, pos, ch, forward, till, n, true)?, if forward { Kind::Inclusive } else { Kind::Exclusive })
            }
            'n' | 'N' => {
                let (q, forward) = self.last_search.clone()?;
                let forward = if c == 'N' { !forward } else { forward };
                (search_from(doc, &q, pos, forward, n)?, Kind::Exclusive)
            }
            '*' | '#' => {
                let (a, b) = commands::word_at(doc, pos);
                if a == b {
                    return None;
                }
                let q = format!(r"\b{}\b", regex::escape(&doc.slice_string(a, b)));
                let forward = c == '*';
                let t = search_from(doc, &q, if forward { b.saturating_sub(1) } else { a }, forward, n)?;
                self.last_search = Some((q, forward));
                (t, Kind::Exclusive)
            }
            _ => return None,
        })
    }

    fn pending_key(&mut self, ctx: &mut Ctx<'_>, pending: Pending, key: Key, out: &mut Outcome) {
        let Some(c) = key.ch else {
            self.reset();
            return;
        };
        let pos = self.caret(ctx);
        match pending {
            Pending::None => {}
            Pending::Register => {
                self.register = Some(c);
            }
            Pending::RecordInto => {
                if c.is_ascii_alphanumeric() {
                    self.recording = Some(c);
                    self.macro_keys.clear();
                }
            }
            Pending::PlayFrom => {
                let name = if c == '@' { self.last_macro } else { Some(c) };
                if let Some(name) = name
                    && let Some(keys) = self.macros.get(&name).cloned()
                {
                    self.last_macro = Some(name);
                    let n = self.take_count();
                    let was = self.replaying;
                    self.replaying = true;
                    for _ in 0..n {
                        for k in &keys {
                            let o = self.key(ctx, k.clone());
                            out.actions.extend(o.actions);
                        }
                    }
                    self.replaying = was;
                }
            }
            Pending::Find { forward, till } => {
                let n = self.take_count();
                self.last_find = Some((c, forward, till));
                match find_in_line(ctx.doc, pos, c, forward, till, n, false) {
                    Some(t) => self.apply_motion(ctx, pos, t, if forward { Kind::Inclusive } else { Kind::Exclusive }, out),
                    None => self.reset(),
                }
            }
            Pending::Replace => {
                let n = self.take_count();
                if self.mode.is_visual() {
                    let s = ctx.cur.primary();
                    let text: String = ctx.doc.slice_string(s.start(), s.end()).chars().map(|x| if x == '\n' { x } else { c }).collect();
                    let (a, b) = (s.start(), s.end());
                    ctx.doc.apply(vec![Edit::new(a, b, text)], ctx.cur, EditKind::Other, |_, cur| cur.set_single(Selection::cursor(a)));
                    self.mode = Mode::Normal;
                    return;
                }
                let end = ctx.doc.line_end(ctx.doc.char_to_line(pos));
                if pos + n > end {
                    return;
                }
                self.begin_change(&[Key::ch('r'), Key::ch(c)]);
                let text: String = std::iter::repeat_n(c, n).collect();
                ctx.doc.apply(vec![Edit::new(pos, pos + n, text)], ctx.cur, EditKind::Other, |_, cur| {
                    cur.set_single(Selection::cursor(pos + n - 1));
                });
                self.end_change();
            }
            Pending::Mark => {
                self.marks.insert(c, pos);
            }
            Pending::GotoMark { line } => {
                if let Some(&m) = self.marks.get(&c) {
                    let m = m.min(ctx.doc.len_chars());
                    let target = if line { first_nonblank(ctx.doc, ctx.doc.char_to_line(m)) } else { m };
                    self.apply_motion(ctx, pos, target, if line { Kind::Line } else { Kind::Exclusive }, out);
                }
            }
            Pending::Z => {
                match c {
                    'z' => out.actions.push(Action::Center),
                    't' => out.actions.push(Action::ScrollTop),
                    'b' => out.actions.push(Action::ScrollBottom),
                    'c' => out.actions.push(Action::Run("fold.fold")),
                    'o' => out.actions.push(Action::Run("fold.unfold")),
                    'a' => out.actions.push(Action::Run("fold.toggle")),
                    'M' => out.actions.push(Action::Run("fold.all")),
                    'R' => out.actions.push(Action::Run("fold.none")),
                    _ => {}
                }
            }
            Pending::Window => match c {
                'v' | 's' => out.actions.push(Action::SplitRight),
                'w' | 'l' => out.actions.push(Action::NextGroup),
                'h' | 'W' => out.actions.push(Action::PrevGroup),
                'c' | 'q' => out.actions.push(Action::CloseTab { force: false }),
                _ => {}
            },
            Pending::G => self.g_key(ctx, c, pos, out),
            Pending::TextObject { around } => {
                let Some((a, b, linewise)) = text_object(ctx.doc, pos, c, around) else {
                    self.reset();
                    return;
                };
                if let Some((op, _)) = self.op.take() {
                    self.apply_op(ctx, op, a, b, linewise, out);
                } else if self.mode.is_visual() {
                    self.anchor = a;
                    self.mode = Mode::Visual;
                    self.set_caret(ctx, b.saturating_sub(1).max(a));
                }
            }
        }
    }

    fn g_key(&mut self, ctx: &mut Ctx<'_>, c: char, pos: usize, out: &mut Outcome) {
        match c {
            'g' => {
                let l = self.count.take().map_or(0, |n| n - 1).min(ctx.doc.len_lines() - 1);
                let t = first_nonblank(ctx.doc, l);
                self.apply_motion(ctx, pos, t, Kind::Line, out);
            }
            'e' | 'E' => {
                let n = self.take_count();
                let t = word_end_back(ctx.doc, pos, n, c == 'E');
                self.apply_motion(ctx, pos, t, Kind::Inclusive, out);
            }
            '_' => {
                let line = ctx.doc.char_to_line(pos);
                let slice = ctx.doc.line_slice(line);
                let mut k = slice.len_chars();
                while k > 0 && slice.char(k - 1).is_whitespace() {
                    k -= 1;
                }
                let t = ctx.doc.line_start(line) + k.saturating_sub(1);
                self.apply_motion(ctx, pos, t, Kind::Inclusive, out);
            }
            'j' => self.visual_row_move(ctx, 1),
            'k' => self.visual_row_move(ctx, -1),
            'U' | 'u' | '~' | 'c' if self.mode.is_visual() => {
                let op = match c {
                    'U' => Op::Upper,
                    'u' => Op::Lower,
                    '~' => Op::SwapCase,
                    _ => Op::Comment,
                };
                self.visual_op(ctx, op, out);
            }
            'U' => self.pend_op(ctx, Op::Upper, 'U'),
            'u' => self.pend_op(ctx, Op::Lower, 'u'),
            '~' => self.pend_op(ctx, Op::SwapCase, '~'),
            'c' => self.pend_op(ctx, Op::Comment, 'c'),
            'd' => out.actions.push(Action::Run("lsp.definition")),
            'D' => out.actions.push(Action::Run("lsp.declaration")),
            'i' => out.actions.push(Action::Run("lsp.implementation")),
            'r' => out.actions.push(Action::Run("lsp.references")),
            'h' => out.actions.push(Action::Run("lsp.hover")),
            't' => out.actions.push(Action::NextTab),
            'T' => out.actions.push(Action::PrevTab),
            'J' => {
                self.begin_change(&[Key::ch('g'), Key::ch('J')]);
                let line = ctx.doc.char_to_line(pos);
                if line + 1 < ctx.doc.len_lines() {
                    let end = ctx.doc.line_end(line);
                    ctx.doc.apply(vec![Edit::delete(end, end + 1)], ctx.cur, EditKind::Other, |_, cur| cur.set_single(Selection::cursor(end)));
                }
                self.end_change();
            }
            'v' => {
                if let Some((mode, a, h)) = self.last_visual {
                    self.mode = mode;
                    self.anchor = a.min(ctx.doc.len_chars());
                    self.set_caret(ctx, h);
                }
            }
            _ => self.reset(),
        }
    }

    /// `gj`: a screen row down, for wrapped lines.
    fn visual_row_move(&mut self, ctx: &mut Ctx<'_>, delta: isize) {
        let (row, v) = ctx.map.pos_to_row(ctx.doc, ctx.cur.head());
        let target = row as isize + delta;
        if target >= 0 && (target as usize) < ctx.map.row_count(ctx.doc) {
            let p = ctx.map.row_to_pos(ctx.doc, target as usize, v);
            self.set_caret(ctx, p);
        }
    }

    fn ctrl_key(&mut self, ctx: &mut Ctx<'_>, key: Key, out: &mut Outcome) {
        let Some(c) = key.ch else {
            out.handled = false;
            return;
        };
        let page = ctx.page_rows.max(2);
        let n = self.count.unwrap_or(1);
        match c {
            'r' => {
                let n = self.take_count();
                for _ in 0..n {
                    ctx.doc.redo(ctx.cur);
                }
                self.clamp_caret(ctx);
            }
            'd' | 'u' | 'f' | 'b' => {
                let rows = match c {
                    'd' | 'u' => page / 2,
                    _ => page,
                } * n;
                self.count = None;
                let line = ctx.doc.char_to_line(self.caret(ctx));
                let down = matches!(c, 'd' | 'f');
                let target = if down { (line + rows).min(last_text_line(ctx.doc)) } else { line.saturating_sub(rows) };
                let p = first_nonblank(ctx.doc, target);
                self.set_caret(ctx, p);
            }
            'w' => self.pending = Pending::Window,
            'v' => self.enter_visual(ctx, Mode::VisualBlock),
            'a' | 'x' => {
                let delta = if c == 'a' { 1 } else { -1 } * self.take_count() as i64;
                self.begin_change(std::slice::from_ref(&key));
                add_to_number(ctx, delta);
                self.end_change();
            }
            'o' => out.actions.push(Action::Run("nav.back")),
            'i' => out.actions.push(Action::Run("nav.forward")),
            // Everything else is the editor's: Ctrl+P, Ctrl+S and the rest keep working.
            _ => out.handled = false,
        }
    }

    fn enter_visual(&mut self, ctx: &mut Ctx<'_>, mode: Mode) {
        let pos = self.caret(ctx);
        if self.mode == mode {
            self.leave_visual(ctx);
            let p = clamp_normal(ctx.doc, pos);
            ctx.cur.set_single(Selection::cursor(p));
            return;
        }
        if !self.mode.is_visual() {
            self.anchor = pos;
        }
        self.mode = mode;
        if mode == Mode::VisualBlock {
            let (row, v) = ctx.map.pos_to_row(ctx.doc, pos);
            self.block = ((row, v), (row, v));
        }
        self.block.1 .0 = if mode == Mode::VisualLine { pos } else { self.block.1.0 };
        self.set_caret(ctx, pos);
    }

    fn leave_visual(&mut self, ctx: &mut Ctx<'_>) {
        let pos = self.caret(ctx);
        self.last_visual = Some((self.mode, self.anchor, pos));
        let (s, e) = {
            let p = ctx.cur.primary();
            (p.start(), p.end())
        };
        self.marks.insert('<', s);
        self.marks.insert('>', e.saturating_sub(1));
        self.mode = Mode::Normal;
        let p = clamp_normal(ctx.doc, pos);
        ctx.cur.set_single(Selection::cursor(p));
    }

    fn pend_op(&mut self, ctx: &mut Ctx<'_>, op: Op, key: char) {
        // Doubling an operator acts on whole lines: dd, yy, >>, gUU.
        if let Some((pending, _)) = self.op
            && pending == op
        {
            self.op = None;
            let n = self.take_count();
            let pos = ctx.cur.head();
            let line = ctx.doc.char_to_line(pos);
            let last = (line + n - 1).min(ctx.doc.len_lines() - 1);
            let (a, b) = line_span(ctx.doc, ctx.doc.line_start(line), ctx.doc.line_start(last));
            let mut out = Outcome::default();
            self.apply_op(ctx, op, a, b, true, &mut out);
            return;
        }
        if op != Op::Yank && !self.in_change {
            let mut keys = Vec::new();
            if let Some(n) = self.count {
                keys.extend(n.to_string().chars().map(Key::ch));
            }
            if matches!(op, Op::Upper | Op::Lower | Op::SwapCase | Op::Comment) {
                keys.push(Key::ch('g'));
            }
            keys.push(Key::ch(key));
            self.begin_change(&keys);
        }
        self.op = Some((op, self.count.unwrap_or(1)));
    }

    fn normal_command(&mut self, ctx: &mut Ctx<'_>, c: char, out: &mut Outcome) {
        let n = self.take_count();
        let pos = ctx.cur.head();
        let line = ctx.doc.char_to_line(pos);
        // An operator waiting, and this key isn't a motion for it.
        if let Some((op, _)) = self.op {
            let doubled = matches!((op, c), (Op::Delete, 'd') | (Op::Change, 'c') | (Op::Yank, 'y') | (Op::Indent, '>') | (Op::Outdent, '<') | (Op::Upper, 'U') | (Op::Lower, 'u') | (Op::SwapCase, '~') | (Op::Comment, 'c'));
            if doubled {
                self.count = Some(n);
                self.pend_op(ctx, op, c);
                let _ = out;
            } else {
                self.reset();
                self.in_change = false;
            }
            return;
        }
        let keys_with_count = |k: char| {
            let mut v = Vec::new();
            if n > 1 {
                v.extend(n.to_string().chars().map(Key::ch));
            }
            v.push(Key::ch(k));
            v
        };
        match c {
            'd' => {
                self.count = Some(n).filter(|x| *x > 1);
                self.pend_op(ctx, Op::Delete, 'd');
            }
            'c' => {
                self.count = Some(n).filter(|x| *x > 1);
                self.pend_op(ctx, Op::Change, 'c');
            }
            'y' => {
                self.count = Some(n).filter(|x| *x > 1);
                self.pend_op(ctx, Op::Yank, 'y');
            }
            '>' => {
                self.count = Some(n).filter(|x| *x > 1);
                self.pend_op(ctx, Op::Indent, '>');
            }
            '<' => {
                self.count = Some(n).filter(|x| *x > 1);
                self.pend_op(ctx, Op::Outdent, '<');
            }
            'v' => self.enter_visual(ctx, Mode::Visual),
            'V' => self.enter_visual(ctx, Mode::VisualLine),
            'i' => {
                self.begin_change(&keys_with_count('i'));
                self.enter_insert(ctx, n);
            }
            'I' => {
                self.begin_change(&keys_with_count('I'));
                ctx.cur.set_single(Selection::cursor(first_nonblank(ctx.doc, line)));
                self.enter_insert(ctx, n);
            }
            'a' => {
                self.begin_change(&keys_with_count('a'));
                let p = if pos < ctx.doc.line_end(line) { pos + 1 } else { pos };
                ctx.cur.set_single(Selection::cursor(p));
                self.enter_insert(ctx, n);
            }
            'A' => {
                self.begin_change(&keys_with_count('A'));
                ctx.cur.set_single(Selection::cursor(ctx.doc.line_end(line)));
                self.enter_insert(ctx, n);
            }
            'o' | 'O' => {
                self.begin_change(&[Key::ch(c)]);
                self.enter_insert(ctx, 1);
                ctx.cur.set_single(Selection::cursor(pos));
                if c == 'o' {
                    commands::insert_line_below(ctx.doc, ctx.cur);
                } else {
                    commands::insert_line_above(ctx.doc, ctx.cur);
                }
            }
            'R' => {
                self.begin_change(&[Key::ch('R')]);
                self.enter_insert(ctx, 1);
                self.mode = Mode::Replace;
            }
            'x' | 'X' | 's' => {
                self.begin_change(&keys_with_count(c));
                let (a, b) = if c == 'X' {
                    let start = ctx.doc.line_start(line);
                    (pos.saturating_sub(n).max(start), pos)
                } else {
                    (pos, (pos + n).min(ctx.doc.line_end(line)))
                };
                let op = if c == 's' { Op::Change } else { Op::Delete };
                self.apply_op(ctx, op, a, b, false, out);
            }
            'D' | 'C' => {
                self.begin_change(&keys_with_count(c));
                let last = (line + n - 1).min(ctx.doc.len_lines() - 1);
                let op = if c == 'C' { Op::Change } else { Op::Delete };
                self.apply_op(ctx, op, pos, ctx.doc.line_end(last), false, out);
            }
            'S' => {
                self.begin_change(&keys_with_count('S'));
                let last = (line + n - 1).min(ctx.doc.len_lines() - 1);
                let (a, b) = line_span(ctx.doc, ctx.doc.line_start(line), ctx.doc.line_start(last));
                self.apply_op(ctx, Op::Change, a, b, true, out);
            }
            'Y' => {
                let last = (line + n - 1).min(ctx.doc.len_lines() - 1);
                let (a, b) = line_span(ctx.doc, ctx.doc.line_start(line), ctx.doc.line_start(last));
                self.apply_op(ctx, Op::Yank, a, b, true, out);
            }
            '~' => {
                self.begin_change(&keys_with_count('~'));
                let end = (pos + n).min(ctx.doc.line_end(line));
                let text: String = ctx.doc.slice_string(pos, end).chars().map(swap_case).collect();
                ctx.doc.apply(vec![Edit::new(pos, end, text)], ctx.cur, EditKind::Other, |_, _| {});
                let p = clamp_normal(ctx.doc, end);
                ctx.cur.set_single(Selection::cursor(p));
                self.end_change();
            }
            'J' => {
                self.begin_change(&keys_with_count('J'));
                let mut joint = pos;
                for _ in 0..n.max(2) - 1 {
                    if line + 1 >= ctx.doc.len_lines() || (line + 2 == ctx.doc.len_lines() && ctx.doc.line_len(line + 1) == 0) {
                        break;
                    }
                    joint = ctx.doc.line_end(line);
                    let mut c2 = Cursors::single(Selection::cursor(pos));
                    commands::join_lines(ctx.doc, &mut c2);
                }
                ctx.cur.set_single(Selection::cursor(joint));
                self.clamp_caret(ctx);
                self.end_change();
            }
            'p' | 'P' => {
                self.begin_change(&keys_with_count(c));
                self.paste(ctx, c == 'P', n, out);
                self.end_change();
            }
            'u' => {
                for _ in 0..n {
                    ctx.doc.undo(ctx.cur);
                }
                self.clamp_caret(ctx);
            }
            '.' => {
                let keys = self.last_change.clone();
                let was = self.replaying;
                self.replaying = true;
                for _ in 0..n {
                    for k in &keys {
                        let o = self.key(ctx, k.clone());
                        out.actions.extend(o.actions);
                    }
                }
                self.replaying = was;
            }
            _ => {
                out.handled = false;
            }
        }
    }

    fn visual_command(&mut self, ctx: &mut Ctx<'_>, c: char, out: &mut Outcome) {
        match c {
            'v' => self.enter_visual(ctx, Mode::Visual),
            'V' => self.enter_visual(ctx, Mode::VisualLine),
            'o' => {
                let pos = self.caret(ctx);
                let other = self.anchor;
                self.anchor = pos;
                self.set_caret(ctx, other);
            }
            'd' | 'x' | 'X' | 'D' => self.visual_op(ctx, Op::Delete, out),
            'c' | 's' | 'S' | 'C' => self.visual_op(ctx, Op::Change, out),
            'y' | 'Y' => self.visual_op(ctx, Op::Yank, out),
            '>' => self.visual_op(ctx, Op::Indent, out),
            '<' => self.visual_op(ctx, Op::Outdent, out),
            'U' => self.visual_op(ctx, Op::Upper, out),
            'u' => self.visual_op(ctx, Op::Lower, out),
            '~' => self.visual_op(ctx, Op::SwapCase, out),
            'I' | 'A' if self.mode == Mode::VisualBlock => {
                // Every row of the block gets a cursor: typing goes to all of them.
                let sels: Vec<Selection> = ctx
                    .cur
                    .iter()
                    .map(|s| Selection::cursor(if c == 'I' { s.start() } else { s.end() }))
                    .collect();
                self.mode = Mode::Insert;
                self.insert_repeat = 0;
                ctx.doc.begin_group();
                *ctx.cur = Cursors::from_vec(sels, 0);
            }
            'p' | 'P' => {
                let s = ctx.cur.primary();
                let reg = self.read_reg().cloned();
                let (a, b) = (s.start(), s.end());
                self.leave_visual(ctx);
                if let Some(r) = reg {
                    ctx.doc.apply(vec![Edit::new(a, b, r.text)], ctx.cur, EditKind::Other, |m, cur| {
                        cur.set_single(Selection::cursor(m.after_edit(0).saturating_sub(1)));
                    });
                }
            }
            'J' => {
                commands::join_lines(ctx.doc, ctx.cur);
                self.leave_visual(ctx);
            }
            _ => out.handled = false,
        }
    }

    fn visual_op(&mut self, ctx: &mut Ctx<'_>, op: Op, out: &mut Outcome) {
        let linewise = self.mode == Mode::VisualLine;
        let spans: Vec<(usize, usize)> = ctx.cur.iter().map(|s| (s.start(), s.end())).collect();
        self.last_visual = Some((self.mode, self.anchor, self.caret(ctx)));
        let block = self.mode == Mode::VisualBlock;
        self.mode = Mode::Normal;
        if block {
            self.block_op(ctx, op, spans, out);
            return;
        }
        let (a, b) = spans[0];
        ctx.cur.set_single(Selection::cursor(a));
        self.apply_op(ctx, op, a, b, linewise, out);
    }

    fn block_op(&mut self, ctx: &mut Ctx<'_>, op: Op, spans: Vec<(usize, usize)>, out: &mut Outcome) {
        let text = spans.iter().map(|(a, b)| ctx.doc.slice_string(*a, *b)).collect::<Vec<_>>().join("\n");
        if matches!(op, Op::Delete | Op::Change | Op::Yank) {
            self.write_reg(Reg { text, linewise: false }, op == Op::Yank, out);
        }
        match op {
            Op::Yank => {
                ctx.cur.set_single(Selection::cursor(spans[0].0));
            }
            Op::Delete | Op::Change => {
                let edits = spans.iter().map(|(a, b)| Edit::delete(*a, *b)).collect();
                let starts: Vec<usize> = spans.iter().map(|s| s.0).collect();
                ctx.doc.apply(edits, ctx.cur, EditKind::Other, |m, c| {
                    let sels = starts.iter().map(|s| Selection::cursor(m.map(*s, Assoc::Before))).collect();
                    *c = Cursors::from_vec(sels, 0);
                });
                if op == Op::Change {
                    self.mode = Mode::Insert;
                    self.insert_repeat = 0;
                    ctx.doc.begin_group();
                } else {
                    ctx.cur.keep_primary();
                    self.clamp_caret(ctx);
                }
            }
            _ => {}
        }
    }

    /// Runs an operator on a range.
    fn apply_op(&mut self, ctx: &mut Ctx<'_>, op: Op, a: usize, b: usize, linewise: bool, out: &mut Outcome) {
        let a = a.min(ctx.doc.len_chars());
        let b = b.min(ctx.doc.len_chars());
        match op {
            Op::Yank => {
                let text = ctx.doc.slice_string(a, b);
                self.write_reg(Reg { text: ensure_nl(text, linewise), linewise }, true, out);
                self.marks.insert('[', a);
                // `yj` stays put; `yk` and a visual yank go to where the text starts.
                let head = ctx.cur.head();
                let target = if linewise && head >= a && head < b { head } else { a };
                ctx.cur.set_single(Selection::cursor(clamp_normal(ctx.doc, target)));
            }
            Op::Delete | Op::Change => {
                let text = ctx.doc.slice_string(a, b);
                self.write_reg(Reg { text: ensure_nl(text, linewise), linewise }, false, out);
                let change = op == Op::Change;
                if change {
                    ctx.doc.begin_group();
                }
                if linewise && change {
                    // cc keeps the line, and its indentation.
                    let line = ctx.doc.char_to_line(a);
                    let indent = ctx.doc.line_indent(line);
                    let end = if b > a && ctx.doc.char_at(b - 1) == Some('\n') { b - 1 } else { b };
                    let n = indent.chars().count();
                    ctx.doc.apply(vec![Edit::new(a, end, indent)], ctx.cur, EditKind::Other, |_, c| c.set_single(Selection::cursor(a + n)));
                } else if linewise {
                    // The last line of the file has no newline of its own to take.
                    let (da, db) = if b == ctx.doc.len_chars() && a > 0 && ctx.doc.char_at(b.saturating_sub(1)) != Some('\n') { (a - 1, b) } else { (a, b) };
                    ctx.doc.apply(vec![Edit::delete(da, db)], ctx.cur, EditKind::Other, |_, c| c.set_single(Selection::cursor(da)));
                    let l = ctx.doc.char_to_line(da.min(ctx.doc.len_chars()));
                    let l = if da != a { l } else { l.min(last_text_line(ctx.doc)) };
                    let p = first_nonblank(ctx.doc, l);
                    ctx.cur.set_single(Selection::cursor(p));
                } else {
                    ctx.doc.apply(vec![Edit::delete(a, b)], ctx.cur, EditKind::Other, |_, c| c.set_single(Selection::cursor(a)));
                }
                if change {
                    self.mode = Mode::Insert;
                    self.insert_repeat = 0;
                    self.insert_keys.clear();
                } else {
                    self.clamp_caret(ctx);
                    self.end_change();
                }
            }
            Op::Indent | Op::Outdent => {
                let mut c = Cursors::single(Selection::new(a, b));
                if op == Op::Indent {
                    commands::indent_lines(ctx.doc, &mut c);
                } else {
                    commands::outdent_lines(ctx.doc, &mut c);
                }
                let l = ctx.doc.char_to_line(a);
                ctx.cur.set_single(Selection::cursor(first_nonblank(ctx.doc, l)));
                self.end_change();
            }
            Op::Upper | Op::Lower | Op::SwapCase => {
                let text = ctx.doc.slice_string(a, b);
                let out_text: String = match op {
                    Op::Upper => text.to_uppercase(),
                    Op::Lower => text.to_lowercase(),
                    _ => text.chars().map(swap_case).collect(),
                };
                ctx.doc.apply(vec![Edit::new(a, b, out_text)], ctx.cur, EditKind::Other, |_, c| c.set_single(Selection::cursor(a)));
                self.clamp_caret(ctx);
                self.end_change();
            }
            Op::Comment => {
                let mut c = Cursors::single(Selection::new(a, b));
                commands::toggle_line_comment(ctx.doc, &mut c);
                let l = ctx.doc.char_to_line(a);
                ctx.cur.set_single(Selection::cursor(first_nonblank(ctx.doc, l)));
                self.end_change();
            }
        }
    }

    fn read_reg(&self) -> Option<&Reg> {
        self.regs.get(&self.register.unwrap_or('"'))
    }

    fn write_reg(&mut self, r: Reg, yank: bool, out: &mut Outcome) {
        let name = self.register.take();
        if matches!(name, Some('+' | '*')) {
            out.actions.push(Action::SetClipboard(r.text.clone()));
        }
        if name != Some('_') {
            if let Some(n) = name {
                self.regs.insert(n, r.clone());
            }
            if yank {
                self.regs.insert('0', r.clone());
            }
            self.regs.insert('"', r);
        }
    }

    fn paste(&mut self, ctx: &mut Ctx<'_>, before: bool, n: usize, out: &mut Outcome) {
        let reg_name = self.register.take();
        if matches!(reg_name, Some('+' | '*')) {
            out.actions.push(Action::PasteClipboard { before });
            return;
        }
        let Some(r) = self.regs.get(&reg_name.unwrap_or('"')).cloned() else { return };
        let text = r.text.repeat(n);
        let pos = ctx.cur.head();
        let line = ctx.doc.char_to_line(pos);
        if r.linewise {
            let (at, text2, first) = if before {
                (ctx.doc.line_start(line), text.clone(), line)
            } else if line + 1 < ctx.doc.len_lines() {
                (ctx.doc.line_start(line + 1), text.clone(), line + 1)
            } else {
                // After a last line with no newline, the newline goes first.
                (ctx.doc.len_chars(), format!("\n{}", text.trim_end_matches('\n')), line + 1)
            };
            ctx.doc.apply(vec![Edit::insert(at, text2)], ctx.cur, EditKind::Other, |_, _| {});
            ctx.cur.set_single(Selection::cursor(first_nonblank(ctx.doc, first)));
        } else {
            let at = if before || ctx.doc.line_len(line) == 0 { pos } else { (pos + 1).min(ctx.doc.line_end(line)) };
            let len = text.chars().count();
            ctx.doc.apply(vec![Edit::insert(at, text)], ctx.cur, EditKind::Other, |_, _| {});
            ctx.cur.set_single(Selection::cursor(at + len - 1));
        }
    }

    /// Text from `"+p`, once the application has it.
    pub fn paste_text(&mut self, ctx: &mut Ctx<'_>, text: String, before: bool) {
        let linewise = text.ends_with('\n');
        self.regs.insert('+', Reg { text, linewise });
        self.register = Some('+');
        let r = self.regs.get(&'+').cloned();
        self.register = None;
        if let Some(r) = r {
            self.regs.insert('"', r);
        }
        let mut out = Outcome::default();
        self.paste(ctx, before, 1, &mut out);
    }

    // The command line -----------------------------------------------------------------------

    /// Runs what was typed on the `:`, `/` or `?` line.
    pub fn command_line(&mut self, ctx: &mut Ctx<'_>, prefix: char, text: &str) -> Vec<Action> {
        match prefix {
            '/' | '?' => {
                let forward = prefix == '/';
                if text.is_empty() {
                    return Vec::new();
                }
                let q = smart_case(text);
                self.last_search = Some((q.clone(), forward));
                let pos = ctx.cur.head();
                match search_from(ctx.doc, &q, pos, forward, 1) {
                    Some(t) => {
                        self.set_caret(ctx, t);
                        vec![Action::Highlight(Some(q))]
                    }
                    None => vec![Action::Message(format!("E486: {text}"))],
                }
            }
            _ => self.ex_command(ctx, text),
        }
    }

    fn ex_command(&mut self, ctx: &mut Ctx<'_>, raw: &str) -> Vec<Action> {
        let cmd = raw.trim();
        if cmd.is_empty() {
            return Vec::new();
        }
        if let Ok(n) = cmd.parse::<usize>() {
            let l = n.saturating_sub(1).min(ctx.doc.len_lines() - 1);
            let p = first_nonblank(ctx.doc, l);
            ctx.cur.set_single(Selection::cursor(p));
            return Vec::new();
        }
        // A range, then the command.
        let (range, rest) = parse_range(ctx, cmd, self);
        let rest = rest.trim_start();
        let (word, arg) = match rest.find(|c: char| !c.is_alphanumeric() && c != '!') {
            Some(i) => (&rest[..i], rest[i..].trim()),
            None => (rest, ""),
        };
        let raw_after_word = rest.get(word.len()..).unwrap_or("");
        match word {
            "w" | "write" => vec![Action::Save],
            "wa" | "wall" => vec![Action::SaveAll],
            "q" | "quit" => vec![Action::Quit { force: false }],
            "q!" | "quit!" => vec![Action::Quit { force: true }],
            "wq" | "x" | "xit" => vec![Action::SaveQuit],
            "qa" | "qall" => vec![Action::QuitAll { force: false }],
            "qa!" | "qall!" => vec![Action::QuitAll { force: true }],
            "wqa" | "xa" => vec![Action::SaveAll, Action::QuitAll { force: false }],
            "e" | "edit" | "o" | "open" if !arg.is_empty() => vec![Action::OpenFile(arg.to_string())],
            "tabnew" | "new" | "enew" => vec![if arg.is_empty() { Action::NewTab } else { Action::OpenFile(arg.to_string()) }],
            "bd" | "bdelete" | "tabclose" => vec![Action::CloseTab { force: false }],
            "bd!" | "bdelete!" => vec![Action::CloseTab { force: true }],
            "sp" | "split" | "vs" | "vsplit" => vec![Action::SplitRight],
            "tabn" | "tabnext" | "bn" | "bnext" => vec![Action::NextTab],
            "tabp" | "tabprev" | "bp" | "bprev" => vec![Action::PrevTab],
            "noh" | "nohlsearch" => vec![Action::Highlight(None)],
            "set" => vec![Action::SetOption(arg.to_string())],
            "d" | "delete" => {
                let (l0, l1) = range.unwrap_or_else(|| {
                    let l = ctx.doc.char_to_line(ctx.cur.head());
                    (l, l)
                });
                let (a, b) = line_span(ctx.doc, ctx.doc.line_start(l0), ctx.doc.line_start(l1));
                let mut out = Outcome::default();
                self.apply_op(ctx, Op::Delete, a, b, true, &mut out);
                out.actions
            }
            "sort" => {
                let (l0, l1) = range.unwrap_or((0, last_text_line(ctx.doc)));
                let mut lines: Vec<String> = (l0..=l1).map(|l| ctx.doc.line_slice(l).to_string()).collect();
                lines.sort();
                let (a, b) = (ctx.doc.line_start(l0), ctx.doc.line_end(l1));
                ctx.doc.apply(vec![Edit::new(a, b, lines.join("\n"))], ctx.cur, EditKind::Other, |_, _| {});
                self.clamp_caret(ctx);
                Vec::new()
            }
            "s" | "substitute" => {
                let (l0, l1) = range.unwrap_or_else(|| {
                    let l = ctx.doc.char_to_line(ctx.cur.head());
                    (l, l)
                });
                self.substitute(ctx, l0, l1, raw_after_word)
            }
            _ => vec![Action::Message(format!("E492: {cmd}"))],
        }
    }

    fn substitute(&mut self, ctx: &mut Ctx<'_>, l0: usize, l1: usize, spec: &str) -> Vec<Action> {
        let mut chars = spec.chars();
        let Some(sep) = chars.next() else { return Vec::new() };
        let mut parts = vec![String::new()];
        let mut esc = false;
        for c in chars {
            if esc {
                let last = parts.last_mut().unwrap();
                if c != sep {
                    last.push('\\');
                }
                last.push(c);
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == sep {
                parts.push(String::new());
            } else {
                parts.last_mut().unwrap().push(c);
            }
        }
        let pattern = parts.first().cloned().unwrap_or_default();
        let replacement = vim_replacement(parts.get(1).map_or("", String::as_str));
        let flags = parts.get(2).cloned().unwrap_or_default();
        let insensitive = flags.contains('i') || (!flags.contains('I') && pattern.chars().all(|c| !c.is_uppercase()) && pattern.starts_with("\\c"));
        let pattern = pattern.trim_start_matches("\\c");
        let re = match RegexBuilder::new(&vim_pattern(pattern)).case_insensitive(insensitive).build() {
            Ok(r) => r,
            Err(e) => return vec![Action::Message(e.to_string())],
        };
        let global = flags.contains('g');
        let (a, b) = (ctx.doc.line_start(l0), ctx.doc.line_end(l1));
        let mut edits = Vec::new();
        let mut count = 0;
        for l in l0..=l1 {
            let text = ctx.doc.line_slice(l).to_string();
            let line_start = ctx.doc.line_start(l);
            for caps in re.captures_iter(&text) {
                let m = caps.get(0).unwrap();
                let start = line_start + text[..m.start()].chars().count();
                let end = line_start + text[..m.end()].chars().count();
                let mut rep = String::new();
                caps.expand(&replacement, &mut rep);
                edits.push(Edit::new(start, end, rep));
                count += 1;
                if !global {
                    break;
                }
            }
        }
        let _ = (a, b);
        if edits.is_empty() {
            return vec![Action::Message(format!("E486: {pattern}"))];
        }
        ctx.doc.apply(edits, ctx.cur, EditKind::Other, |m, c| {
            c.map(|s| Selection::cursor(m.map(s.head, Assoc::Before)));
        });
        self.clamp_caret(ctx);
        vec![Action::Message(format!("{count} substitutions"))]
    }
}

fn ensure_nl(mut text: String, linewise: bool) -> String {
    if linewise && !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

fn swap_case(c: char) -> char {
    if c.is_uppercase() { c.to_lowercase().next().unwrap_or(c) } else { c.to_uppercase().next().unwrap_or(c) }
}

fn clamp_normal(doc: &Document, pos: usize) -> usize {
    let (line, col) = doc.line_col(pos);
    let len = doc.line_len(line);
    let p = doc.line_start(line) + col.min(len.saturating_sub(1));
    // The last empty "line" after a final newline can't hold a caret when there's text above.
    if line == doc.len_lines() - 1 && len == 0 && line > 0 {
        return first_nonblank(doc, line - 1).max(doc.line_start(line - 1)).min(doc.line_end(line - 1).saturating_sub(usize::from(doc.line_len(line - 1) > 0)));
    }
    p
}

fn step_left_in_line(doc: &Document, pos: usize) -> usize {
    let (_, col) = doc.line_col(pos);
    if col > 0 { pos - 1 } else { pos }
}

fn last_text_line(doc: &Document) -> usize {
    let last = doc.len_lines() - 1;
    if last > 0 && doc.line_len(last) == 0 { last - 1 } else { last }
}

fn first_nonblank(doc: &Document, line: usize) -> usize {
    let n = doc.line_slice(line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
    doc.line_start(line) + n.min(doc.line_len(line).saturating_sub(1).max(if doc.line_len(line) == 0 { 0 } else { n.min(doc.line_len(line) - 1) }))
}

/// Whole lines from the one holding `a` to the one holding `b`, with the last line's newline.
fn line_span(doc: &Document, a: usize, b: usize) -> (usize, usize) {
    let (la, lb) = (doc.char_to_line(a), doc.char_to_line(b));
    let end = if lb + 1 < doc.len_lines() { doc.line_start(lb + 1) } else { doc.len_chars() };
    (doc.line_start(la), end)
}

fn word_forward(doc: &Document, mut pos: usize, n: usize, big: bool) -> usize {
    let total = doc.len_chars();
    for _ in 0..n {
        let Some(c) = doc.char_at(pos) else { return total };
        let k = class(c, big);
        if k != 0 {
            while pos < total && class(doc.rope.char(pos), big) == k {
                pos += 1;
            }
        }
        while pos < total && class(doc.rope.char(pos), big) == 0 {
            pos += 1;
            // An empty line is a word.
            if pos < total && doc.rope.char(pos - 1) == '\n' && doc.rope.char(pos) == '\n' {
                break;
            }
        }
    }
    pos
}

fn word_back(doc: &Document, mut pos: usize, n: usize, big: bool) -> usize {
    for _ in 0..n {
        while pos > 0 && class(doc.rope.char(pos - 1), big) == 0 {
            pos -= 1;
        }
        if pos == 0 {
            return 0;
        }
        let k = class(doc.rope.char(pos - 1), big);
        while pos > 0 && class(doc.rope.char(pos - 1), big) == k {
            pos -= 1;
        }
    }
    pos
}

fn word_end(doc: &Document, mut pos: usize, n: usize, big: bool) -> usize {
    let total = doc.len_chars();
    for _ in 0..n {
        pos += 1;
        while pos < total && class(doc.rope.char(pos), big) == 0 {
            pos += 1;
        }
        if pos >= total {
            return total.saturating_sub(1);
        }
        let k = class(doc.rope.char(pos), big);
        while pos + 1 < total && class(doc.rope.char(pos + 1), big) == k {
            pos += 1;
        }
    }
    pos
}

fn word_end_back(doc: &Document, mut pos: usize, n: usize, big: bool) -> usize {
    for _ in 0..n {
        if pos == 0 {
            return 0;
        }
        let k = class(doc.rope.char(pos), big);
        while pos > 0 && k != 0 && class(doc.rope.char(pos), big) == k {
            pos -= 1;
        }
        while pos > 0 && class(doc.rope.char(pos), big) == 0 {
            pos -= 1;
        }
    }
    pos
}

fn find_in_line(doc: &Document, pos: usize, ch: char, forward: bool, till: bool, n: usize, repeat: bool) -> Option<usize> {
    let line = doc.char_to_line(pos);
    let (start, end) = (doc.line_start(line), doc.line_end(line));
    let mut p = pos;
    // `;` after `t` mustn't get stuck on the char it already stopped next to.
    if repeat && till {
        p = if forward { p + 1 } else { p.saturating_sub(1) };
    }
    for _ in 0..n {
        loop {
            if forward {
                p += 1;
                if p >= end {
                    return None;
                }
            } else {
                if p <= start {
                    return None;
                }
                p -= 1;
            }
            if doc.rope.char(p) == ch {
                break;
            }
        }
    }
    Some(if till { if forward { p - 1 } else { p + 1 } } else { p })
}

fn smart_case(q: &str) -> String {
    let q2 = vim_pattern(q);
    if q.chars().any(char::is_uppercase) { q2 } else { format!("(?i){q2}") }
}

/// Vim's magic mode: `\(` groups and `(` is literal, the other way around from the regex crate.
fn vim_pattern(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(n @ ('(' | ')' | '|' | '{' | '}' | '+' | '?')) => out.push(n),
                Some('<' | '>') => out.push_str("\\b"),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push_str("\\\\"),
            },
            '(' | ')' | '|' | '{' | '}' | '+' | '?' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn vim_replacement(r: &str) -> String {
    let mut out = String::with_capacity(r.len());
    let mut chars = r.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(d) if d.is_ascii_digit() => out.push_str(&format!("${{{d}}}")),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            },
            '&' => out.push_str("${0}"),
            '$' => out.push_str("$$"),
            _ => out.push(c),
        }
    }
    out
}

fn search_from(doc: &Document, q: &str, pos: usize, forward: bool, n: usize) -> Option<usize> {
    let re = RegexBuilder::new(q).multi_line(true).build().ok()?;
    let text = doc.rope.to_string();
    let hits: Vec<usize> = re.find_iter(&text).filter(|m| m.start() != m.end()).map(|m| doc.rope.byte_to_char(m.start())).collect();
    if hits.is_empty() {
        return None;
    }
    let len = hits.len();
    let i = if forward {
        let first = hits.partition_point(|h| *h <= pos);
        (first + n - 1) % len
    } else {
        let first = hits.partition_point(|h| *h < pos);
        (first + len * n - n % len) % len
    };
    // Backward from `first` means the one before it.
    Some(if forward { hits[i] } else { hits[(hits.partition_point(|h| *h < pos) + len - (n % len).max(1)) % len] })
}

fn parse_range<'a>(ctx: &Ctx<'_>, cmd: &'a str, vim: &Vim) -> (Option<(usize, usize)>, &'a str) {
    let last = last_text_line(ctx.doc);
    if let Some(rest) = cmd.strip_prefix('%') {
        return (Some((0, last)), rest);
    }
    let cur_line = ctx.doc.char_to_line(ctx.cur.head());
    let addr = |s: &str| -> Option<usize> {
        match s {
            "." => Some(cur_line),
            "$" => Some(last),
            "'<" => vim.marks.get(&'<').map(|p| ctx.doc.char_to_line(*p)),
            "'>" => vim.marks.get(&'>').map(|p| ctx.doc.char_to_line(*p)),
            n => n.parse::<usize>().ok().map(|x| x.saturating_sub(1).min(last)),
        }
    };
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^('<|'>|\.|\$|\d+),('<|'>|\.|\$|\d+)").unwrap());
    if let Some(c) = re.captures(cmd)
        && let (Some(a), Some(b)) = (addr(&c[1]), addr(&c[2]))
    {
        return (Some((a.min(b), a.max(b))), &cmd[c[0].len()..]);
    }
    (None, cmd)
}

fn add_to_number(ctx: &mut Ctx<'_>, delta: i64) {
    let pos = ctx.cur.head();
    let line = ctx.doc.char_to_line(pos);
    let start = ctx.doc.line_start(line);
    let text = ctx.doc.line_slice(line).to_string();
    let col = pos - start;
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"-?\d+").unwrap());
    for m in re.find_iter(&text) {
        let (a, b) = (text[..m.start()].chars().count(), text[..m.end()].chars().count());
        if b > col {
            let Ok(v) = m.as_str().parse::<i64>() else { return };
            let new = (v + delta).to_string();
            let n = new.chars().count();
            ctx.doc.apply(vec![Edit::new(start + a, start + b, new)], ctx.cur, EditKind::Other, |_, c| {
                c.set_single(Selection::cursor(start + a + n - 1));
            });
            return;
        }
    }
}

/// `iw`, `a(`, `i"` and the like: (start, end, whole lines).
fn text_object(doc: &Document, pos: usize, c: char, around: bool) -> Option<(usize, usize, bool)> {
    match c {
        'w' | 'W' => {
            let big = c == 'W';
            let ch = doc.char_at(pos)?;
            let k = class(ch, big);
            let total = doc.len_chars();
            let (mut a, mut b) = (pos, pos + 1);
            while a > 0 && class(doc.rope.char(a - 1), big) == k && doc.rope.char(a - 1) != '\n' {
                a -= 1;
            }
            while b < total && class(doc.rope.char(b), big) == k && doc.rope.char(b) != '\n' {
                b += 1;
            }
            if around {
                let before = b;
                while b < total && matches!(doc.rope.char(b), ' ' | '\t') {
                    b += 1;
                }
                if b == before {
                    while a > 0 && matches!(doc.rope.char(a - 1), ' ' | '\t') {
                        a -= 1;
                    }
                }
            }
            Some((a, b, false))
        }
        '"' | '\'' | '`' => {
            let line = doc.char_to_line(pos);
            let start = doc.line_start(line);
            let text: Vec<char> = doc.line_slice(line).chars().collect();
            let col = pos - start;
            let quotes: Vec<usize> = text.iter().enumerate().filter(|(i, x)| **x == c && (*i == 0 || text[*i - 1] != '\\')).map(|(i, _)| i).collect();
            // The pair around the caret, or the next one on the line.
            let pair = quotes.as_chunks::<2>().0.iter().find(|p| col <= p[1])?;
            let (qa, qb) = (pair[0], pair[1]);
            if around {
                let mut e = qb + 1;
                while e < text.len() && text[e] == ' ' {
                    e += 1;
                }
                Some((start + qa, start + e, false))
            } else {
                Some((start + qa + 1, start + qb, false))
            }
        }
        'p' => {
            let line = doc.char_to_line(pos);
            let blank = doc.line_is_blank(line);
            let same = |l: usize| doc.line_is_blank(l) == blank;
            let mut a = line;
            while a > 0 && same(a - 1) {
                a -= 1;
            }
            let last = last_text_line(doc);
            let mut b = line;
            while b < last && same(b + 1) {
                b += 1;
            }
            if around {
                while b < last && doc.line_is_blank(b + 1) {
                    b += 1;
                }
            }
            let (x, y) = line_span(doc, doc.line_start(a), doc.line_start(b));
            Some((x, y, true))
        }
        _ => {
            let (open, close) = opener(c)?;
            let (a, b) = enclosing_pair(doc, pos, open, close)?;
            if around {
                Some((a, b + 1, false))
            } else {
                // Block-like: `di{` keeps the braces on their own lines.
                let mut x = a + 1;
                let mut y = b;
                if doc.char_at(x) == Some('\n') {
                    x += 1;
                    let line = doc.char_to_line(y);
                    if y == first_nonblank_or_end(doc, line) && line > 0 {
                        y = doc.line_start(line);
                    }
                }
                Some((x, y.max(x), false))
            }
        }
    }
}

fn first_nonblank_or_end(doc: &Document, line: usize) -> usize {
    doc.line_start(line) + doc.line_slice(line).chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn enclosing_pair(doc: &Document, pos: usize, open: char, close: char) -> Option<(usize, usize)> {
    let total = doc.len_chars();
    // On a bracket counts as inside it.
    let mut depth = 0i32;
    let mut a = None;
    let mut p = if doc.char_at(pos) == Some(close) { pos } else { (pos + 1).min(total) };
    while p > 0 {
        p -= 1;
        let c = doc.rope.char(p);
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                a = Some(p);
                break;
            }
            depth -= 1;
        }
    }
    let a = a?;
    let mut depth = 0i32;
    for (i, c) in doc.rope.chars_at(a).enumerate() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some((a, a + i));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::Folds;

    struct T {
        doc: Document,
        cur: Cursors,
        vim: Vim,
        actions: Vec<Action>,
        first_row: usize,
    }

    impl T {
        /// `|` marks the caret.
        fn new(src: &str) -> Self {
            let at = src.chars().position(|c| c == '|').unwrap_or(0);
            let text: String = src.chars().filter(|c| *c != '|').collect();
            let mut doc = Document::new(&text);
            doc.indent = crate::buffer::Indent { spaces: true, width: 4 };
            Self { doc, cur: Cursors::single(Selection::cursor(at)), vim: Vim::new(), actions: Vec::new(), first_row: 0 }
        }

        fn press(&mut self, key: Key) {
            let map = DisplayMap::new(&self.doc, &Folds::default(), None, 4);
            let opts = EditOpts { auto_pairs: false, auto_indent: true, surround: false };
            let mut ctx = Ctx { doc: &mut self.doc, cur: &mut self.cur, map: &map, page_rows: 10, first_row: self.first_row, opts };
            let was_insert = self.vim.is_insert();
            let out = self.vim.key(&mut ctx, key.clone());
            self.actions.extend(out.actions);
            // What the editor does with a key Vim leaves to it, in insert mode.
            if !out.handled && was_insert {
                match key.name {
                    "enter" => commands::newline(&mut self.doc, &mut self.cur, opts),
                    "backspace" => commands::backspace(&mut self.doc, &mut self.cur, opts),
                    _ => {
                        if let Some(c) = key.ch {
                            commands::insert_plain(&mut self.doc, &mut self.cur, &c.to_string());
                        }
                    }
                }
            }
        }

        /// Keys in Vim notation: `<esc>`, `<cr>`, `<bs>`, `<c-r>`.
        fn keys(&mut self, s: &str) -> &mut Self {
            let chars: Vec<char> = s.chars().collect();
            let mut i = 0;
            while i < chars.len() {
                if chars[i] == '<'
                    && let Some(end) = chars[i..].iter().position(|c| *c == '>')
                    && end > 1
                {
                    let name: String = chars[i + 1..i + end].iter().collect();
                    let key = match name.as_str() {
                        "esc" => Key::named("escape"),
                        "cr" => Key::named("enter"),
                        "bs" => Key::named("backspace"),
                        "lt" => Key::ch('<'),
                        c if c.starts_with("c-") => Key::ctrl(c.chars().nth(2).unwrap()),
                        _ => panic!("{name}"),
                    };
                    self.press(key);
                    i += end + 1;
                    continue;
                }
                self.press(Key::ch(chars[i]));
                i += 1;
            }
            self
        }

        fn ex(&mut self, prefix: char, s: &str) -> Vec<Action> {
            let map = DisplayMap::new(&self.doc, &Folds::default(), None, 4);
            let mut ctx = Ctx { doc: &mut self.doc, cur: &mut self.cur, map: &map, page_rows: 10, first_row: 0, opts: EditOpts::default() };
            self.vim.command_line(&mut ctx, prefix, s)
        }

        fn text(&self) -> String {
            self.doc.rope.to_string()
        }

        fn show(&self) -> String {
            let mut s = self.text();
            let b = self.doc.rope.char_to_byte(self.cur.head());
            s.insert(b, '|');
            s
        }

        fn sel(&self) -> String {
            let p = self.cur.primary();
            self.doc.slice_string(p.start(), p.end())
        }
    }

    fn check(start: &str, keys: &str, want: &str) {
        let mut t = T::new(start);
        t.keys(keys);
        assert_eq!(t.show(), want, "{start:?} after {keys:?}");
    }

    /// Text cut short under a visual selection (another view, a reload) leaves its start past the
    /// end; moving on from there stays inside the text.
    #[test]
    fn visual_selection_survives_the_text_shrinking_under_it() {
        let mut t = T::new("abc\nde|f\n");
        t.keys("v");
        let len = t.doc.len_chars();
        let mut c = Cursors::single(Selection::cursor(0));
        t.doc.apply(vec![crate::Edit::delete(1, len)], &mut c, crate::EditKind::Other, |_, _| {});
        t.cur = Cursors::single(Selection::cursor(1));
        t.keys("h");
        let n = t.doc.len_chars();
        for s in t.cur.iter() {
            assert!(s.anchor <= n && s.head <= n, "{s:?} past {n}");
        }
    }

    #[test]
    fn motions() {
        check("|foo bar baz", "w", "foo |bar baz");
        check("|foo bar baz", "2w", "foo bar |baz");
        check("foo bar |baz", "b", "foo |bar baz");
        check("|foo bar", "e", "fo|o bar");
        check("|foo.bar baz", "W", "foo.bar |baz");
        check("|foo.bar baz", "w", "foo|.bar baz");
        check("  abc|d", "^", "  |abcd");
        check("  abc|d", "0", "|  abcd");
        check("|abcd\n", "$", "abc|d\n");
        check("|a\nb\nc\n", "G", "a\nb\n|c\n");
        check("a\nb\n|c\n", "gg", "|a\nb\nc\n");
        check("|a\nb\nc\n", "2G", "a\n|b\nc\n");
        check("|ab(c)d", "%", "ab(c|)d");
        check("|a,b,c", "fb", "a,|b,c");
        check("|a,b,c", "tc", "a,b|,c");
        check("a,b,|c", "Fa", "|a,b,c");
        check("|a,b,c", "f,;", "a,b|,c");
        check("|a,b,c", "f,;,", "a|,b,c");
        check("|a\n\nb\n\nc", "}", "a\n|\nb\n\nc");
        check("|a\n\nb\n\nc", "}}", "a\n\nb\n|\nc");
        check("|a\nb", "l", "|a\nb");
        check("|abc", "$h", "a|bc");
    }

    #[test]
    fn j_and_k_keep_the_column() {
        check("abc|d\nx\nabcd\n", "j", "abcd\n|x\nabcd\n");
        check("abc|d\nx\nabcd\n", "jj", "abcd\nx\nabc|d\n");
        check("a\n|b\n", "k", "|a\nb\n");
        // Past the end does nothing, and the last empty "line" isn't a place to be.
        check("a\n|b\n", "j", "a\n|b\n");
    }

    #[test]
    fn delete_change_yank_with_motions() {
        check("|foo bar", "dw", "|bar");
        check("|foo bar", "de", "| bar");
        check("foo |bar", "db", "|bar");
        check("|foo bar baz", "d2w", "|baz");
        check("|foo bar baz", "2dw", "|baz");
        check("|abcd", "dl", "|bcd");
        check("ab|cd", "d$", "a|b");
        check("ab|cd", "D", "a|b");
        check("ab|cd", "d0", "|cd");
        check("|a,b,c", "dt,", "|,b,c");
        check("|a,b,c", "df,", "|b,c");
        check("|foo bar", "cwxy<esc>", "x|y bar");
        check("|foo bar", "cexy<esc>", "x|y bar");
        check("ab|cd", "Cxy<esc>", "abx|y");
        check("|abc", "xx", "|c");
        check("ab|c", "X", "a|c");
        check("|abc", "sz<esc>", "|zbc");
        check("|abc", "rz", "|zbc");
        check("|abc", "2rz", "z|zc");
        check("|aBc", "~", "A|Bc");
        check("|abc", "~~", "AB|c");
        check("|abc def", "gUw", "|ABC def");
        check("|ABC", "guu", "|abc");
        check("|abc", "g~~", "|ABC");
    }

    #[test]
    fn whole_lines() {
        check("a\n|b\nc\n", "dd", "a\n|c\n");
        check("a\nb\n|c\n", "dd", "a\n|b\n");
        check("|a\nb\nc\n", "2dd", "|c\n");
        check("|a\nb\nc\n", "dj", "|c\n");
        check("a\n|b\nc\n", "dk", "|c\n");
        check("a\n  |b\nc\n", "cc", "a\n  |\nc\n");
        check("  |ab\nc\n", "ccx<esc>", "  |x\nc\n");
        check("|a\nb\n", "Sxy<esc>", "x|y\nb\n");
        check("|a\nb\n", ">>", "    |a\nb\n");
        check("    |a\n", "<lt><lt>", "|a\n");
        check("|a\nb\nc\n", "J", "a| b\nc\n");
    }

    #[test]
    fn yank_and_put() {
        check("|foo bar", "yiwP", "fo|ofoo bar");
        check("|foo bar", "yiwp", "ffo|ooo bar");
        check("|a\nb\n", "yyp", "a\n|a\nb\n");
        check("|a\nb\n", "yyP", "|a\na\nb\n");
        check("a\n|b", "yyp", "a\nb\n|b");
        check("|a\nb\n", "ddp", "b\n|a\n");
        check("|abc", "x2p", "ba|ac");
        // A named register keeps its text through other deletes.
        check("|one two", "\"ayiwwdiw\"aP", "oneon|e ");
        // Deleting doesn't touch register 0.
        check("|a b", "yiwwx\"0P", "a|a ");
        // The black hole register leaves the unnamed one alone.
        check("|a b", "yiww\"_xP", "a|a ");
    }

    #[test]
    fn text_objects() {
        check("foo b|ar baz", "diw", "foo | baz");
        check("foo b|ar baz", "daw", "foo |baz");
        check("f(a, |b)", "di(", "f(|)");
        check("f(a, |b)", "da(", "|f");
        check("f(a, |b)", "cibx<esc>", "f(|x)");
        check("let s = \"a |b\";", "di\"", "let s = \"|\";");
        check("let s = \"a |b\";", "da\"", "let s = |;");
        check("x{a[|1]b}", "di[", "x{a[|]b}");
        check("x{a[|1]b}", "di{", "x{|}");
        check("fn f() {\n    |a;\n    b;\n}\n", "di{", "fn f() {\n|}\n");
        check("<|a>", "di<", "<|>");
        check("|a\nb\n\nc\n", "dip", "|\nc\n");
        check("|a\nb\n\nc\n", "dap", "|c\n");
        // A quote pair further along the line counts.
        check("|x = \"oi\"", "ci\"z<esc>", "x = \"|z\"");
    }

    #[test]
    fn insert_commands_and_counts() {
        check("|b", "iaa<esc>", "a|ab");
        check("|b", "ax<esc>", "b|x");
        check("  |b c", "Ix<esc>", "  |xb c");
        check("|b c", "Ax<esc>", "b c|x");
        check("|a\nb", "ox<esc>", "a\n|x\nb");
        check("|a\nb", "Ox<esc>", "|x\na\nb");
        check("  |a", "ox<esc>", "  a\n  |x");
        check("|b", "3ix<esc>", "xx|xb");
        check("|ab", "Rxy<esc>", "x|y");
    }

    #[test]
    fn insert_is_one_undo_step() {
        let mut t = T::new("|");
        t.keys("ihello<cr>world<esc>");
        assert_eq!(t.text(), "hello\nworld");
        t.keys("u");
        assert_eq!(t.text(), "");
        t.keys("<c-r>");
        assert_eq!(t.text(), "hello\nworld");
        let mut t = T::new("|foo bar");
        t.keys("cwxyz<esc>");
        t.keys("u");
        assert_eq!(t.text(), "foo bar", "the delete and the typing go together");
    }

    #[test]
    fn dot_repeats_the_last_change() {
        check("|a b c d", "dw.", "|c d");
        check("|a\nb\nc\nd\n", "dd.", "|c\nd\n");
        check("|a b c", "cwz<esc>w.", "z |z c");
        check("|abc", "ix<esc>.", "|xxabc");
        check("|abcd", "x.", "|cd");
        check("|abcd", "x2.", "|d");
        check("a|\n", "Ax<esc>.", "ax|x\n");
        check("|a\nb\n", ">>j.", "    a\n    |b\n");
        // Moving around doesn't count as a change.
        check("|a b c", "xw.", " | c");
    }

    #[test]
    fn visual_mode() {
        let mut t = T::new("|abc def");
        t.keys("vl");
        assert_eq!(t.sel(), "ab");
        t.keys("e");
        assert_eq!(t.sel(), "abc");
        t.keys("d");
        assert_eq!(t.show(), "| def");
        assert_eq!(t.vim.mode, Mode::Normal);

        check("|a\nb\nc\n", "Vjd", "|c\n");
        check("|a\nb\n", "Vjy", "|a\nb\n");
        check("foo |bar", "viwcz<esc>", "foo |z");
        check("|abc", "vly", "|abc");
        check("|ab cd", "vey", "|ab cd");
        check("|ab", "vU", "|Ab");
        check("|a\nb\n", "Vj>", "    |a\n    b\n");
        check("|abc xyz", "yiwwviwp", "abc ab|c");

        // Going backward from the anchor keeps its char selected.
        let mut t = T::new("ab|cd");
        t.keys("vh");
        assert_eq!(t.sel(), "bc");
        // o jumps to the other end.
        let mut t = T::new("|abcd");
        t.keys("vlo");
        assert_eq!(t.sel(), "ab");
        t.keys("<esc>");
        assert_eq!(t.vim.mode, Mode::Normal);
        assert!(t.cur.primary().is_empty());
    }

    #[test]
    fn visual_block() {
        let mut t = T::new("|abc\ndef\nghi\n");
        t.keys("<c-v>jjl");
        let parts: Vec<_> = t.cur.iter().map(|s| t.doc.slice_string(s.start(), s.end())).collect();
        assert_eq!(parts, vec!["ab", "de", "gh"]);
        t.keys("d");
        assert_eq!(t.text(), "c\nf\ni\n");
        let mut t = T::new("|abc\ndef\n");
        t.keys("<c-v>jIx");
        // The editor would type at both cursors; here the test does it for it.
        assert_eq!(t.text(), "xabc\nxdef\n");
        t.keys("<esc>");
        assert_eq!(t.vim.mode, Mode::Normal);
        assert_eq!(t.cur.len(), 1);
    }

    #[test]
    fn macros() {
        let mut t = T::new("|a\nb\nc\nd\n");
        t.keys("qqIx<esc>jq");
        assert_eq!(t.text(), "xa\nb\nc\nd\n");
        t.keys("@q");
        assert_eq!(t.text(), "xa\nxb\nc\nd\n");
        t.keys("@@");
        assert_eq!(t.text(), "xa\nxb\nxc\nd\n");
        let mut t = T::new("|a\nb\nc\n");
        t.keys("qaAz<esc>jq2@a");
        assert_eq!(t.text(), "az\nbz\ncz\n");
    }

    #[test]
    fn marks() {
        check("|a\nb\nc\n", "majj'a", "|a\nb\nc\n");
        check("x|y\nb\n", "maj`a", "x|y\nb\n");
        check("|a\nb\nc\nd\n", "majjd'a", "|d\n");
    }

    #[test]
    fn numbers() {
        check("x = |9", "<c-a>", "x = 1|0");
        check("|x = 9", "<c-x>", "x = |8");
        check("|-1", "3<c-a>", "|2");
    }

    #[test]
    fn scrolling_moves_the_caret() {
        let src = format!("|{}", "x\n".repeat(50));
        let mut t = T::new(&src);
        t.keys("<c-d>");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 5);
        t.keys("<c-f>");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 15);
        t.keys("<c-u>");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 10);
        t.keys("<c-b>");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 0);
    }

    #[test]
    fn top_middle_and_bottom_of_the_screen() {
        let src = format!("|{}", "  x\n".repeat(50));
        let line = |t: &T| t.doc.char_to_line(t.cur.head());
        let mut t = T::new(&src);
        t.first_row = 20;
        t.keys("L");
        assert_eq!(line(&t), 29);
        assert_eq!(t.doc.line_col(t.cur.head()).1, 2, "first non-blank");
        t.keys("H");
        assert_eq!(line(&t), 20);
        t.keys("M");
        assert_eq!(line(&t), 24);
        t.keys("3H");
        assert_eq!(line(&t), 22);
        t.keys("2L");
        assert_eq!(line(&t), 28);
        // An operator takes the whole lines between here and there.
        let mut t = T::new(&src);
        t.keys("dL");
        assert_eq!(t.doc.len_lines(), 41);
        // A short file ends before the screen does.
        let mut t = T::new("|a\nb\nc\n");
        t.keys("L");
        assert_eq!(line(&t), 2);
    }

    #[test]
    fn search() {
        let mut t = T::new("|foo\nbar\nfoo\nbaz\nfoo\n");
        let a = t.ex('/', "foo");
        assert_eq!(a, vec![Action::Highlight(Some("(?i)foo".into()))]);
        assert_eq!(t.doc.char_to_line(t.cur.head()), 2);
        t.keys("n");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 4);
        t.keys("n");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 0, "wraps");
        t.keys("N");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 4);
        t.ex('?', "bar");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 1);
        // Smart case, and a search for nothing says so.
        assert!(matches!(t.ex('/', "FOO")[0], Action::Message(_)));
        // d with a search motion is d-n.
        let mut t = T::new("|ab foo cd foo\n");
        t.ex('/', "foo");
        t.keys("dn");
        assert_eq!(t.text(), "ab foo\n");
        let mut t = T::new("fo|o bar foo\nfoo2\n");
        t.keys("*");
        assert_eq!(t.cur.head(), 8);
    }

    #[test]
    fn ex_commands() {
        let mut t = T::new("|a\nb\nc\n");
        assert_eq!(t.ex(':', "w"), vec![Action::Save]);
        assert_eq!(t.ex(':', "wq"), vec![Action::SaveQuit]);
        assert_eq!(t.ex(':', "q!"), vec![Action::Quit { force: true }]);
        assert_eq!(t.ex(':', "e src/a.rs"), vec![Action::OpenFile("src/a.rs".into())]);
        assert!(matches!(t.ex(':', "bogus")[0], Action::Message(_)));
        t.ex(':', "3");
        assert_eq!(t.doc.char_to_line(t.cur.head()), 2);
        t.ex(':', "2,3d");
        assert_eq!(t.text(), "a\n");
    }

    #[test]
    fn substitute() {
        let mut t = T::new("|foo foo\nfoo\nbar\n");
        t.ex(':', "s/foo/x/");
        assert_eq!(t.text(), "x foo\nfoo\nbar\n");
        let mut t = T::new("|foo foo\nfoo\nbar\n");
        t.ex(':', "%s/foo/x/g");
        assert_eq!(t.text(), "x x\nx\nbar\n");
        let mut t = T::new("|ab\n");
        t.ex(':', r"%s/\(a\)\(b\)/\2\1-&/");
        assert_eq!(t.text(), "ba-ab\n");
        let mut t = T::new("|a(b)\n");
        t.ex(':', "s/(b)/$1/");
        assert_eq!(t.text(), "a$1\n", "parentheses and dollars are literal, as in Vim");
        let mut t = T::new("|a/b\n");
        t.ex(':', r"s#/#\\#");
        assert_eq!(t.text(), "a\\b\n");
        let mut t = T::new("|FOO\n");
        t.ex(':', "s/foo/x/i");
        assert_eq!(t.text(), "x\n");
        let mut t = T::new("|a\na\na\n");
        t.keys("Vj<esc>");
        t.ex(':', "'<,'>s/a/b/");
        assert_eq!(t.text(), "b\nb\na\n");
        let mut t = T::new("|x\n");
        assert!(matches!(t.ex(':', "s/nope/y/")[0], Action::Message(_)));
    }

    #[test]
    fn escape_cancels_whatever_was_going_on() {
        let mut t = T::new("|abc");
        t.keys("d<esc>x");
        assert_eq!(t.text(), "bc");
        t.keys("3<esc>x");
        assert_eq!(t.text(), "c");
        t.keys("f<esc>x");
        assert_eq!(t.text(), "");
    }

    #[test]
    fn caret_stays_on_a_char() {
        check("|ab\n", "ax<esc>", "a|xb\n");
        check("a|b\n", "x", "|a\n");
        check("|ab\n", "$a<esc>", "a|b\n");
        // Unknown keys in normal mode aren't taken from the editor: nothing gets typed.
        let mut t = T::new("|ab");
        t.keys("Q");
        assert_eq!(t.text(), "ab");
    }

    #[test]
    fn keys_vim_does_not_want_go_back_to_the_editor() {
        let mut t = T::new("|ab");
        let map = DisplayMap::new(&t.doc, &Folds::default(), None, 4);
        let mut ctx = Ctx { doc: &mut t.doc, cur: &mut t.cur, map: &map, page_rows: 10, first_row: 0, opts: EditOpts::default() };
        assert!(!t.vim.key(&mut ctx, Key::ctrl('p')).handled, "Ctrl+P stays the file picker");
        assert!(!t.vim.key(&mut ctx, Key::ctrl('s')).handled);
        assert!(t.vim.key(&mut ctx, Key::ch('l')).handled);
    }

    #[test]
    fn non_ascii_text_moves_by_chars() {
        check("|ação ok", "w", "ação |ok");
        check("|ação ok", "x", "|ção ok");
        check("|日本語 x", "dw", "|x");
    }
}
