//! What each command does.

use super::{App, BoolSetting, DialogMsg, DialogState, Focus, Message, PaletteMode, PanelView, SettingMsg, SidebarView};
use crate::state::{TabId, TabKind};
use crate::fx::{self, Task};
use std::path::PathBuf;
use tinta_core::buffer::LineEnding;
use tinta_core::commands::{self as cmds, LineOp};
use tinta_core::keymap::{Command, Keymap};
use tinta_core::settings::{self, LineNumbers, Settings, Whitespace};
use tinta_core::{Selection, display, git as core_git};

impl App {
    pub fn run(&mut self, c: Command) -> Task {
        use Command as C;
        match c {
            C::NewFile => return self.new_scratch(),
            C::OpenFile => return open_file_dialog(self.folder.clone()),
            C::OpenFolder => return open_folder_dialog(),
            C::CloseFolder => return self.close_folder(),
            C::Save => return self.save_active(),
            C::SaveAs => {
                return match self.active_tab_id() {
                    Some(id) => self.save_as_dialog(id),
                    None => Task::none(),
                };
            }
            C::SaveAll => return self.save_all_dirty_named(),
            C::Revert => {
                if let Some((t, _)) = self.active() {
                    let d = t.doc;
                    return self.reload_doc(d);
                }
            }
            C::CloseTab => {
                if let Some(id) = self.active_tab_id() {
                    return self.request_close_tab(id);
                }
            }
            C::CloseOtherTabs => {
                let a = self.active_tab_id();
                let g = self.groups[self.active_group].tabs();
                return self.close_tabs_where(move |id, _| Some(id) == a || !g.contains(&id));
            }
            C::CloseAllTabs => return self.close_tabs_where(|_, _| false),
            C::ReopenTab => {
                if let Some(p) = self.closed_tabs.pop() {
                    return self.open_path(&p);
                }
            }
            C::PinTab => {
                if let Some((t, _)) = self.active_mut() {
                    t.pinned = !t.pinned;
                }
                self.refresh_tab_titles();
            }
            C::NextTab => return self.cycle_tab(1),
            C::PrevTab => return self.cycle_tab(-1),
            C::MoveTabLeft => self.move_tab(-1),
            C::MoveTabRight => self.move_tab(1),
            C::Quit => return self.request_quit(),
            C::Undo => {
                let Some(id) = self.active_tab_id() else { return Task::none() };
                if self.can_edit() {
                    let Some((t, e)) = self.active_mut() else { return Task::none() };
                    e.doc.undo(&mut t.cursors);
                    self.edit_tab = Some(id);
                    self.want_reveal = true;
                }
            }
            C::Redo => {
                let Some(id) = self.active_tab_id() else { return Task::none() };
                if self.can_edit() {
                    let Some((t, e)) = self.active_mut() else { return Task::none() };
                    e.doc.redo(&mut t.cursors);
                    self.edit_tab = Some(id);
                    self.want_reveal = true;
                }
            }
            C::Cut => return self.copy_selection(true),
            C::Copy => return self.copy_selection(false),
            C::Paste => return fx::clipboard_read(Message::Pasted),
            C::DeleteLine => self.edit(|d, c, _| cmds::delete_lines(d, c)),
            C::DuplicateSelection => self.edit(|d, c, _| cmds::duplicate(d, c)),
            C::CopyLineDown => self.edit(|d, c, _| cmds::copy_lines(d, c, true)),
            C::CopyLineUp => self.edit(|d, c, _| cmds::copy_lines(d, c, false)),
            C::MoveLineUp => self.edit(|d, c, _| cmds::move_lines(d, c, false)),
            C::MoveLineDown => self.edit(|d, c, _| cmds::move_lines(d, c, true)),
            C::InsertLineBelow => self.edit(|d, c, _| cmds::insert_line_below(d, c)),
            C::InsertLineAbove => self.edit(|d, c, _| cmds::insert_line_above(d, c)),
            C::JoinLines => self.edit(|d, c, _| cmds::join_lines(d, c)),
            C::IndentLines => self.edit(|d, c, _| cmds::indent_lines(d, c)),
            C::OutdentLines => self.edit(|d, c, _| cmds::outdent_lines(d, c)),
            C::SortAsc => self.edit(|d, c, _| cmds::line_op(d, c, LineOp::SortAsc)),
            C::SortDesc => self.edit(|d, c, _| cmds::line_op(d, c, LineOp::SortDesc)),
            C::SortCaseless => self.edit(|d, c, _| cmds::line_op(d, c, LineOp::SortCaseless)),
            C::ReverseLines => self.edit(|d, c, _| cmds::line_op(d, c, LineOp::Reverse)),
            C::UniqueLines => self.edit(|d, c, _| cmds::line_op(d, c, LineOp::Unique)),
            C::ShuffleLines => self.edit(|d, c, _| cmds::line_op(d, c, LineOp::Shuffle)),
            C::TrimTrailing => self.edit(|d, c, _| cmds::trim_trailing(d, c)),
            C::ToggleComment => self.edit(|d, c, _| cmds::toggle_line_comment(d, c)),
            C::ToggleBlockComment => self.edit(|d, c, _| cmds::toggle_block_comment(d, c)),
            C::UpperCase => self.edit(|d, c, _| cmds::transform_selections(d, c, cmds::to_upper)),
            C::LowerCase => self.edit(|d, c, _| cmds::transform_selections(d, c, cmds::to_lower)),
            C::TitleCase => self.edit(|d, c, _| cmds::transform_selections(d, c, cmds::to_title)),
            C::SnakeCase => self.edit(|d, c, _| cmds::transform_selections(d, c, cmds::to_snake)),
            C::CamelCase => self.edit(|d, c, _| cmds::transform_selections(d, c, cmds::to_camel)),
            C::PascalCase => self.edit(|d, c, _| cmds::transform_selections(d, c, cmds::to_pascal)),
            C::KebabCase => self.edit(|d, c, _| cmds::transform_selections(d, c, cmds::to_kebab)),
            C::Transpose => self.edit(|d, c, _| cmds::transpose(d, c)),
            C::DeleteWordLeft => self.edit(|d, c, _| cmds::delete_word(d, c, false)),
            C::DeleteWordRight => self.edit(|d, c, _| cmds::delete_word(d, c, true)),
            C::DeleteToLineStart => self.edit(|d, c, _| cmds::delete_to_line_edge(d, c, false)),
            C::DeleteToLineEnd => self.edit(|d, c, _| cmds::delete_to_line_edge(d, c, true)),
            C::SelectAll => {
                if self.focus == Focus::Editor
                    && let Some((t, e)) = self.active_mut()
                {
                    cmds::select_all(&e.doc, &mut t.cursors);
                    self.after_caret_move();
                }
            }
            C::SelectLine => self.select_op(|d, c, _| cmds::select_line(d, c)),
            C::SelectWord => self.select_op(|d, c, _| cmds::select_word(d, c)),
            C::AddNextOccurrence => self.select_op(|d, c, _| {
                cmds::add_next_occurrence(d, c);
            }),
            C::SkipOccurrence => self.select_op(|d, c, _| {
                cmds::skip_occurrence(d, c);
            }),
            C::SelectAllOccurrences => self.select_op(|d, c, _| {
                cmds::select_all_occurrences(d, c);
            }),
            C::SplitIntoLines => self.select_op(|d, c, _| cmds::split_into_lines(d, c)),
            C::AddCursorAbove => self.select_op(|d, c, m| cmds::add_cursor_vertical(d, c, m, false)),
            C::AddCursorBelow => self.select_op(|d, c, m| cmds::add_cursor_vertical(d, c, m, true)),
            C::ExpandSelection => {
                self.sync_active_structure();
                if let Some((t, e)) = self.active_mut() {
                    t.expand_stack.push(t.cursors.clone());
                    t.cursors.map(|s| cmds::expand_selection(&e.doc, s));
                    let stack = std::mem::take(&mut t.expand_stack);
                    self.want_reveal = true;
                    self.after_caret_move();
                    if let Some((t, _)) = self.active_mut() {
                        t.expand_stack = stack;
                    }
                }
            }
            C::ShrinkSelection => {
                if let Some((t, _)) = self.active_mut()
                    && let Some(c) = t.expand_stack.pop()
                {
                    t.cursors = c;
                    self.after_caret_move();
                }
            }
            C::Find => return self.open_find(false),
            C::Replace => return self.open_find(true),
            C::FindNext => self.find_step(true),
            C::FindPrev => self.find_step(false),
            C::FindSelection => {
                if let Some((t, e)) = self.active() {
                    let p = t.cursors.primary();
                    let (a, b) = if p.is_empty() { cmds::word_at(&e.doc, p.head) } else { (p.start(), p.end()) };
                    let s = e.doc.slice_string(a, b);
                    self.find.query.pattern = s;
                    self.find.open = true;
                    self.find.generation += 1;
                }
            }
            C::ReplaceAll => return self.on_find(super::FindMsg::ReplaceAll),
            C::FindInFiles => return self.open_search(false),
            C::ReplaceInFiles => return self.open_search(true),
            C::CommandPalette => return self.open_palette(PaletteMode::Commands),
            C::QuickOpen => return self.open_palette(PaletteMode::Files),
            C::GoToLine => return self.open_palette(PaletteMode::Line),
            C::GoToSymbol => {
                self.request_symbols();
                return self.open_palette(PaletteMode::Symbols);
            }
            C::ChangeLanguage => return self.open_palette(PaletteMode::Language),
            C::ChangeTheme => return self.open_palette(PaletteMode::Theme),
            C::OpenRecent => return self.open_palette(PaletteMode::Recent),
            C::GoBack => return self.nav_step(true),
            C::GoForward => return self.nav_step(false),
            C::JumpToBracket => self.select_op(|d, c, _| cmds::jump_to_bracket(d, c, false)),
            C::SelectToBracket => self.select_op(|d, c, _| cmds::jump_to_bracket(d, c, true)),
            C::ToggleBookmark => {
                if let Some((t, e)) = self.active_mut() {
                    let l = e.doc.char_to_line(t.cursors.head());
                    if !e.bookmarks.remove(&l) {
                        e.bookmarks.insert(l);
                    }
                }
            }
            C::NextBookmark => self.bookmark_step(true),
            C::PrevBookmark => self.bookmark_step(false),
            C::ClearBookmarks => {
                if let Some((_, e)) = self.active_mut() {
                    e.bookmarks.clear();
                }
            }
            C::NextChange => self.change_step(true),
            C::PrevChange => self.change_step(false),
            C::RevertHunk => self.revert_hunk(),
            C::Fold => {
                self.sync_active_structure();
                self.fold_at_cursor();
            }
            C::Unfold => self.unfold_at_cursor(),
            C::FoldAll => {
                self.sync_active_structure();
                self.fold_all();
            }
            C::UnfoldAll => {
                if let Some((t, _)) = self.active_mut() {
                    t.folds.clear();
                }
            }
            C::TriggerCompletion => return self.trigger_completion(None),
            C::ParameterHints => return self.request_signature(),
            C::ShowHover => return self.hover_at_caret(),
            C::GoToDefinition => return self.go_to_location("definition"),
            C::GoToDeclaration => return self.go_to_location("declaration"),
            C::GoToImplementation => return self.go_to_location("implementation"),
            C::GoToTypeDefinition => return self.go_to_location("typeDefinition"),
            C::FindReferences => return self.find_references(),
            C::Rename => return self.start_rename(),
            C::FormatDocument => return self.format_document(),
            C::CodeAction => return self.request_code_actions(),
            C::RestartLsp => return self.restart_lsp(),
            C::NextDiagnostic => self.diag_step(true),
            C::PrevDiagnostic => self.diag_step(false),
            C::ToggleSidebar => {
                self.sidebar = match self.sidebar {
                    Some(_) => None,
                    None => Some(self.last_sidebar),
                };
                self.mark_session_dirty();
            }
            // The tree takes the keyboard, as it does in other editors.
            C::ShowExplorer => {
                let t = self.show_sidebar(SidebarView::Explorer);
                if self.folder.is_some() {
                    self.focus_explorer();
                }
                return t;
            }
            C::ShowSearch => return self.open_search(false),
            C::ShowGit => return self.show_sidebar(SidebarView::Git),
            C::ShowOutline => return self.show_sidebar(SidebarView::Outline),
            C::TogglePanel => {
                if self.panel.is_some() {
                    self.panel = None;
                    self.focus = Focus::Editor;
                } else {
                    self.panel = Some(PanelView::Terminal);
                    return self.ensure_terminal();
                }
            }
            C::ToggleTerminal => {
                if self.panel == Some(PanelView::Terminal) && self.focus == Focus::Terminal {
                    self.panel = None;
                    self.focus = Focus::Editor;
                } else {
                    self.panel = Some(PanelView::Terminal);
                    return self.ensure_terminal();
                }
            }
            C::NewTerminal => return self.new_terminal(),
            C::ShowProblems => self.panel = Some(PanelView::Problems),
            C::SplitRight => {
                self.split_group();
                return self.on_active_changed();
            }
            C::FocusNextGroup => return self.focus_group(1),
            C::FocusPrevGroup => return self.focus_group(-1),
            C::CloseGroup => {
                let ids = self.groups[self.active_group].tabs();
                let mut t = Vec::new();
                for id in ids {
                    t.push(self.request_close_tab(id));
                }
                return Task::batch(t);
            }
            C::DebugStart => return self.debug_start(false),
            C::DebugStop => self.debug_stop(),
            C::DebugRestart => return self.debug_restart(),
            C::DebugPause => {
                if let Some(s) = &mut self.debug.session {
                    s.pause();
                }
            }
            C::StepOver => return self.debug_step(|s| s.step_over()),
            C::StepInto => return self.debug_step(|s| s.step_in()),
            C::StepOut => {
                if let Some(s) = &mut self.debug.session {
                    s.step_out();
                }
            }
            C::ToggleBreakpoint => return self.toggle_breakpoint_here(),
            C::BreakpointCondition => return self.start_breakpoint_condition(),
            C::RemoveBreakpoints => {
                let paths: Vec<_> = self.debug.breakpoints.keys().cloned().collect();
                self.debug.breakpoints.clear();
                if let Some(s) = self.debug.session.as_mut().filter(|s| s.is_live()) {
                    for p in paths {
                        s.send_breakpoints(&p, &[]);
                    }
                }
            }
            C::ShowDebug => return self.show_sidebar(SidebarView::Debug),
            C::ShowDebugConsole => self.panel = Some(PanelView::Debug),
            C::DebugConfigure => return self.debug_configure(),
            C::DebugSelectConfig => return self.debug_pick_config(),
            C::ToggleVim => {
                self.settings.vim_mode = !self.settings.vim_mode;
                self.vim = tinta_core::vim::Vim::new();
                self.vim_reset();
                self.persist_settings();
            }
            C::FoldToggle => {
                self.sync_active_structure();
                let folded = self.active().is_some_and(|(t, e)| t.folds.is_folded(e.doc.char_to_line(t.cursors.head())));
                if folded {
                    self.unfold_at_cursor();
                } else {
                    self.fold_at_cursor();
                }
            }
            C::ToggleWordWrap => {
                self.settings.word_wrap = !self.settings.word_wrap;
                self.persist_settings();
            }
            C::ToggleMinimap => {
                self.settings.minimap = !self.settings.minimap;
                self.persist_settings();
            }
            C::ToggleWhitespace => {
                self.settings.render_whitespace = match self.settings.render_whitespace {
                    Whitespace::All => Whitespace::Selection,
                    _ => Whitespace::All,
                };
                self.persist_settings();
            }
            C::ToggleLineNumbers => {
                self.settings.line_numbers = match self.settings.line_numbers {
                    LineNumbers::On => LineNumbers::Relative,
                    LineNumbers::Relative => LineNumbers::Off,
                    LineNumbers::Off => LineNumbers::On,
                };
                self.persist_settings();
            }
            C::ToggleStickyScroll => {
                self.settings.sticky_scroll = !self.settings.sticky_scroll;
                self.persist_settings();
            }
            C::ToggleBlame => {
                self.settings.inline_blame = !self.settings.inline_blame;
                self.persist_settings();
            }
            C::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                return self.set_fullscreen(self.fullscreen);
            }
            C::ToggleZen => {
                self.zen = !self.zen;
                return self.set_fullscreen(self.zen);
            }
            C::ZoomIn => self.zoom(1.0),
            C::ZoomOut => self.zoom(-1.0),
            C::ZoomReset => {
                self.settings.font_size = self.base_font_size;
                self.apply_font();
            }
            C::ScrollUp | C::ScrollDown => {
                let d = if c == C::ScrollUp { -self.metrics.line_h } else { self.metrics.line_h };
                if let Some(id) = self.active_tab_id() {
                    if let Some(t) = self.tabs.get_mut(&id) {
                        t.scroll_y += d;
                    }
                    self.clamp_scroll(id);
                }
            }
            C::CenterCursor => self.center_active(),
            C::MarkdownPreview => self.open_preview(),
            C::GitRefresh => {
                self.refresh_git_panel();
                self.refresh_git_for_active();
            }
            C::GitCommit => return self.show_sidebar(SidebarView::Git),
            C::GitPush => return self.git_remote("push"),
            C::GitPull => return self.git_remote("pull"),
            C::GitFetch => return self.git_remote("fetch"),
            C::GitStageAll => {
                if let Some(r) = self.git.root.clone() {
                    let _ = core_git::stage_all(&r);
                    self.refresh_git_panel();
                }
            }
            C::GitLog => {
                if let Some(r) = self.git.root.clone() {
                    match core_git::log(&r, 300) {
                        Ok(text) => return self.open_generated("git log".into(), &text, "Plain Text"),
                        Err(e) => return self.toast(e),
                    }
                }
            }
            C::GitCheckout => return self.open_palette(PaletteMode::Branch),
            C::GitDiffFile => return self.show_file_diff(),
            C::CompareSaved => return self.compare_with_saved(),
            C::RunBuild => return self.run_default_build(),
            C::RunTask => return self.run_task_picker(),
            C::RerunTask => return self.rerun_task(),
            C::StopTask => self.stop_task(),
            C::ShowTasks => self.panel = Some(PanelView::Tasks),
            C::NextTaskProblem => return self.task_problem_step(true),
            C::PrevTaskProblem => return self.task_problem_step(false),
            C::SetLf => self.set_eol(LineEnding::Lf),
            C::SetCrlf => self.set_eol(LineEnding::CrLf),
            C::IndentSpaces => self.set_indent_kind(true),
            C::IndentTabs => self.set_indent_kind(false),
            C::ChangeIndentSize => return self.open_palette(PaletteMode::Indent),
            C::ChangeEncoding => return self.open_palette(PaletteMode::Encoding),
            C::InsertSnippet => return self.open_palette(PaletteMode::Snippet),
            C::OpenFolderInFiles => {
                let p = self.active().and_then(|(_, e)| e.doc.path.clone()).or(self.folder.clone());
                if let Some(p) = p {
                    let dir = if p.is_dir() { p } else { p.parent().map(PathBuf::from).unwrap_or(p) };
                    let _ = open::that_detached(dir);
                }
            }
            C::CopyPath => {
                if let Some(p) = self.active().and_then(|(_, e)| e.doc.path.clone()) {
                    return fx::clipboard_write(p.display().to_string());
                }
            }
            C::CopyRelativePath => {
                if let Some(p) = self.active().and_then(|(_, e)| e.doc.path.clone()) {
                    let rel = self.folder.as_ref().and_then(|f| p.strip_prefix(f).ok()).unwrap_or(&p);
                    return fx::clipboard_write(rel.display().to_string());
                }
            }
            C::OpenSettings => {
                self.settings_open = !self.settings_open;
            }
            C::OpenSettingsJson => return self.open_config(settings::settings_path(), "settings"),
            C::OpenKeybindings => return self.open_config(settings::keybindings_path(), "keys"),
            C::OpenSnippets => return self.open_config(settings::snippets_path(), "snippets"),
            C::ShowShortcuts => {
                self.shortcuts_filter.clear();
                self.dialog = Some(DialogState::Shortcuts);
            }
            C::About => self.dialog = Some(DialogState::About),
        }
        Task::none()
    }

    fn set_fullscreen(&self, on: bool) -> Task {
        Task::effect(fx::Effect::Fullscreen(on))
    }

    fn select_op(&mut self, f: impl FnOnce(&tinta_core::Document, &mut tinta_core::Cursors, &tinta_core::DisplayMap)) {
        let Some((t, e)) = self.active_mut() else { return };
        f(&e.doc, &mut t.cursors, &t.map);
        e.doc.break_undo();
        self.want_reveal = true;
        self.after_caret_move();
    }

    fn cycle_tab(&mut self, d: i32) -> Task {
        let g = &mut self.groups[self.active_group];
        let n = g.model.len() as i32;
        if n < 2 {
            return Task::none();
        }
        let Some(pos) = g.model.position(g.model.active()) else { return Task::none() };
        let next = (pos as i32 + d).rem_euclid(n) as u16;
        g.model.activate_position(next);
        self.on_active_changed()
    }

    fn move_tab(&mut self, d: i32) {
        let g = &mut self.groups[self.active_group];
        let n = g.model.len() as i32;
        let e = g.model.active();
        let Some(pos) = g.model.position(e) else { return };
        let target = pos as i32 + d;
        if target >= 0 && target < n {
            g.model.position_set(e, target as u16);
        }
        self.mark_session_dirty();
    }

    fn focus_group(&mut self, d: i32) -> Task {
        let n = self.groups.len() as i32;
        self.active_group = (self.active_group as i32 + d).rem_euclid(n) as usize;
        self.focus = Focus::Editor;
        self.on_active_changed()
    }

    pub fn push_nav(&mut self) {
        if let Some((t, e)) = self.active()
            && let Some(p) = &e.doc.path
        {
            let entry = (p.clone(), t.cursors.head());
            if self.nav_back.last() != Some(&entry) {
                self.nav_back.push(entry);
                self.nav_forward.clear();
            }
            if self.nav_back.len() > 100 {
                self.nav_back.remove(0);
            }
        }
    }

    fn nav_step(&mut self, back: bool) -> Task {
        let current = self.active().and_then(|(t, e)| e.doc.path.clone().map(|p| (p, t.cursors.head())));
        let target = if back { self.nav_back.pop() } else { self.nav_forward.pop() };
        let Some((path, pos)) = target else { return Task::none() };
        if let Some(c) = current {
            if back {
                self.nav_forward.push(c);
            } else {
                self.nav_back.push(c);
            }
        }
        let t = self.open_path(&path);
        if let Some((tab, e)) = self.active_mut() {
            let p = pos.min(e.doc.len_chars());
            tab.cursors.set_single(Selection::cursor(p));
        }
        self.center_active();
        self.after_caret_move();
        t
    }

    fn bookmark_step(&mut self, forward: bool) {
        let Some((t, e)) = self.active() else { return };
        let l = e.doc.char_to_line(t.cursors.head());
        let next = if forward {
            e.bookmarks.range(l + 1..).next().or(e.bookmarks.iter().next()).copied()
        } else {
            e.bookmarks.range(..l).next_back().or(e.bookmarks.iter().next_back()).copied()
        };
        if let Some(n) = next {
            self.jump_to(n, 0);
        }
    }

    fn change_step(&mut self, forward: bool) {
        if self.diff_step(forward) {
            return;
        }
        let Some((t, e)) = self.active() else { return };
        let l = e.doc.char_to_line(t.cursors.head());
        let h = if forward { e.git_diff.next_hunk(l) } else { e.git_diff.prev_hunk(l) };
        if let Some(line) = h.map(|h| h.new.start) {
            self.jump_to(line, 0);
        }
    }

    fn revert_hunk(&mut self) {
        let Some((t, e)) = self.active() else { return };
        let l = e.doc.char_to_line(t.cursors.head());
        let Some(hunk) = e.git_diff.hunk_at(l).cloned() else { return };
        let Some(base) = e.git_base.clone() else { return };
        let text = core_git::hunk_base_text(&base, &hunk);
        let start = e.doc.pos(hunk.new.start, 0);
        let end = if hunk.new.end < e.doc.len_lines() { e.doc.line_start(hunk.new.end) } else { e.doc.len_chars() };
        self.edit(|d, c, _| {
            d.apply(vec![tinta_core::Edit::new(start, end, text)], c, tinta_core::EditKind::Other, |m, c| {
                c.map(|s| Selection::cursor(m.map(s.head, tinta_core::buffer::Assoc::Before)));
            });
        });
    }

    fn fold_at_cursor(&mut self) {
        let Some((t, e)) = self.active_mut() else { return };
        let tab = e.doc.indent.width.max(1);
        let line = e.doc.char_to_line(t.cursors.head());
        // On a header folds it; inside a block folds the block around.
        let mut target = None;
        if display::is_foldable(&e.doc, line, tab) && !t.folds.is_folded(line) {
            target = Some(line);
        } else if let Some(h) = display::enclosing_headers(&e.doc, line, tab, 1).pop().filter(|_| e.doc.structure.as_ref().is_some_and(|s| s.has_tree())) {
            target = Some(h);
        } else {
            let base = display::line_indent_vcols(&e.doc, line, tab);
            let mut l = line;
            while l > 0 {
                l -= 1;
                if !e.doc.line_is_blank(l) && display::line_indent_vcols(&e.doc, l, tab) < base {
                    target = Some(l);
                    break;
                }
            }
        }
        if let Some(h) = target
            && let Some(end) = App::fold_line_range(&e.doc, h)
        {
            t.folds.add(h, end);
            let header_end = e.doc.line_end(h);
            let hidden_end = e.doc.line_end(end);
            t.cursors.map(|s| if s.head > header_end && s.head <= hidden_end { Selection::cursor(header_end) } else { s });
        }
        self.want_reveal = true;
    }

    fn unfold_at_cursor(&mut self) {
        let Some((t, e)) = self.active_mut() else { return };
        let line = e.doc.char_to_line(t.cursors.head());
        if !t.folds.remove(line) {
            t.folds.reveal(line);
        }
    }

    fn fold_all(&mut self) {
        let Some((t, e)) = self.active_mut() else { return };
        let tab = e.doc.indent.width.max(1);
        let mut l = 0;
        let total = e.doc.len_lines();
        while l < total {
            if display::is_foldable(&e.doc, l, tab)
                && let Some(end) = App::fold_line_range(&e.doc, l)
            {
                t.folds.add(l, end);
                l = end + 1;
                continue;
            }
            l += 1;
        }
        let cur = t.cursors.head();
        let cl = e.doc.char_to_line(cur);
        if t.folds.is_hidden(cl) {
            let fold = t.folds.ranges.iter().find(|r| (r.0 as usize) < cl && cl <= r.1 as usize).copied();
            if let Some((h, _)) = fold {
                t.cursors.set_single(Selection::cursor(e.doc.line_end(h as usize)));
            }
        }
        t.scroll_y = 0.0;
        self.want_reveal = true;
    }

    fn open_preview(&mut self) {
        let Some((t, _)) = self.active() else { return };
        let doc = t.doc;
        self.split_group();
        // split_group opened a text view of the document: turn it into the preview.
        if let Some(id) = self.active_tab_id()
            && let Some(t) = self.tabs.get_mut(&id)
        {
            t.doc = doc;
            t.kind = TabKind::Preview;
        }
        self.refresh_tab_titles();
        self.refresh_previews();
    }

    /// Markdown only re-parses when its document changed.
    pub fn refresh_previews(&mut self) {
        let App { tabs, docs, .. } = self;
        for t in tabs.values_mut().filter(|t| t.kind == TabKind::Preview) {
            let Some(e) = docs.get(&t.doc) else { continue };
            if t.preview_cache.as_ref().is_none_or(|(rev, _)| *rev != e.doc.revision) {
                t.preview_cache = Some((e.doc.revision, crate::ui::markdown::parse(&e.doc.rope.to_string())));
            }
        }
    }

    fn set_eol(&mut self, eol: LineEnding) {
        if let Some((_, e)) = self.active_mut() {
            e.doc.eol = eol;
        }
    }

    fn set_indent_kind(&mut self, spaces: bool) {
        if let Some((_, e)) = self.active_mut() {
            e.doc.indent.spaces = spaces;
        }
    }

    fn open_config(&mut self, path: PathBuf, kind: &str) -> Task {
        if !path.exists() {
            let _ = std::fs::create_dir_all(settings::config_dir());
            let body = match kind {
                "settings" => serde_json::to_string_pretty(&self.settings).unwrap_or_else(|_| "{}".into()),
                "keys" => "[\n  // { \"key\": \"ctrl+t\", \"command\": \"file.new\" },\n  // { \"key\": \"ctrl+q\", \"command\": \"-\" }\n]\n".to_string(),
                _ => "{\n  // \"Rust\": [\n  //   { \"prefix\": \"pr\", \"description\": \"print\", \"body\": \"println!(\\\"$1\\\");$0\" }\n  // ]\n}\n".to_string(),
            };
            if let Err(e) = std::fs::write(&path, body) {
                return self.toast(e.to_string());
            }
        }
        self.open_path(&path)
    }

    /// Saves the settings file. Zoom is left out: it belongs to the moment.
    pub fn persist_settings(&mut self) {
        let mut s = self.settings.clone();
        s.font_size = self.base_font_size;
        if let Err(e) = s.save() {
            self.output.push(format!("settings: {e}"));
        }
        self.settings_mtime = super::file_mtime(&settings::settings_path());
    }

    pub fn reload_config_files(&mut self) -> Task {
        let mut tasks = Vec::new();
        self.settings_mtime = super::file_mtime(&settings::settings_path());
        self.keymap_mtime = super::file_mtime(&settings::keybindings_path());
        match std::fs::read_to_string(settings::settings_path()).map(|s| Settings::parse(&s)) {
            Ok(Ok(new)) => {
                let theme_changed = new.ui_theme != self.settings.ui_theme;
                self.base_font_size = new.font_size;
                self.settings = new;
                self.apply_font();
                self.refresh_syntax_theme();
                if theme_changed {
                    tasks.push(self.apply_ui_theme());
                }
            }
            Ok(Err(e)) => tasks.push(self.toast(format!("settings.json: {e}"))),
            Err(_) => {}
        }
        let mut km = Keymap::with_defaults();
        if let Ok(src) = std::fs::read_to_string(settings::keybindings_path()) {
            for e in km.apply_user(&src) {
                self.output.push(format!("keybindings.json: {e}"));
            }
        }
        self.keymap = km;
        Task::batch(tasks)
    }

    pub fn on_setting(&mut self, m: SettingMsg) -> Task {
        let mut extra = Task::none();
        match m {
            SettingMsg::UiTheme(t) => {
                self.settings.ui_theme = t;
                extra = self.apply_ui_theme();
            }
            SettingMsg::SyntaxTheme(n) => {
                self.settings.syntax_theme = n;
                self.refresh_syntax_theme();
            }
            SettingMsg::FontFamily(f) => {
                self.settings.font_family = f;
                self.apply_font();
            }
            SettingMsg::FontSize(s) => {
                self.settings.font_size = s.clamp(6.0, 72.0);
                self.base_font_size = self.settings.font_size;
                self.apply_font();
            }
            SettingMsg::LineHeight(h) => {
                self.settings.line_height = h.clamp(1.0, 3.0);
                self.apply_font();
            }
            SettingMsg::TabSize(n) => {
                self.settings.tab_size = n.clamp(1, 16);
            }
            SettingMsg::LineNumbers(v) => self.settings.line_numbers = v,
            SettingMsg::Whitespace(v) => self.settings.render_whitespace = v,
            SettingMsg::CursorStyle(v) => self.settings.cursor_style = v,
            SettingMsg::AutoSave(v) => self.settings.auto_save = v,
            SettingMsg::Bool(k, v) => {
                let s = &mut self.settings;
                match k {
                    BoolSetting::Minimap => s.minimap = v,
                    BoolSetting::WordWrap => s.word_wrap = v,
                    BoolSetting::CurrentLine => s.highlight_current_line = v,
                    BoolSetting::BracketColors => s.bracket_colors = v,
                    BoolSetting::IndentGuides => s.indent_guides = v,
                    BoolSetting::StickyScroll => s.sticky_scroll = v,
                    BoolSetting::Folding => s.folding = v,
                    BoolSetting::Occurrences => s.occurrences_highlight = v,
                    BoolSetting::CursorBlink => s.cursor_blink = v,
                    BoolSetting::InsertSpaces => s.insert_spaces = v,
                    BoolSetting::DetectIndent => s.detect_indentation = v,
                    BoolSetting::AutoPairs => s.auto_pairs = v,
                    BoolSetting::AutoIndent => s.auto_indent = v,
                    BoolSetting::AutoComplete => s.auto_complete = v,
                    BoolSetting::WordCompletion => s.word_completion = v,
                    BoolSetting::Snippets => s.snippets = v,
                    BoolSetting::GitGutter => s.git_gutter = v,
                    BoolSetting::InlineBlame => s.inline_blame = v,
                    BoolSetting::FormatOnSave => s.format_on_save = v,
                    BoolSetting::TrimTrailing => s.trim_trailing_whitespace = v,
                    BoolSetting::FinalNewline => s.insert_final_newline = v,
                    BoolSetting::RestoreSession => s.restore_session = v,
                    BoolSetting::HotExit => s.hot_exit = v,
                    BoolSetting::ShowHidden => {
                        s.show_hidden_files = v;
                        self.reload_explorer();
                        self.index_files();
                    }
                    BoolSetting::Lsp => s.lsp_enabled = v,
                    BoolSetting::Vim => {
                        s.vim_mode = v;
                        self.vim = tinta_core::vim::Vim::new();
                        self.vim_reset();
                    }
                }
            }
        }
        self.persist_settings();
        extra
    }

    pub fn on_dialog(&mut self, m: DialogMsg) -> Task {
        match m {
            DialogMsg::Input(s) => {
                self.dialog_input = s;
                Task::none()
            }
            DialogMsg::ShortcutsFilter(s) => {
                self.shortcuts_filter = s;
                Task::none()
            }
            DialogMsg::Cancel => {
                self.dialog = None;
                self.dialog_input.clear();
                self.focus = Focus::Editor;
                Task::none()
            }
            DialogMsg::Secondary => {
                self.focus = Focus::Editor;
                let d = self.dialog.take();
                match d {
                    Some(DialogState::CloseUnsaved(id)) => self.close_tab(id),
                    Some(DialogState::QuitUnsaved) => {
                        for e in self.docs.values_mut() {
                            e.doc.mark_saved();
                        }
                        self.quit_now()
                    }
                    _ => Task::none(),
                }
            }
            DialogMsg::Primary => {
                self.focus = Focus::Editor;
                let d = self.dialog.take();
                let input = std::mem::take(&mut self.dialog_input);
                match d {
                    Some(DialogState::CloseUnsaved(id)) => self.save_then_close(id),
                    Some(DialogState::QuitUnsaved) => {
                        let t = self.save_all_dirty_named();
                        let q = self.quit_now();
                        Task::batch([t, q])
                    }
                    Some(DialogState::ChangedOnDisk(doc)) => self.reload_doc(doc),
                    Some(DialogState::NewFile(dir)) => self.create_file(dir, &input),
                    Some(DialogState::NewFolder(dir)) => self.create_folder(dir, &input),
                    Some(DialogState::RenamePath(p)) => self.rename_path(p, &input),
                    Some(DialogState::TrashPath(p)) => self.trash_path(p),
                    Some(DialogState::RenameSymbol) => self.do_rename(input),
                    Some(DialogState::BreakpointCondition(path, line)) => {
                        self.set_breakpoint_condition(path, line, input);
                        Task::none()
                    }
                    Some(DialogState::ConfirmDiscard(entry)) => self.git_discard(entry),
                    _ => Task::none(),
                }
            }
        }
    }

    fn save_then_close(&mut self, id: TabId) -> Task {
        let t = self.save_tab(id);
        // Untitled buffers go through the save dialog first and stay open until it answers.
        let named = self.tabs.get(&id).and_then(|t| self.docs.get(&t.doc)).is_some_and(|e| e.doc.path.is_some());
        if named {
            let c = self.close_tab(id);
            return Task::batch([t, c]);
        }
        t
    }

    pub fn toggle_sidebar_view(&mut self, v: SidebarView) -> Task {
        if self.sidebar == Some(v) {
            self.sidebar = None;
            return Task::none();
        }
        self.show_sidebar(v)
    }

    pub fn show_sidebar(&mut self, v: SidebarView) -> Task {
        self.sidebar = Some(v);
        self.last_sidebar = v;
        self.mark_session_dirty();
        match v {
            SidebarView::Git => self.refresh_git_panel(),
            SidebarView::Outline => self.request_symbols(),
            _ => {}
        }
        Task::none()
    }

    pub fn close_popups(&mut self) {
        self.completion = None;
        self.hover = None;
        self.signature = None;
    }

    pub fn on_escape_key(&mut self) -> Task {
        if self.completion.is_some() || self.hover.is_some() || self.signature.is_some() {
            self.close_popups();
            return Task::none();
        }
        if self.find.open {
            self.close_find();
            return Task::none();
        }
        if self.settings_open {
            self.settings_open = false;
            return Task::none();
        }
        if self.zen {
            return self.run(Command::ToggleZen);
        }
        if let Some((t, _)) = self.active_mut() {
            t.snippet = None;
            if t.cursors.is_multi() {
                t.cursors.keep_primary();
            } else {
                let h = t.cursors.head();
                t.cursors.set_single(Selection::cursor(h));
            }
        }
        self.focus = Focus::Editor;
        self.after_caret_move();
        Task::none()
    }

    fn create_file(&mut self, dir: PathBuf, name: &str) -> Task {
        if name.trim().is_empty() {
            return Task::none();
        }
        let path = dir.join(name.trim());
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !path.exists()
            && let Err(e) = std::fs::write(&path, b"")
        {
            return self.toast(e.to_string());
        }
        self.reload_explorer();
        self.open_path(&path)
    }

    fn create_folder(&mut self, dir: PathBuf, name: &str) -> Task {
        if name.trim().is_empty() {
            return Task::none();
        }
        if let Err(e) = std::fs::create_dir_all(dir.join(name.trim())) {
            return self.toast(e.to_string());
        }
        self.explorer.expanded.insert(dir);
        self.reload_explorer();
        Task::none()
    }

    fn rename_path(&mut self, from: PathBuf, name: &str) -> Task {
        if name.trim().is_empty() {
            return Task::none();
        }
        let Some(parent) = from.parent() else { return Task::none() };
        let to = parent.join(name.trim());
        if let Err(e) = std::fs::rename(&from, &to) {
            return self.toast(e.to_string());
        }
        if let Some(id) = self.doc_by_path(&from)
            && let Some(e) = self.docs.get_mut(&id)
        {
            e.doc.path = Some(to);
        }
        self.reload_explorer();
        self.refresh_tab_titles();
        Task::none()
    }

    fn trash_path(&mut self, p: PathBuf) -> Task {
        if let Err(e) = trash::delete(&p) {
            return self.toast(e.to_string());
        }
        if let Some(id) = self.doc_by_path(&p) {
            let tabs: Vec<TabId> = self.tabs.values().filter(|t| t.doc == id).map(|t| t.id).collect();
            for t in tabs {
                let _ = self.close_tab(t);
            }
        }
        self.reload_explorer();
        Task::none()
    }
}

fn open_file_dialog(_dir: Option<PathBuf>) -> Task {
    Task::effect(fx::Effect::OpenFiles(Box::new(Message::OpenFiles)))
}

fn open_folder_dialog() -> Task {
    Task::effect(fx::Effect::OpenFolder(Box::new(Message::OpenFolderPath)))
}
