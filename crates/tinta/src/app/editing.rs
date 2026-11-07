//! Typing, the mouse in the text, find-as-you-type and keeping the caret in view.

use super::{App, FindMsg, Focus, Message};
use crate::state::TabKind;
use crate::ui::editor_view::{EditorEvent, Geometry, PressButton};
use cosmic::app::Task;
use cosmic::iced::keyboard::Modifiers;
use tinta_core::commands::{self, EditOpts};
use tinta_core::{Cursors, Selection, find};

pub const FIND_INPUT: &str = "find-input";
pub const REPLACE_INPUT: &str = "replace-input";
/// How many matches get highlighted. Replacing isn't limited by this.
pub const MAX_SHOWN_MATCHES: usize = 100_000;

impl App {
    pub fn edit_opts(&self) -> EditOpts {
        EditOpts { auto_pairs: self.settings.auto_pairs, auto_indent: self.settings.auto_indent, surround: self.settings.auto_pairs }
    }

    /// True when the active document may be edited. Read-only views refuse politely.
    pub fn can_edit(&self) -> bool {
        self.active().is_some_and(|(t, e)| !e.doc.readonly && !e.is_read_only_view && t.kind == TabKind::Text)
    }

    /// Runs an edit on the active document, then asks for the caret to come into view.
    pub fn edit(&mut self, f: impl FnOnce(&mut tinta_core::Document, &mut Cursors, &tinta_core::DisplayMap)) {
        if !self.can_edit() {
            return;
        }
        let id = self.active_tab_id();
        let Some((t, e)) = self.active_mut() else { return };
        f(&mut e.doc, &mut t.cursors, &t.map);
        self.edit_tab = id;
        self.want_reveal = true;
    }

    pub fn type_text(&mut self, text: &str) -> Task<Message> {
        if !self.can_edit() {
            return Task::none();
        }
        let opts = self.edit_opts();
        let text = text.to_string();
        self.edit(|d, c, _| commands::type_text(d, c, &text, opts));
        self.after_typing(&text)
    }

    pub fn key_enter(&mut self) -> Task<Message> {
        let opts = self.edit_opts();
        self.edit(|d, c, _| commands::newline(d, c, opts));
        self.close_popups();
        Task::none()
    }

    pub fn key_tab(&mut self, shift: bool) -> Task<Message> {
        if shift {
            self.edit(|d, c, _| commands::outdent_lines(d, c));
            return Task::none();
        }
        if self.expand_snippet_prefix() {
            return Task::none();
        }
        self.edit(|d, c, _| commands::tab(d, c));
        Task::none()
    }

    pub fn key_backspace(&mut self) -> Task<Message> {
        let opts = self.edit_opts();
        self.edit(|d, c, _| commands::backspace(d, c, opts));
        self.after_deleting()
    }

    pub fn key_delete(&mut self) -> Task<Message> {
        self.edit(|d, c, _| commands::delete_forward(d, c));
        self.after_deleting()
    }

    pub fn on_pasted(&mut self, text: Option<String>) -> Task<Message> {
        let Some(text) = text else { return Task::none() };
        if self.focus == Focus::Terminal {
            if let Some(s) = self.terms.get(self.active_term) {
                let mut bytes = Vec::new();
                let bracketed = s.is_bracketed_paste();
                if bracketed {
                    bytes.extend_from_slice(b"\x1b[200~");
                }
                bytes.extend_from_slice(text.replace('\n', "\r").as_bytes());
                if bracketed {
                    bytes.extend_from_slice(b"\x1b[201~");
                }
                s.write(bytes);
            }
            return Task::none();
        }
        let text = tinta_core::buffer::normalize_newlines(&text).into_owned();
        // Text copied here keeps how it was copied: per cursor, or as whole lines.
        let meta = self.clip_meta.clone().filter(|(joined, _, _)| *joined == text);
        self.edit(|d, c, _| match meta {
            Some((_, parts, true)) if parts.len() == 1 => commands::paste_lines(d, c, &parts[0]),
            Some((_, parts, _)) if parts.len() == c.len() && parts.len() > 1 => commands::paste_each(d, c, &parts),
            _ => commands::insert_plain(d, c, &text),
        });
        self.close_popups();
        Task::none()
    }

    pub fn copy_selection(&mut self, cut: bool) -> Task<Message> {
        if self.focus == Focus::Terminal {
            let t = self.terms.get(self.active_term).and_then(|s| s.selection_text());
            return t.map_or_else(Task::none, cosmic::iced::clipboard::write);
        }
        if cut && !self.can_edit() {
            return Task::none();
        }
        let id = self.active_tab_id();
        let Some((t, e)) = self.active_mut() else { return Task::none() };
        let (parts, whole) = if cut { commands::cut(&mut e.doc, &mut t.cursors) } else { commands::copy_text(&e.doc, &t.cursors) };
        let joined = parts.join(if whole { "" } else { "\n" });
        if cut {
            self.edit_tab = id;
            self.want_reveal = true;
        }
        self.clip_meta = Some((joined.clone(), parts, whole));
        cosmic::iced::clipboard::write(joined)
    }

    /// After anything that moves carets.
    pub fn after_caret_move(&mut self) {
        let occ = self.settings.occurrences_highlight;
        let Some((t, e)) = self.active_mut() else { return };
        t.caret_stamp += 1;
        t.bracket_pair = commands::matching_bracket(&e.doc, t.cursors.head());
        t.occurrence = None;
        t.preedit.clear();
        if occ && !t.cursors.is_multi() {
            let p = t.cursors.primary();
            if p.is_empty() {
                let (a, b) = commands::word_at(&e.doc, p.head);
                if a != b {
                    t.occurrence = Some((e.doc.slice_string(a, b), true));
                }
            } else if p.end() - p.start() < 200 && e.doc.char_to_line(p.start()) == e.doc.char_to_line(p.end()) {
                let s = e.doc.slice_string(p.start(), p.end());
                if !s.trim().is_empty() {
                    t.occurrence = Some((s, false));
                }
            }
        }
        // A caret that wandered off where the snippet is stops the snippet.
        if let Some(s) = &t.snippet {
            let h = t.cursors.head();
            let near = s.current().iter().any(|r| r.0 <= h && h <= r.1);
            if !near {
                t.snippet = None;
            }
        }
        self.refilter_completion();
    }

    // Scrolling ----------------------------------------------------------------------------

    pub fn reveal_active(&mut self) {
        let Some(id) = self.active_tab_id() else { return };
        let (m, settings) = (self.metrics, self.settings.clone());
        let Some(t) = self.tabs.get_mut(&id) else { return };
        let Some(e) = self.docs.get(&t.doc) else { return };
        let geo = Geometry::new(e.doc.len_lines(), t.view_w, &m, &settings);
        t.map.update(&e.doc, &t.folds, t.map.wrap, e.doc.indent.width.max(1));
        let (row, v) = t.map.pos_to_row(&e.doc, t.cursors.head());
        let y = row as f32 * m.line_h;
        let margin = m.line_h * 2.0;
        if y < t.scroll_y + margin {
            t.scroll_y = (y - margin).max(0.0);
        } else if y + m.line_h > t.scroll_y + t.view_h - margin {
            t.scroll_y = y + m.line_h + margin - t.view_h;
        }
        if t.map.wrap.is_none() {
            let x = v as f32 * m.cell_w;
            let margin = m.cell_w * 6.0;
            if x < t.scroll_x + margin {
                t.scroll_x = (x - margin).max(0.0);
            } else if x > t.scroll_x + geo.text_w - margin {
                t.scroll_x = x - geo.text_w + margin;
            }
        }
    }

    pub fn center_active(&mut self) {
        let m = self.metrics;
        let Some((t, e)) = self.active_mut() else { return };
        t.map.update(&e.doc, &t.folds, t.map.wrap, e.doc.indent.width.max(1));
        let (row, _) = t.map.pos_to_row(&e.doc, t.cursors.head());
        t.scroll_y = (row as f32 * m.line_h - t.view_h / 2.0).max(0.0);
        t.scroll_x = 0.0;
    }

    // The mouse ----------------------------------------------------------------------------

    pub fn on_editor_event(&mut self, id: crate::state::TabId, ev: EditorEvent) -> Task<Message> {
        // A click in another split makes it the active one.
        if matches!(ev, EditorEvent::Press { .. } | EditorEvent::BreakpointToggle { .. } | EditorEvent::GutterSelect { .. } | EditorEvent::FoldToggle { .. } | EditorEvent::SecondaryPress { .. }) {
            if let Some(g) = self.groups.iter().position(|g| g.active_tab() == Some(id)) {
                self.active_group = g;
            }
            if self.focus != Focus::Editor {
                self.focus = Focus::Editor;
            }
            self.hover = None;
        }
        match ev {
            EditorEvent::Viewport { w, h, cell_w } => {
                if self.metrics.cell_w != cell_w {
                    self.metrics.cell_w = cell_w;
                }
                if let Some(t) = self.tabs.get_mut(&id) {
                    t.view_w = w;
                    t.view_h = h;
                }
                // Tabs not on screen get the same size, so they open at the right wrap width.
                Task::none()
            }
            EditorEvent::Scroll { dx, dy } => {
                if let Some(t) = self.tabs.get_mut(&id) {
                    t.scroll_y += dy;
                    t.scroll_x = (t.scroll_x + dx).max(0.0);
                }
                self.clamp_scroll(id);
                self.hover = None;
                Task::none()
            }
            EditorEvent::ScrollTo { y } => {
                if let Some(t) = self.tabs.get_mut(&id) {
                    t.scroll_y = y;
                }
                self.clamp_scroll(id);
                Task::none()
            }
            EditorEvent::Press { row, vcol, clicks, mods, button } => {
                let t = self.on_press(id, row, vcol, clicks, mods, button);
                self.vim_after_mouse();
                t
            }
            EditorEvent::Drag { row, vcol, mods } => {
                self.on_drag(id, row, vcol, mods);
                Task::none()
            }
            EditorEvent::Release => {
                if let Some(t) = self.tabs.get_mut(&id) {
                    t.column_anchor = None;
                    t.drag_origin = None;
                }
                self.vim_after_mouse();
                Task::none()
            }
            EditorEvent::FoldToggle { row } => {
                self.sync_active_structure();
                self.toggle_fold_at_row(id, row);
                Task::none()
            }
            EditorEvent::BreakpointToggle { row } => {
                let target = self.tabs.get(&id).and_then(|t| {
                    let e = self.docs.get(&t.doc)?;
                    Some((e.doc.path.clone(), t.map.row(&e.doc, row).line as usize))
                });
                match target {
                    Some((Some(path), line)) => {
                        self.toggle_breakpoint_at(&path, line);
                        Task::none()
                    }
                    Some((None, _)) => self.toast(crate::i18n::tr("Save the file first")),
                    None => Task::none(),
                }
            }
            EditorEvent::GutterSelect { row, extend } => {
                self.gutter_select(id, row, extend);
                Task::none()
            }
            EditorEvent::GutterDrag { row } => {
                self.gutter_drag(id, row);
                Task::none()
            }
            EditorEvent::StickyJump { line } => {
                self.jump_to(line, 0);
                Task::none()
            }
            EditorEvent::Hover { row, vcol } => self.request_hover(id, row, vcol),
            EditorEvent::HoverEnd => {
                self.hover = None;
                Task::none()
            }
            EditorEvent::ZoomWheel { delta } => {
                self.zoom(if delta > 0.0 { 1.0 } else { -1.0 });
                Task::none()
            }
            EditorEvent::Ime(text) => {
                if let Some(t) = self.tabs.get_mut(&id) {
                    t.preedit.clear();
                }
                self.type_text(&text)
            }
            EditorEvent::Preedit(text) => {
                if let Some(t) = self.tabs.get_mut(&id) {
                    t.preedit = text;
                }
                Task::none()
            }
            EditorEvent::SecondaryPress { row, vcol } => {
                // A right click outside the selection moves the caret there first.
                let Some(t) = self.tabs.get_mut(&id) else { return Task::none() };
                let Some(e) = self.docs.get(&t.doc) else { return Task::none() };
                let pos = t.map.row_to_pos(&e.doc, row, vcol);
                if !t.cursors.iter().any(|s| s.start() <= pos && pos <= s.end()) {
                    t.cursors.set_single(Selection::cursor(pos));
                    self.after_caret_move();
                }
                Task::none()
            }
        }
    }

    fn on_press(&mut self, id: crate::state::TabId, row: usize, vcol: usize, clicks: u8, mods: Modifiers, button: PressButton) -> Task<Message> {
        self.close_popups();
        let Some(t) = self.tabs.get_mut(&id) else { return Task::none() };
        let Some(e) = self.docs.get_mut(&t.doc) else { return Task::none() };
        e.doc.break_undo();
        t.expand_stack.clear();
        // Column select: middle button, or Shift+Alt with the left one.
        if button == PressButton::Middle || (mods.alt() && mods.shift()) {
            t.column_anchor = Some((row, vcol));
            let pos = t.map.row_to_pos(&e.doc, row, vcol);
            t.cursors.set_single(Selection::cursor(pos));
            self.after_caret_move();
            return Task::none();
        }
        let pos = t.map.row_to_pos(&e.doc, row, vcol);
        // Ctrl+click follows a definition.
        if mods.control() && !mods.shift() && clicks == 1 {
            t.cursors.set_single(Selection::cursor(pos));
            self.after_caret_move();
            return self.go_to_location("definition");
        }
        // Alt+click adds a cursor, or takes one away.
        let add = mods.alt() && !mods.shift();
        let (a, b) = match clicks {
            2 => commands::word_or_run_at(&e.doc, pos),
            3 => {
                let line = e.doc.char_to_line(pos);
                let end = if line + 1 < e.doc.len_lines() { e.doc.line_start(line + 1) } else { e.doc.len_chars() };
                (e.doc.line_start(line), end)
            }
            _ => (pos, pos),
        };
        if add && clicks == 1 && t.cursors.remove_at(pos) {
            self.after_caret_move();
            return Task::none();
        }
        let sel = if mods.shift() && !add {
            Selection::new(t.cursors.primary().anchor, b)
        } else {
            Selection::new(a, b)
        };
        if add {
            t.cursors.add(sel);
        } else {
            t.cursors.set_single(sel);
        }
        t.drag_origin = Some(crate::state::DragOrigin { start: a, end: b, clicks, added_cursor: add });
        self.after_caret_move();
        Task::none()
    }

    fn on_drag(&mut self, id: crate::state::TabId, row: usize, vcol: usize, mods: Modifiers) {
        let Some(t) = self.tabs.get_mut(&id) else { return };
        let Some(e) = self.docs.get(&t.doc) else { return };
        if mods.alt() && mods.shift() && t.column_anchor.is_none() {
            let (r, v) = t.map.pos_to_row(&e.doc, t.cursors.primary().anchor);
            t.column_anchor = Some((r, v));
        }
        if let Some(anchor) = t.column_anchor {
            t.cursors = commands::column_select(&e.doc, &t.map, anchor, (row, vcol));
            t.caret_stamp += 1;
            return;
        }
        let pos = t.map.row_to_pos(&e.doc, row, vcol);
        let Some(origin) = t.drag_origin else { return };
        let (s, h) = match origin.clicks {
            2 | 3 => {
                let (a, b) = if origin.clicks == 2 {
                    commands::word_or_run_at(&e.doc, pos)
                } else {
                    let line = e.doc.char_to_line(pos);
                    let end = if line + 1 < e.doc.len_lines() { e.doc.line_start(line + 1) } else { e.doc.len_chars() };
                    (e.doc.line_start(line), end)
                };
                if pos < origin.start { (origin.end, a) } else { (origin.start, b.max(origin.end)) }
            }
            _ => {
                let anchor = if mods.shift() { t.cursors.primary().anchor } else { origin.start };
                (anchor, pos)
            }
        };
        let sel = Selection::new(s, h);
        if origin.added_cursor {
            // Replace only the newest selection.
            let keep = t.cursors.primary_index();
            let mut v: Vec<Selection> = t.cursors.as_slice().to_vec();
            if let Some(x) = v.get_mut(keep) {
                *x = sel;
            }
            t.cursors = Cursors::from_vec(v, keep);
        } else {
            t.cursors.set_single(sel);
        }
        t.caret_stamp += 1;
        self.after_caret_move();
    }

    fn toggle_fold_at_row(&mut self, id: crate::state::TabId, row: usize) {
        let Some(t) = self.tabs.get_mut(&id) else { return };
        let Some(e) = self.docs.get(&t.doc) else { return };
        let line = t.map.row(&e.doc, row).line as usize;
        if !t.folds.remove(line)
            && let Some(end) = App::fold_line_range(&e.doc, line)
        {
            t.folds.add(line, end);
            // A caret in the folded lines moves to the header.
            let header_end = e.doc.line_end(line);
            let hidden_end = e.doc.line_end(end);
            t.cursors.map(|s| {
                if s.head > header_end && s.head <= hidden_end { Selection::cursor(header_end) } else { s }
            });
        }
    }

    fn gutter_select(&mut self, id: crate::state::TabId, row: usize, extend: bool) {
        let Some(t) = self.tabs.get_mut(&id) else { return };
        let Some(e) = self.docs.get(&t.doc) else { return };
        let line = t.map.row(&e.doc, row).line as usize;
        let start = e.doc.line_start(line);
        let end = if line + 1 < e.doc.len_lines() { e.doc.line_start(line + 1) } else { e.doc.len_chars() };
        let sel = if extend { Selection::new(t.cursors.primary().anchor, end) } else { Selection::new(start, end) };
        t.cursors.set_single(sel);
        t.drag_origin = Some(crate::state::DragOrigin { start, end, clicks: 3, added_cursor: false });
        self.after_caret_move();
    }

    fn gutter_drag(&mut self, id: crate::state::TabId, row: usize) {
        let Some(t) = self.tabs.get_mut(&id) else { return };
        let Some(e) = self.docs.get(&t.doc) else { return };
        let Some(origin) = t.drag_origin else { return };
        let line = t.map.row(&e.doc, row).line as usize;
        let start = e.doc.line_start(line);
        let end = if line + 1 < e.doc.len_lines() { e.doc.line_start(line + 1) } else { e.doc.len_chars() };
        let sel = if start < origin.start { Selection::new(origin.end, start) } else { Selection::new(origin.start, end) };
        t.cursors.set_single(sel);
        t.caret_stamp += 1;
        self.after_caret_move();
    }

    pub fn zoom(&mut self, delta: f32) {
        self.settings.font_size = (self.settings.font_size + delta).clamp(6.0, 72.0);
        self.apply_font();
    }

    pub fn apply_font(&mut self) {
        self.metrics = crate::state::Metrics::new(&self.editor_font_family(), self.settings.font_size, self.settings.line_height);
    }

    // Find ---------------------------------------------------------------------------------

    pub fn open_find(&mut self, replace: bool) -> Task<Message> {
        self.find.open = true;
        if replace {
            self.find.replace_open = true;
        }
        // A short selection on one line seeds the query.
        if let Some((t, e)) = self.active() {
            let p = t.cursors.primary();
            if !p.is_empty() && e.doc.char_to_line(p.start()) == e.doc.char_to_line(p.end()) {
                self.find.query.pattern = e.doc.slice_string(p.start(), p.end());
            }
        }
        self.find.generation += 1;
        self.focus = Focus::Input;
        let id = cosmic::widget::Id::new(FIND_INPUT);
        Task::batch([cosmic::widget::text_input::focus(id.clone()), cosmic::widget::text_input::select_all(id)])
    }

    pub fn close_find(&mut self) {
        self.find.open = false;
        self.find.in_selection = None;
        self.find.current = None;
        self.focus = Focus::Editor;
        if let Some((_, e)) = self.active_mut() {
            e.matches.clear();
            e.matches_key = None;
        }
    }

    /// Recomputes matches when the query or the text changed.
    pub fn refresh_find_matches(&mut self) {
        // Vim's search lights up its matches without the find bar.
        let vim_q = self.vim_highlight.clone().filter(|_| !self.find.open);
        if !self.find.open && vim_q.is_none() {
            return;
        }
        let gen_id = self.find.generation;
        let q = match vim_q {
            Some(pattern) => find::Query { pattern, regex: true, case_sensitive: true, whole_word: false },
            None => self.find.query.clone(),
        };
        let sel_ranges = self.find.in_selection.clone();
        let Some(tab_id) = self.active_tab_id() else { return };
        let Some(t) = self.tabs.get(&tab_id) else { return };
        let doc_id = t.doc;
        let Some(e) = self.docs.get_mut(&doc_id) else { return };
        let key = (e.doc.revision, gen_id);
        if e.matches_key == Some(key) {
            return;
        }
        e.matches_key = Some(key);
        e.matches.clear();
        self.find.error = None;
        if q.pattern.is_empty() {
            self.find.current = None;
            return;
        }
        match q.compile() {
            Ok(re) => {
                let mut m = find::find_all(&e.doc, &re, MAX_SHOWN_MATCHES);
                if let Some(ranges) = &sel_ranges {
                    m.retain(|x| ranges.iter().any(|r| r.0 <= x.0 && x.1 <= r.1));
                }
                e.matches = m;
            }
            Err(err) => self.find.error = Some(err),
        }
        self.update_current_match();
    }

    fn update_current_match(&mut self) {
        let Some((t, e)) = self.active() else { return };
        let pos = t.cursors.primary().start();
        let c = find::next_match(&e.matches, pos);
        self.find.current = c;
    }

    fn select_match(&mut self, i: usize) {
        let Some((t, e)) = self.active_mut() else { return };
        let Some(&(a, b)) = e.matches.get(i) else { return };
        let l = e.doc.char_to_line(a);
        t.folds.reveal(l);
        t.cursors.set_single(Selection::new(a, b));
        self.find.current = Some(i);
        self.want_reveal = true;
        self.after_caret_move();
    }

    pub fn find_step(&mut self, forward: bool) {
        if !self.find.open {
            // F3 with the bar closed still searches for the last query.
            self.find.open = !self.find.query.pattern.is_empty();
            self.find.generation += 1;
        }
        self.refresh_find_matches();
        let Some((t, e)) = self.active() else { return };
        let p = t.cursors.primary();
        let i = if forward { find::next_match(&e.matches, p.end()) } else { find::prev_match(&e.matches, p.start()) };
        if let Some(i) = i {
            self.select_match(i);
        }
    }

    pub fn on_find(&mut self, m: FindMsg) -> Task<Message> {
        match m {
            FindMsg::Query(s) => {
                self.find.query.pattern = s;
                self.find.generation += 1;
                self.refresh_find_matches();
                // Show the first result without moving focus out of the field.
                if let Some(i) = self.find.current {
                    self.select_match(i);
                }
            }
            FindMsg::Replacement(s) => self.find.replacement = s,
            FindMsg::ToggleCase => {
                self.find.query.case_sensitive = !self.find.query.case_sensitive;
                self.find.generation += 1;
            }
            FindMsg::ToggleWord => {
                self.find.query.whole_word = !self.find.query.whole_word;
                self.find.generation += 1;
            }
            FindMsg::ToggleRegex => {
                self.find.query.regex = !self.find.query.regex;
                self.find.generation += 1;
            }
            FindMsg::ToggleInSelection => {
                if self.find.in_selection.is_some() {
                    self.find.in_selection = None;
                } else if let Some((t, _)) = self.active() {
                    let r: Vec<_> = t.cursors.iter().filter(|s| !s.is_empty()).map(|s| (s.start(), s.end())).collect();
                    self.find.in_selection = (!r.is_empty()).then_some(r);
                }
                self.find.generation += 1;
            }
            FindMsg::ToggleReplace => self.find.replace_open = !self.find.replace_open,
            FindMsg::Next => self.find_step(true),
            FindMsg::Prev => self.find_step(false),
            FindMsg::ReplaceOne => self.replace_one(),
            FindMsg::ReplaceAll => return self.replace_all(),
            FindMsg::Close => self.close_find(),
        }
        Task::none()
    }

    fn replace_one(&mut self) {
        self.refresh_find_matches();
        let q = self.find.query.clone();
        let Ok(re) = q.compile() else { return };
        let replacement = self.find.replacement.clone();
        let Some((t, e)) = self.active() else { return };
        let p = t.cursors.primary();
        // The selection is a match: replace it. Otherwise go to the next one first.
        let is_match = e.matches.binary_search(&(p.start(), p.end())).is_ok();
        if !is_match {
            self.find_step(true);
            return;
        }
        let (a, b) = (p.start(), p.end());
        let out = find::expand(&re, &q, &e.doc.slice_string(a, b), &replacement);
        self.edit(|d, c, _| {
            d.apply(vec![tinta_core::Edit::new(a, b, out.clone())], c, tinta_core::EditKind::Other, |m, c| {
                c.set_single(Selection::cursor(m.after_edit(0)));
            });
        });
        // The next step needs the new list right away, not after this message.
        self.find.generation += 1;
        self.refresh_find_matches();
        self.find_step(true);
    }

    fn replace_all(&mut self) -> Task<Message> {
        self.refresh_find_matches();
        let q = self.find.query.clone();
        let Ok(re) = q.compile() else { return Task::none() };
        let replacement = self.find.replacement.clone();
        let ranges = self.find.in_selection.clone();
        let Some((_, e)) = self.active() else { return Task::none() };
        // The highlighted list may be cut short; this one never is.
        let mut all = find::find_all(&e.doc, &re, usize::MAX);
        if let Some(r) = &ranges {
            all.retain(|x| r.iter().any(|s| s.0 <= x.0 && x.1 <= s.1));
        }
        let n = all.len();
        if n == 0 {
            return Task::none();
        }
        let edits: Vec<tinta_core::Edit> = all
            .iter()
            .map(|&(a, b)| tinta_core::Edit::new(a, b, find::expand(&re, &q, &e.doc.slice_string(a, b), &replacement)))
            .collect();
        self.edit(|d, c, _| {
            d.apply(edits, c, tinta_core::EditKind::Other, |m, c| {
                c.map(|s| Selection::cursor(m.map(s.head, tinta_core::buffer::Assoc::After)));
            });
        });
        self.find.generation += 1;
        self.toast(format!("{n} {}", crate::i18n::tr("replacements")))
    }
}
