//! Long runs of random edits, checked against the simple truth: undo gives back what was there,
//! and what is kept up to date edit by edit (colors, the syntax tree, the screen rows) matches
//! what a fresh start on the final text gives. Seeded, so a failure repeats.

use std::time::Duration;
use tinta_core::commands::{self, EditOpts, Motion};
use tinta_core::display::Folds;
use tinta_core::structure::Structure;
use tinta_core::syntax::{Highlighter, theme_by_name};
use tinta_core::{Cursors, DisplayMap, Document, Selection};

/// xorshift64*: small, and the same everywhere.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

const PIECES: &[&str] = &[
    "a", "x", " ", "\n", "\t", "{", "}", "(", ")", "\"", "'", "/*", "*/", "//", "fn ", "let ", "ção", "日本", "🦀", "\n    ",
    "struct S { x: u8 }\n", "if a { b(); }\n", "\"str\\\"ing\"", "/* multi\nline */", "#[derive(Debug)]\n", "r#\"raw\"#", "é",
];

const RUST: &str = "use std::fmt;\n\n/// Docs.\n#[derive(Debug)]\npub struct Point {\n    x: i32,\n    y: i32,\n}\n\nimpl fmt::Display for Point {\n    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {\n        // a comment\n        write!(f, \"({}, {})\", self.x, self.y)\n    }\n}\n\nfn main() {\n    let p = Point { x: 1, y: 2 };\n    /* block\n       comment */\n    println!(\"{p}\");\n}\n";

fn random_cursors(rng: &mut Rng, doc: &Document) -> Cursors {
    let n = doc.len_chars();
    let mut c = Cursors::single(Selection::cursor(rng.below(n + 1)));
    for _ in 0..rng.below(3) {
        let a = rng.below(n + 1);
        let b = if rng.below(2) == 0 { a } else { rng.below(n + 1) };
        c.add(Selection::new(a, b));
    }
    c
}

/// One random editing operation, the way the keyboard reaches them.
fn random_edit(rng: &mut Rng, doc: &mut Document, cur: &mut Cursors) {
    let map = DisplayMap::new(doc, &Folds::default(), None, 4);
    let opts = EditOpts { auto_pairs: rng.below(2) == 0, auto_indent: rng.below(2) == 0, surround: rng.below(2) == 0 };
    match rng.below(22) {
        0..=5 => commands::type_text(doc, cur, rng.pick(PIECES), opts),
        6 => commands::newline(doc, cur, opts),
        7 | 8 => commands::backspace(doc, cur, opts),
        9 => commands::delete_forward(doc, cur),
        10 => commands::delete_word(doc, cur, rng.below(2) == 0),
        11 => commands::delete_lines(doc, cur),
        12 => commands::duplicate(doc, cur),
        13 => commands::move_lines(doc, cur, rng.below(2) == 0),
        14 => {
            if rng.below(2) == 0 {
                commands::indent_lines(doc, cur)
            } else {
                commands::outdent_lines(doc, cur)
            }
        }
        15 => commands::toggle_line_comment(doc, cur),
        16 => commands::join_lines(doc, cur),
        17 => commands::paste_each(doc, cur, &[rng.pick(PIECES).to_string()]),
        18 => commands::transpose(doc, cur),
        19 => {
            let _ = commands::cut(doc, cur);
        }
        20 => {
            let m = *rng.pick(&[Motion::Left, Motion::WordRight, Motion::Down, Motion::LineEnd, Motion::ParagraphUp, Motion::PageDown(5)]);
            commands::move_cursors(doc, cur, &map, m, rng.below(2) == 0);
        }
        _ => *cur = random_cursors(rng, doc),
    }
}

fn check_cursors(doc: &Document, cur: &Cursors) {
    assert!(!cur.is_empty());
    for s in cur.iter() {
        assert!(s.anchor <= doc.len_chars() && s.head <= doc.len_chars(), "{s:?} past {}", doc.len_chars());
    }
}

#[test]
fn undo_and_redo_retrace_any_run_of_edits() {
    for seed in 1..=40u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut doc = Document::new(RUST);
        let mut cur = Cursors::single(Selection::cursor(0));
        let mut seen = vec![doc.rope.to_string()];
        for _ in 0..120 {
            random_edit(&mut rng, &mut doc, &mut cur);
            check_cursors(&doc, &cur);
            // No coalescing: every step is its own undo, so each one can be compared.
            doc.break_undo();
            let now = doc.rope.to_string();
            if *seen.last().unwrap() != now {
                seen.push(now);
            }
        }
        let last = doc.rope.to_string();
        let mut back = vec![last.clone()];
        while doc.undo(&mut cur) {
            check_cursors(&doc, &cur);
            back.push(doc.rope.to_string());
        }
        back.dedup();
        back.reverse();
        assert_eq!(back, seen, "seed {seed}: undo went through other texts");
        assert_eq!(doc.rope.to_string(), RUST, "seed {seed}");
        assert!(!doc.is_dirty(), "seed {seed}: back where it was saved");
        while doc.redo(&mut cur) {
            check_cursors(&doc, &cur);
        }
        assert_eq!(doc.rope.to_string(), last, "seed {seed}: redo");
        assert!(doc.is_dirty() || last == RUST);
    }
}

fn highlight_all(h: &mut Highlighter, rope: &ropey::Rope) {
    while !h.is_done(rope.len_lines()) {
        h.advance(rope, Duration::from_secs(1));
    }
}

#[test]
fn colors_kept_up_edit_by_edit_match_a_fresh_parse() {
    let theme = theme_by_name("COSMIC", true);
    for seed in 1..=25u64 {
        let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
        let mut doc = Document::new(&RUST.repeat(6));
        doc.highlighter = Highlighter::new("Rust", theme.clone());
        let mut cur = Cursors::single(Selection::cursor(0));
        for step in 0..150 {
            random_edit(&mut rng, &mut doc, &mut cur);
            // The way frames do it: a slice of parsing now and then, sometimes none at all.
            let line = rng.below(doc.len_lines());
            let rope = doc.rope.clone();
            let h = doc.highlighter.as_mut().unwrap();
            match rng.below(4) {
                0 => {}
                1 => {
                    h.advance(&rope, Duration::ZERO);
                }
                2 => {
                    h.advance_through(&rope, line, Duration::from_secs(1));
                }
                _ => highlight_all(h, &rope),
            }
            if step % 50 == 49 {
                let _ = doc.undo(&mut cur);
            }
        }
        let mut h = doc.highlighter.take().unwrap();
        highlight_all(&mut h, &doc.rope);
        let mut fresh = Highlighter::new("Rust", theme.clone()).unwrap();
        highlight_all(&mut fresh, &doc.rope);
        for line in 0..doc.len_lines() {
            let (a, b) = (h.line(line).unwrap(), fresh.line(line).unwrap());
            assert_eq!(a.spans, b.spans, "seed {seed}, line {line}: {:?}", doc.line_text(line));
            assert_eq!(a.noncode, b.noncode, "seed {seed}, line {line}");
            assert_eq!(a.depth, b.depth, "seed {seed}, line {line}");
        }
    }
}

/// Only the blocks an edit touched are looked at again; the rest is kept or moved. That must give
/// what looking at the whole tree gives. (Against a parse from scratch it can differ now and then:
/// tree-sitter's incremental parse may recover from syntax errors another way.)
#[test]
fn the_syntax_tree_kept_up_edit_by_edit_matches_looking_at_all_of_it() {
    for seed in 1..=80u64 {
        let mut rng = Rng(seed.wrapping_mul(0xA076_1D64_78BD_642F));
        let mut doc = Document::new(&RUST.repeat(3));
        doc.structure = Structure::new("Rust");
        doc.sync_structure();
        let mut cur = Cursors::single(Selection::cursor(0));
        for step in 0..100 {
            random_edit(&mut rng, &mut doc, &mut cur);
            if rng.below(3) == 0 {
                doc.sync_structure();
                let s = doc.structure.as_mut().unwrap();
                let (folds, symbols) = (s.folds().clone(), s.symbols().to_vec());
                s.rebuild_everything(&doc.rope);
                assert_eq!(&folds, s.folds(), "seed {seed}, step {step}: folds\n{}", doc.rope);
                assert_eq!(symbols, s.symbols(), "seed {seed}, step {step}: symbols\n{}", doc.rope);
            }
        }
    }
    // With no errors in the text, a parse from scratch agrees as well.
    let mut doc = Document::new(&RUST.repeat(3));
    doc.structure = Structure::new("Rust");
    doc.sync_structure();
    let mut cur = Cursors::single(Selection::cursor(0));
    let mut rng = Rng(99);
    for _ in 0..200 {
        // Whole valid items in and out: the text stays free of errors.
        let n = doc.len_lines();
        let line = rng.below(n);
        let at = doc.line_start(line);
        if doc.line_text(line).starts_with("fn ") || doc.line_text(line).is_empty() {
            let item = *rng.pick(&["fn added() {\n    let x = 1;\n}\n", "struct T;\n", "\n", "// note\n"]);
            doc.apply(vec![tinta_core::Edit::insert(at, item)], &mut cur, tinta_core::EditKind::Other, |_, _| {});
        }
        doc.sync_structure();
    }
    let kept = doc.structure.take().unwrap();
    let mut fresh = Structure::new("Rust").unwrap();
    fresh.update(&doc.rope, doc.revision);
    assert_eq!(kept.folds(), fresh.folds());
    assert_eq!(kept.symbols(), fresh.symbols());
}

#[test]
fn every_position_survives_a_trip_through_the_screen_rows() {
    for seed in 1..=30u64 {
        let mut rng = Rng(seed.wrapping_mul(0xE703_7ED1_A0B4_28DB));
        let mut doc = Document::new(RUST);
        let mut cur = Cursors::single(Selection::cursor(0));
        for _ in 0..40 {
            random_edit(&mut rng, &mut doc, &mut cur);
        }
        let mut folds = Folds::default();
        for _ in 0..rng.below(3) {
            let a = rng.below(doc.len_lines());
            let b = (a + 1 + rng.below(4)).min(doc.len_lines() - 1);
            if b > a {
                folds.add(a, b);
            }
        }
        let wrap = [None, Some(8), Some(20), Some(40)][rng.below(4)];
        let map = DisplayMap::new(&doc, &folds, wrap, 4);
        let rows = map.row_count(&doc);
        assert!(rows >= 1);
        for pos in 0..=doc.len_chars() {
            let line = doc.char_to_line(pos);
            let (row, vcol) = map.pos_to_row(&doc, pos);
            assert!(row < rows, "seed {seed}: {pos} on row {row} of {rows}");
            if folds.is_hidden(line) {
                continue;
            }
            let back = map.row_to_pos(&doc, row, vcol);
            // A wrapped line's break belongs to the row before it or after it; both are fine.
            let same_place = back == pos || (wrap.is_some() && map.pos_to_row(&doc, back).0.abs_diff(row) <= 1 && back.abs_diff(pos) <= 1);
            assert!(same_place, "seed {seed} wrap {wrap:?}: {pos} -> ({row}, {vcol}) -> {back}");
        }
        // Rows go through the lines in order, and each line's rows cover it.
        let mut last = (0u32, 0u32);
        for r in 0..rows {
            let row = map.row(&doc, r);
            assert!((row.line, row.start) >= last, "seed {seed}: row {r} goes back");
            assert!(row.start <= row.end && row.end as usize <= doc.line_len(row.line as usize));
            last = (row.line, row.start);
        }
    }
}

#[test]
fn find_all_agrees_with_a_plain_search() {
    let mut rng = Rng(7);
    for _ in 0..200 {
        let mut text = String::new();
        for _ in 0..rng.below(60) {
            text.push_str(rng.pick(&["ab", "Ab", "ção", " ", "\n", "日本", "🦀", "aba", "x"]));
        }
        let doc = Document::new(&text);
        let pattern = *rng.pick(&["ab", "a", "ção", "日", "🦀", "ba", "AB"]);
        let q = tinta_core::find::Query { pattern: pattern.into(), regex: false, case_sensitive: true, whole_word: false };
        let re = q.compile().unwrap();
        let got = tinta_core::find::find_all(&doc, &re, usize::MAX);
        let chars: Vec<char> = text.chars().collect();
        let p: Vec<char> = pattern.chars().collect();
        let mut want = Vec::new();
        let mut i = 0;
        while i + p.len() <= chars.len() {
            if chars[i..i + p.len()] == p[..] {
                want.push((i, i + p.len()));
                i += p.len();
            } else {
                i += 1;
            }
        }
        assert_eq!(got, want, "{pattern:?} in {text:?}");
        assert_eq!(tinta_core::find::find_all(&doc, &re, 2), want.iter().take(2).copied().collect::<Vec<_>>());
    }
}
