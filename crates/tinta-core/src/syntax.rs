//! Syntax highlighting over syntect grammars, one line at a time.
//!
//! Lines keep their spans until they are re-parsed, so an edit does not flicker, and parsing
//! stops as soon as the state after an edit matches what was there before: typing in the middle
//! of a 100k-line file costs a few lines, not the rest of the file. Work is bounded by a time
//! budget per call, so a huge file highlights in the background of the frame loop.

use std::rc::Rc;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use syntect::highlighting::{
    Color, FontStyle, HighlightIterator, HighlightState, Highlighter as SynHighlighter, ScopeSelectors,
    StyleModifier, Theme, ThemeItem, ThemeSettings,
};
use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxSet};

pub fn syntax_set() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: [u8; 3],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

/// A run of equally styled chars. Columns are chars, not bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub style: Style,
}

#[derive(Debug, Clone, Default)]
pub struct LineInfo {
    pub spans: Box<[Span]>,
    /// Comment and string ranges, where brackets do not count.
    pub noncode: Box<[(u32, u32)]>,
    /// Nesting depth of brackets at the line's start.
    pub depth: i32,
}

#[derive(Clone)]
struct State {
    parse: ParseState,
    hl: HighlightState,
    scopes: ScopeStack,
    depth: i32,
}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        self.depth == other.depth && self.parse == other.parse && self.scopes == other.scopes
    }
}

/// Lines longer than this are shown plain: grammars slow down badly on minified files.
const MAX_LINE_LEN: usize = 8_000;
const CHECKPOINT_EVERY: usize = 16;

pub struct Highlighter {
    theme: Arc<Theme>,
    initial: State,
    lines: Vec<LineInfo>,
    checkpoints: Vec<Option<Rc<State>>>,
    /// Every line before this one is up to date.
    frontier: usize,
    /// Past this line a matching checkpoint means the rest of the file is still right.
    resync_after: usize,
    /// Where lines stop being known at all (nothing parsed yet).
    known: usize,
    text_gen: u64,
}

impl Highlighter {
    pub fn new(language: &str, theme: Arc<Theme>) -> Option<Self> {
        let ss = syntax_set();
        let syntax = ss.find_syntax_by_name(language)?;
        let hl = SynHighlighter::new(&theme);
        let initial = State {
            parse: ParseState::new(syntax),
            hl: HighlightState::new(&hl, ScopeStack::new()),
            scopes: ScopeStack::new(),
            depth: 0,
        };
        Some(Self {
            theme,
            initial,
            lines: Vec::new(),
            checkpoints: Vec::new(),
            frontier: 0,
            resync_after: 0,
            known: 0,
            text_gen: 0,
        })
    }

    pub fn generation(&self) -> u64 {
        self.text_gen
    }

    pub fn is_done(&self, total_lines: usize) -> bool {
        self.frontier >= total_lines
    }

    pub fn frontier(&self) -> usize {
        self.frontier
    }

    pub fn line(&self, line: usize) -> Option<&LineInfo> {
        self.lines.get(line)
    }

    /// True when `col` of `line` is inside a comment or string.
    pub fn is_noncode(&self, line: usize, col: usize) -> bool {
        self.lines.get(line).is_some_and(|l| {
            l.noncode.iter().any(|&(s, e)| (s as usize) <= col && col < e as usize)
        })
    }

    pub fn on_change(&mut self, start_line: usize, removed_lines: usize, inserted_lines: usize) {
        if self.lines.len() > start_line {
            let end = (start_line + 1 + removed_lines).min(self.lines.len());
            let fresh = inserted_lines + 1;
            // Keep the old spans of the edited line while it waits to be parsed: no flicker.
            let keep = self.lines[start_line].clone();
            self.lines.splice(start_line..end, std::iter::repeat_with(|| keep.clone()).take(fresh));
            self.checkpoints.splice(start_line..end, std::iter::repeat_n(None, fresh));
            let delta = inserted_lines as isize - removed_lines as isize;
            self.known = self.lines.len().min((self.known as isize + delta).max(0) as usize);
            // An edit still waiting further down must not be skipped by an early stop.
            let pending = if self.resync_after >= start_line {
                (self.resync_after as isize + delta).max(0) as usize
            } else {
                self.resync_after
            };
            let mut resync = pending.max(start_line + inserted_lines);
            // A parse that stopped partway left two kinds of saved states: fresh ones before where
            // it stopped and old ones after. Going back over the fresh ones, a match would say
            // the rest is right while the lines after the stop still wait for an earlier edit.
            let old_frontier = self.frontier;
            if old_frontier > start_line {
                let stale_from = if old_frontier > start_line + removed_lines {
                    (old_frontier as isize + delta).max(0) as usize
                } else {
                    start_line + inserted_lines
                };
                // `known` has already moved with the edit.
                if stale_from < self.known {
                    resync = resync.max(stale_from);
                }
            }
            self.resync_after = resync;
            self.frontier = self.frontier.min(start_line);
        }
        self.text_gen += 1;
    }

    /// Parses on from the frontier until the budget runs out. True if anything changed.
    pub fn advance(&mut self, rope: &ropey::Rope, budget: Duration) -> bool {
        self.advance_inner(rope, budget, None)
    }

    /// Parses through `line` if that's close enough to do now, so a question about it (is the
    /// caret in a comment?) has an answer. Lines far past the frontier are left to `advance`:
    /// getting there would spend the time on lines nobody is asking about.
    pub fn advance_through(&mut self, rope: &ropey::Rope, line: usize, budget: Duration) -> bool {
        const NEAR: usize = 512;
        if line < self.frontier || line > self.frontier + NEAR {
            return false;
        }
        self.advance_inner(rope, budget, Some(line))
    }

    fn advance_inner(&mut self, rope: &ropey::Rope, budget: Duration, stop_after: Option<usize>) -> bool {
        let total = rope.len_lines();
        self.lines.resize_with(total, LineInfo::default);
        self.checkpoints.resize(total, None);
        self.known = self.known.min(total);
        if self.frontier >= total {
            return false;
        }
        let ss = syntax_set();
        let highlighter = SynHighlighter::new(&self.theme);
        // Back up to the nearest saved state. Only those before the frontier can be trusted.
        let mut line = self.frontier.saturating_sub(1);
        let mut state = loop {
            if line == 0 {
                break self.initial.clone();
            }
            if let Some(cp) = &self.checkpoints[line] {
                break (**cp).clone();
            }
            line -= 1;
        };
        let start = Instant::now();
        let mut buf = String::new();
        let mut changed = false;
        while line < total {
            if line >= self.frontier {
                // Every call gets at least one line done, however little time it was given.
                if changed && start.elapsed() > budget {
                    break;
                }
                changed = true;
            }
            if line > 0 {
                // An edit that added or removed lines leaves old states off the regular spacing.
                // They still say how this text was parsed before, so they can end the work early,
                // but past this point they must not stay: the next call backs up to whatever it
                // finds behind the frontier, and an old state there would color the rest wrong.
                let on_spacing = line.is_multiple_of(CHECKPOINT_EVERY);
                let ahead = line >= self.frontier;
                if ahead || on_spacing {
                    let same = self.checkpoints[line].as_deref().is_some_and(|cp| *cp == state);
                    if same && line > self.resync_after && ahead && line < self.known {
                        // Nothing from here on changes.
                        self.frontier = self.known;
                        self.resync_after = 0;
                        return changed;
                    }
                    if !on_spacing {
                        self.checkpoints[line] = None;
                    } else if !same {
                        self.checkpoints[line] = Some(Rc::new(state.clone()));
                    }
                }
            }
            buf.clear();
            let slice = rope.line(line);
            buf.extend(slice.chunks());
            let info = parse_line(&buf, &mut state, ss, &highlighter);
            if line >= self.frontier {
                self.lines[line] = info;
                self.frontier = line + 1;
                self.known = self.known.max(self.frontier);
            }
            if stop_after.is_some_and(|s| line >= s) {
                return changed;
            }
            line += 1;
        }
        if line >= total {
            self.resync_after = 0;
        }
        changed
    }
}

fn is_noncode_scope(stack: &ScopeStack) -> bool {
    static COMMENT: OnceLock<Scope> = OnceLock::new();
    static STRING: OnceLock<Scope> = OnceLock::new();
    let comment = *COMMENT.get_or_init(|| Scope::new("comment").unwrap());
    let string = *STRING.get_or_init(|| Scope::new("string").unwrap());
    stack.as_slice().iter().any(|s| comment.is_prefix_of(*s) || string.is_prefix_of(*s))
}

fn parse_line(text: &str, state: &mut State, ss: &SyntaxSet, hl: &SynHighlighter) -> LineInfo {
    let depth_at_start = state.depth;
    if text.len() > MAX_LINE_LEN {
        return LineInfo { depth: depth_at_start, ..LineInfo::default() };
    }
    let Ok(ops) = state.parse.parse_line(text, ss) else {
        return LineInfo { depth: depth_at_start, ..LineInfo::default() };
    };
    // Walk the scope ops to learn what is code; byte offsets become chars below.
    let mut noncode_bytes: Vec<(usize, usize)> = Vec::new();
    let mut pos = 0usize;
    let mut scopes = state.scopes.clone();
    let mut push_range = |from: usize, to: usize, stack: &ScopeStack| {
        if to > from && is_noncode_scope(stack) {
            match noncode_bytes.last_mut() {
                Some(last) if last.1 == from => last.1 = to,
                _ => noncode_bytes.push((from, to)),
            }
        }
    };
    for (offset, op) in &ops {
        push_range(pos, *offset, &scopes);
        pos = *offset;
        let _ = scopes.apply(op);
    }
    push_range(pos, text.len(), &scopes);
    state.scopes = scopes;

    let mut spans = Vec::new();
    let mut col = 0u32;
    for (style, piece) in HighlightIterator::new(&mut state.hl, &ops, text, hl) {
        let n = piece.trim_end_matches('\n').chars().count() as u32;
        if n == 0 {
            continue;
        }
        spans.push(Span {
            start: col,
            end: col + n,
            style: Style {
                fg: [style.foreground.r, style.foreground.g, style.foreground.b],
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: style.font_style.contains(FontStyle::UNDERLINE),
            },
        });
        col += n;
    }
    // Merge neighbours: fewer spans, fewer draw calls.
    let mut merged: Vec<Span> = Vec::with_capacity(spans.len());
    for s in spans {
        match merged.last_mut() {
            Some(last) if last.style == s.style => last.end = s.end,
            _ => merged.push(s),
        }
    }

    let mut noncode = Vec::with_capacity(noncode_bytes.len());
    let mut depth = state.depth;
    let (mut byte, mut ccol) = (0usize, 0u32);
    let mut iter = noncode_bytes.iter().copied().peekable();
    let mut start_col = None;
    for ch in text.chars() {
        while let Some(&(s, e)) = iter.peek() {
            if byte >= e {
                noncode.push((start_col.take().unwrap_or(ccol), ccol));
                iter.next();
            } else {
                if byte >= s && start_col.is_none() {
                    start_col = Some(ccol);
                }
                break;
            }
        }
        if start_col.is_none() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                _ => {}
            }
        }
        byte += ch.len_utf8();
        ccol += 1;
    }
    if let Some(s) = start_col {
        noncode.push((s, ccol));
    }
    state.depth = depth;
    LineInfo { spans: merged.into(), noncode: noncode.into(), depth: depth_at_start }
}

// Themes -------------------------------------------------------------------------------------

fn color(hex: u32) -> Color {
    Color { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8, a: 255 }
}

fn item(scope: &str, fg: u32, style: Option<FontStyle>) -> ThemeItem {
    ThemeItem {
        scope: ScopeSelectors::from_str(scope).expect("valid selector"),
        style: StyleModifier { foreground: Some(color(fg)), background: None, font_style: style },
    }
}

/// Palette in the spirit of COSMIC's own accents, one for dark windows and one for light.
pub fn cosmic_theme(dark: bool) -> Theme {
    let p: [u32; 11] = if dark {
        // fg, comment, keyword, string, number, function, type, variable, tag, operator, invalid
        [0xE2E4E9, 0x8A929C, 0xC69BFF, 0x98D77E, 0xFFB86B, 0x7CC4FF, 0x5FD7C8, 0xF0A8B8, 0xFF8F8F, 0x9AC8E8, 0xFF6B6B]
    } else {
        [0x24272C, 0x6B7380, 0x7A3FD1, 0x2F7D1E, 0xB25E00, 0x0B62C4, 0x007F73, 0xB5345F, 0xC22F2F, 0x2B6C94, 0xC00000]
    };
    build_theme(if dark { "COSMIC Dark" } else { "COSMIC Light" }, p)
}

/// GNOME's Adwaita palette (the colours of the HIG, as in GtkSourceView's Adwaita schemes),
/// one for dark windows and one for light.
pub fn adwaita_theme(dark: bool) -> Theme {
    let p: [u32; 11] = if dark {
        // fg, comment, keyword, string, number, function, type, variable, tag, operator, invalid
        [0xDEDDDA, 0x9A9996, 0xFFA348, 0xF8E45C, 0xDC8ADD, 0x99C1F1, 0x8FF0A4, 0xF66151, 0x62A0EA, 0xC0BFBC, 0xFF7B63]
    } else {
        [0x241F31, 0x77767B, 0xC64600, 0x986A44, 0x813D9C, 0x1A5FB4, 0x26A269, 0xC01C28, 0x1C71D8, 0x5E5C64, 0xE01B24]
    };
    build_theme(if dark { "Adwaita Dark" } else { "Adwaita Light" }, p)
}

fn build_theme(name: &str, p: [u32; 11]) -> Theme {
    let [fg, comment, keyword, string, number, function, ty, variable, tag, operator, invalid] = p;
    let italic = Some(FontStyle::ITALIC);
    let bold = Some(FontStyle::BOLD);
    let scopes = vec![
        item("comment, punctuation.definition.comment", comment, italic),
        item("string, punctuation.definition.string", string, None),
        item("constant.character.escape, string.regexp", operator, None),
        item("constant.numeric, constant.language, constant.character, constant.other", number, None),
        item("keyword, storage, keyword.control, storage.type, storage.modifier", keyword, None),
        item("keyword.operator, punctuation.separator, punctuation.accessor", operator, None),
        item("entity.name.function, support.function, meta.function-call variable.function, variable.function", function, None),
        item("entity.name.type, entity.name.class, entity.name.struct, entity.name.enum, entity.name.trait, entity.name.interface, entity.name.namespace, support.type, support.class, entity.other.inherited-class", ty, None),
        item("variable.parameter, variable.other.member, variable.other.readwrite.instance", variable, None),
        item("variable.language", tag, italic),
        item("entity.name.tag, meta.tag.sgml", tag, None),
        item("entity.other.attribute-name", number, None),
        item("entity.name.constant, variable.other.constant, support.constant", number, None),
        item("meta.preprocessor, keyword.control.import, keyword.other.import, meta.attribute, meta.annotation", ty, None),
        item("markup.heading, entity.name.section, punctuation.definition.heading", function, bold),
        item("markup.bold", fg, bold),
        item("markup.italic", fg, italic),
        item("markup.raw, markup.inline.raw", string, None),
        item("markup.underline.link, string.other.link", ty, None),
        item("markup.list, punctuation.definition.list_item", keyword, None),
        item("markup.quote", comment, italic),
        item("markup.inserted, meta.diff.header.to-file", string, None),
        item("markup.deleted, meta.diff.header.from-file", tag, None),
        item("meta.diff.range, meta.diff.header", function, None),
        item("invalid, invalid.illegal", invalid, None),
    ];
    Theme {
        name: Some(name.into()),
        author: None,
        settings: ThemeSettings { foreground: Some(color(fg)), ..ThemeSettings::default() },
        scopes,
    }
}

/// Names shown in settings: "COSMIC" and "Adwaita" follow the desktop, the rest are fixed.
pub fn theme_names() -> Vec<String> {
    let mut out = vec!["COSMIC".to_string(), "Adwaita".to_string()];
    out.extend(extra_themes().iter().map(|(n, _)| n.clone()));
    out
}

fn extra_themes() -> &'static Vec<(String, Arc<Theme>)> {
    static T: OnceLock<Vec<(String, Arc<Theme>)>> = OnceLock::new();
    T.get_or_init(|| {
        use two_face::theme::{EmbeddedThemeName as N, extra};
        let set = extra();
        [
            ("Catppuccin Latte", N::CatppuccinLatte),
            ("Catppuccin Mocha", N::CatppuccinMocha),
            ("Dracula", N::Dracula),
            ("GitHub", N::Github),
            ("Gruvbox Dark", N::GruvboxDark),
            ("Gruvbox Light", N::GruvboxLight),
            ("Monokai", N::MonokaiExtended),
            ("Nord", N::Nord),
            ("One Half Dark", N::OneHalfDark),
            ("One Half Light", N::OneHalfLight),
            ("Solarized Dark", N::SolarizedDark),
            ("Solarized Light", N::SolarizedLight),
            ("Zenburn", N::Zenburn),
        ]
        .into_iter()
        .map(|(n, t)| (n.to_string(), Arc::new(set.get(t).clone())))
        .collect()
    })
}

/// Finds a theme by name; "COSMIC" (or anything unknown) adapts to the desktop's mode.
pub fn theme_by_name(name: &str, dark: bool) -> Arc<Theme> {
    if let Some((_, t)) = extra_themes().iter().find(|(n, _)| n == name) {
        return t.clone();
    }
    if name == "Adwaita" {
        static AD: OnceLock<Arc<Theme>> = OnceLock::new();
        static AL: OnceLock<Arc<Theme>> = OnceLock::new();
        let cell = if dark { &AD } else { &AL };
        return cell.get_or_init(|| Arc::new(adwaita_theme(dark))).clone();
    }
    static D: OnceLock<Arc<Theme>> = OnceLock::new();
    static L: OnceLock<Arc<Theme>> = OnceLock::new();
    if dark { D.get_or_init(|| Arc::new(cosmic_theme(true))).clone() } else { L.get_or_init(|| Arc::new(cosmic_theme(false))).clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    fn run(h: &mut Highlighter, r: &Rope) {
        while !h.is_done(r.len_lines()) {
            h.advance(r, Duration::from_millis(50));
        }
    }

    fn hl(lang: &str) -> Highlighter {
        Highlighter::new(lang, theme_by_name("COSMIC", true)).unwrap()
    }

    #[test]
    fn colours_keywords_and_strings_differently() {
        let r = Rope::from_str("fn main() { let s = \"oi\"; }\n");
        let mut h = hl("Rust");
        run(&mut h, &r);
        let spans = &h.line(0).unwrap().spans;
        assert!(spans.len() > 3);
        let col = |c: u32| spans.iter().find(|s| s.start <= c && c < s.end).unwrap().style.fg;
        assert_ne!(col(0), col(21), "keyword vs string");
    }

    #[test]
    fn all_cosmic_selectors_parse_and_every_language_loads() {
        cosmic_theme(true);
        cosmic_theme(false);
        adwaita_theme(true);
        adwaita_theme(false);
        assert_eq!(theme_by_name("Adwaita", true).name.as_deref(), Some("Adwaita Dark"));
        assert_eq!(theme_by_name("Adwaita", false).name.as_deref(), Some("Adwaita Light"));
        assert!(syntax_set().syntaxes().len() > 100);
        assert!(theme_names().len() > 10);
    }

    #[test]
    fn multiline_constructs_carry_over_and_resync_stops_early() {
        let mut src = String::from("/* start\n");
        for i in 0..4000 {
            src.push_str(&format!("let a{i} = {i};\n"));
        }
        src.push_str("*/\nfn x() {}\n");
        let mut r = Rope::from_str(&src);
        let mut h = hl("Rust");
        run(&mut h, &r);
        assert!(h.is_noncode(100, 0), "inside block comment");
        let last = r.len_lines() - 2;
        assert!(!h.is_noncode(last, 0), "after the comment");

        // Editing inside the comment does not change what follows: parsing stops early.
        r.insert_char(r.line_to_char(200), 'x');
        h.on_change(200, 0, 0);
        let t = Instant::now();
        h.advance(&r, Duration::from_secs(5));
        assert!(h.is_done(r.len_lines()));
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
        assert!(h.is_noncode(3000, 0));

        // Closing the comment early does change the rest of the file.
        let at = r.line_to_char(10);
        r.insert(at, "*/\n");
        h.on_change(10, 0, 1);
        run(&mut h, &r);
        assert!(!h.is_noncode(3000, 0));
        assert!(h.is_noncode(5, 0));
    }

    /// Opening a comment, parsing only part of what follows, then typing above it: the lines
    /// past where the parse stopped still have to turn into comment.
    #[test]
    fn an_edit_above_a_half_done_parse_does_not_end_it_early() {
        let mut src = String::new();
        for i in 0..2000 {
            src.push_str(&format!("let a{i} = {i};\n"));
        }
        let mut r = Rope::from_str(&src);
        let mut h = hl("Rust");
        run(&mut h, &r);
        assert!(!h.is_noncode(1500, 0));
        r.insert(r.line_to_char(10), "/*");
        h.on_change(10, 0, 0);
        h.advance_through(&r, 300, Duration::from_secs(5));
        assert_eq!(h.frontier(), 301);
        // Typing on line 5 changes nothing that carries over to the lines after it.
        r.insert_char(r.line_to_char(5), 'x');
        h.on_change(5, 0, 0);
        run(&mut h, &r);
        assert!(h.is_noncode(200, 0));
        assert!(h.is_noncode(1500, 0), "past where the first parse stopped");
    }

    #[test]
    fn advance_through_parses_up_to_the_line_and_no_further() {
        let mut src = String::new();
        for i in 0..3000 {
            src.push_str(&format!("let a{i} = {i}; // c\n"));
        }
        let r = Rope::from_str(&src);
        let mut h = hl("Rust");
        // Close to the frontier: done, and nothing past it.
        assert!(h.advance_through(&r, 40, Duration::from_secs(5)));
        assert_eq!(h.frontier(), 41);
        assert!(h.is_noncode(40, 14) && !h.is_noncode(40, 0));
        // Far ahead of it: left for the background parse.
        assert!(!h.advance_through(&r, 2000, Duration::from_secs(5)));
        assert_eq!(h.frontier(), 41);
        // Behind it: already known.
        assert!(!h.advance_through(&r, 10, Duration::from_secs(5)));
        run(&mut h, &r);
        assert!(h.is_done(r.len_lines()));
        assert!(h.is_noncode(2000, 18) && !h.is_noncode(2000, 14));
    }

    #[test]
    fn an_edit_that_adds_lines_survives_being_parsed_in_slices() {
        // The way a big file gets parsed: a bit per frame, so each call resumes from a saved state.
        let mut src = String::from("/*\n");
        for i in 0..1500 {
            src.push_str(&format!("texto {i}\n"));
        }
        src.push_str("*/\nfn x() {}\n");
        let mut r = Rope::from_str(&src);
        let mut h = hl("Rust");
        run(&mut h, &r);
        assert!(h.is_noncode(1000, 0));
        // Closing the comment near the top, with a line added, shifts every saved state by one.
        let at = r.line_to_char(10);
        r.insert(at, "*/\n");
        h.on_change(10, 0, 1);
        let mut calls = 0;
        while !h.is_done(r.len_lines()) {
            h.advance(&r, Duration::ZERO);
            calls += 1;
            assert!(calls < 5000, "every call makes progress");
        }
        for line in [11, 12, 100, 500, 1000, 1500] {
            assert!(!h.is_noncode(line, 0), "line {line} is out of the comment now");
        }
        assert!(h.is_noncode(5, 0));
        // And the other way: opening one, with lines taken away.
        let (a, b) = (r.line_to_char(10), r.line_to_char(11));
        r.remove(a..b);
        h.on_change(10, 1, 0);
        while !h.is_done(r.len_lines()) {
            h.advance(&r, Duration::ZERO);
        }
        for line in [11, 100, 500, 1000] {
            assert!(h.is_noncode(line, 0), "line {line} is back in the comment");
        }
    }

    #[test]
    fn adding_a_line_still_stops_early_when_nothing_else_changes() {
        let mut src = String::new();
        for i in 0..50_000 {
            src.push_str(&format!("let x{i} = {i};\n"));
        }
        let mut r = Rope::from_str(&src);
        let mut h = hl("Rust");
        run(&mut h, &r);
        let at = r.line_to_char(1000);
        r.insert(at, "let novo = 1;\n");
        h.on_change(1000, 0, 1);
        h.advance(&r, Duration::from_secs(30));
        assert!(h.is_done(r.len_lines()));
        assert!(h.frontier() >= r.len_lines() - 1);
        // One call, and it didn't take anything like parsing 49 thousand lines would.
        let t = Instant::now();
        let at = r.line_to_char(2000);
        r.insert(at, "let outro = 2;\n");
        h.on_change(2000, 0, 1);
        h.advance(&r, Duration::from_secs(30));
        assert!(t.elapsed() < Duration::from_millis(400), "{:?}", t.elapsed());
    }

    #[test]
    fn deleting_lines_shrinks_cache_consistently() {
        let mut r = Rope::from_str("a\nb\nc\nd\n");
        let mut h = hl("Rust");
        run(&mut h, &r);
        let (s, e) = (r.line_to_char(1), r.line_to_char(3));
        r.remove(s..e);
        h.on_change(1, 2, 0);
        run(&mut h, &r);
        assert_eq!(h.lines.len(), r.len_lines());
    }

    #[test]
    fn brackets_in_strings_do_not_change_depth() {
        let r = Rope::from_str("let a = \"((((\";\nfn f() {\n    x\n}\ny\n");
        let mut h = hl("Rust");
        run(&mut h, &r);
        assert_eq!(h.line(1).unwrap().depth, 0);
        assert_eq!(h.line(2).unwrap().depth, 1);
        assert_eq!(h.line(4).unwrap().depth, 0);
    }

    #[test]
    fn budget_is_respected_on_large_files() {
        let src = "fn x() { let a = 1; }\n".repeat(200_000);
        let r = Rope::from_str(&src);
        let mut h = hl("Rust");
        let t = Instant::now();
        h.advance(&r, Duration::from_millis(5));
        assert!(t.elapsed() < Duration::from_millis(100));
        assert!(!h.is_done(r.len_lines()));
        assert!(h.frontier() > 0);
    }
}
