//! Connects the Vim state machine to the editor: keys in, actions out.

use super::{App, Focus, KeyEv, Message};
use crate::i18n::tr;
use crate::state::TabKind;
use crate::fx::{self, Task};
use tinta_core::keymap::Command;
use tinta_core::settings::LineNumbers;
use tinta_core::vim::{Action, Ctx, Key};

pub const VIM_LINE: &str = "vim-line";

#[derive(Debug, Clone)]
pub enum VimMsg {
    Input(String),
    Submit,
    Pasted(Option<String>, bool),
}

fn to_vim_key(ev: &KeyEv) -> Option<Key> {
    let named = match ev.name.as_str() {
        "escape" => "escape",
        "enter" => "enter",
        "backspace" => "backspace",
        "delete" => "delete",
        "tab" => "tab",
        "up" => "up",
        "down" => "down",
        "left" => "left",
        "right" => "right",
        "home" => "home",
        "end" => "end",
        "pageup" => "pageup",
        "pagedown" => "pagedown",
        _ => "",
    };
    if !named.is_empty() {
        return Some(Key::named(named));
    }
    if ev.mods.control() {
        let mut it = ev.name.chars();
        return match (it.next(), it.next()) {
            (Some(c), None) => Some(Key::ctrl(c)),
            _ => None,
        };
    }
    let text = ev.text.as_deref()?;
    let mut it = text.chars();
    match (it.next(), it.next()) {
        (Some(c), None) if !c.is_control() => Some(Key::ch(c)),
        _ => None,
    }
}

/// Keys that change text. On a read-only buffer, those do nothing and the rest still moves around.
fn is_edit_key(c: char, visual: bool) -> bool {
    if visual { "dxXDcsSCpPJUu~<>rR".contains(c) } else { "dcxXsSDCrRiIaAoOpPJu~.><".contains(c) }
}

impl App {
    pub fn vim_active(&self) -> bool {
        self.settings.vim_mode && self.focus == Focus::Editor && self.active().is_some_and(|(t, _)| t.kind == TabKind::Text)
    }

    fn vim_ctx_run<R>(&mut self, f: impl FnOnce(&mut tinta_core::vim::Vim, &mut Ctx<'_>) -> R) -> Option<R> {
        let id = self.active_tab_id()?;
        let page_rows = self.page_rows();
        let opts = self.edit_opts();
        let line_h = self.metrics.line_h.max(1.0);
        let App { tabs, docs, vim, .. } = self;
        let t = tabs.get_mut(&id)?;
        let e = docs.get_mut(&t.doc)?;
        let first_row = (t.scroll_y / line_h).ceil() as usize;
        let mut ctx = Ctx { doc: &mut e.doc, cur: &mut t.cursors, map: &t.map, page_rows, first_row, opts };
        let out = f(vim, &mut ctx);
        self.edit_tab = Some(id);
        Some(out)
    }

    /// `Some`: Vim dealt with it. `None`: on to the keymap and the editor.
    pub fn vim_on_key(&mut self, ev: &KeyEv) -> Option<Task> {
        if !self.vim_active() || ev.mods.alt() || ev.mods.logo() || self.keymap.is_pending() {
            return None;
        }
        let key = to_vim_key(ev)?;
        let insert = self.vim.is_insert();
        if key.name == "escape" {
            // First things first: an open list or find bar closes before anything else happens.
            if !insert && (self.completion.is_some() || self.hover.is_some() || self.signature.is_some() || self.find.open) {
                return None;
            }
            self.close_popups();
        }
        if !insert && !self.can_edit() && !self.vim.is_waiting() && !key.ctrl {
            let visual = self.vim.mode != tinta_core::vim::Mode::Normal;
            if key.ch.is_some_and(|c| is_edit_key(c, visual)) {
                return Some(Task::none());
            }
        }
        if !insert && !self.can_edit() && key.ctrl && matches!(key.ch, Some('r' | 'a' | 'x')) {
            return Some(Task::none());
        }
        if insert && !self.can_edit() {
            self.vim.mode = tinta_core::vim::Mode::Normal;
        }
        let out = self.vim_ctx_run(|vim, ctx| vim.key(ctx, key))?;
        if !out.handled {
            // A key it doesn't know: in insert mode the editor types it, and otherwise only the
            // keymap gets a look, so nothing lands in the text by accident.
            return None;
        }
        self.want_reveal = true;
        self.after_caret_move();
        Some(self.vim_actions(out.actions))
    }

    fn vim_actions(&mut self, actions: Vec<Action>) -> Task {
        let mut tasks = Vec::new();
        for a in actions {
            tasks.push(self.vim_action(a));
        }
        Task::batch(tasks)
    }

    fn vim_action(&mut self, a: Action) -> Task {
        match a {
            Action::Save => self.save_active(),
            Action::SaveAll => self.save_all_dirty_named(),
            Action::Quit { force } => self.vim_close(force, true),
            Action::CloseTab { force } => self.vim_close(force, false),
            Action::SaveQuit => {
                let t = self.save_active();
                let c = self.vim_close(true, true);
                Task::batch([t, c])
            }
            Action::QuitAll { force } => {
                if force {
                    for e in self.docs.values_mut() {
                        e.doc.mark_saved();
                    }
                    self.quit_now()
                } else {
                    self.request_quit()
                }
            }
            Action::OpenFile(p) => {
                let path = std::path::PathBuf::from(&p);
                let path = if path.is_absolute() { path } else { self.folder.clone().unwrap_or_default().join(path) };
                self.open_path(&path)
            }
            Action::NewTab => self.new_scratch(),
            Action::SplitRight => self.run(Command::SplitRight),
            Action::NextTab => self.run(Command::NextTab),
            Action::PrevTab => self.run(Command::PrevTab),
            Action::NextGroup => self.run(Command::FocusNextGroup),
            Action::PrevGroup => self.run(Command::FocusPrevGroup),
            Action::CommandLine(prefix) => {
                self.vim_line = Some((prefix, String::new()));
                self.focus = Focus::Input;
                fx::focus(VIM_LINE)
            }
            Action::SetClipboard(text) => fx::clipboard_write(text),
            Action::PasteClipboard { before } => fx::clipboard_read(move |t| Message::Vim(VimMsg::Pasted(t, before))),
            Action::Run(id) => match Command::from_id(id) {
                Some(c) => self.run(c),
                None => Task::none(),
            },
            Action::Center => {
                self.center_active();
                Task::none()
            }
            Action::ScrollTop | Action::ScrollBottom => {
                let m = self.metrics;
                if let Some((t, e)) = self.active_mut() {
                    let (row, _) = t.map.pos_to_row(&e.doc, t.cursors.head());
                    let y = row as f32 * m.line_h;
                    t.scroll_y = if matches!(a, Action::ScrollTop) { y } else { (y + m.line_h - t.view_h).max(0.0) };
                }
                Task::none()
            }
            Action::Highlight(q) => {
                self.vim_highlight = q;
                self.find.generation += 1;
                if self.vim_highlight.is_none()
                    && !self.find.open
                    && let Some((_, e)) = self.active_mut()
                {
                    e.matches.clear();
                    e.matches_key = None;
                }
                Task::none()
            }
            Action::SetOption(opt) => self.vim_set(&opt),
            Action::Message(m) => self.toast(m),
        }
    }

    /// `:q` closes the tab, and the last tab closes the editor. `:bd` only ever closes the tab.
    fn vim_close(&mut self, force: bool, quit_when_last: bool) -> Task {
        let Some(id) = self.active_tab_id() else {
            return if quit_when_last { self.request_quit() } else { Task::none() };
        };
        let dirty = self.tabs.get(&id).and_then(|t| self.docs.get(&t.doc)).is_some_and(|e| e.doc.is_dirty() && !e.is_read_only_view);
        if dirty && !force {
            return self.toast(tr("No write since last change (add ! to override)"));
        }
        let task = if force { self.close_tab(id) } else { self.request_close_tab(id) };
        if quit_when_last && self.tabs.is_empty() {
            return Task::batch([task, self.request_quit()]);
        }
        task
    }

    fn vim_set(&mut self, opt: &str) -> Task {
        let s = &mut self.settings;
        match opt.trim() {
            "wrap" => s.word_wrap = true,
            "nowrap" => s.word_wrap = false,
            "nu" | "number" => s.line_numbers = LineNumbers::On,
            "rnu" | "relativenumber" => s.line_numbers = LineNumbers::Relative,
            "nonu" | "nonumber" | "nornu" | "norelativenumber" => s.line_numbers = LineNumbers::Off,
            "list" => s.render_whitespace = tinta_core::settings::Whitespace::All,
            "nolist" => s.render_whitespace = tinta_core::settings::Whitespace::Selection,
            other => {
                if let Some(n) = other.strip_prefix("ts=").or(other.strip_prefix("tabstop=")).or(other.strip_prefix("sw=")).and_then(|n| n.parse::<usize>().ok()) {
                    if let Some((_, e)) = self.active_mut() {
                        e.doc.indent.width = n.clamp(1, 16);
                    }
                    return Task::none();
                }
                return self.toast(format!("E518: {other}"));
            }
        }
        self.persist_settings();
        Task::none()
    }

    /// The buffer on screen changed under Vim: whatever it was doing there is over.
    pub fn vim_reset(&mut self) {
        if !self.settings.vim_mode {
            return;
        }
        if self.vim.is_insert() {
            for e in self.docs.values_mut() {
                e.doc.end_group();
            }
        }
        self.vim.mode = tinta_core::vim::Mode::Normal;
        self.vim_ctx_run(|vim, ctx| vim.clamp_caret(ctx));
    }

    pub fn vim_after_mouse(&mut self) {
        if self.settings.vim_mode {
            self.vim_ctx_run(|vim, ctx| vim.after_mouse(ctx));
        }
    }

    pub fn on_vim(&mut self, m: VimMsg) -> Task {
        match m {
            VimMsg::Input(s) => {
                if let Some((_, text)) = &mut self.vim_line {
                    *text = s;
                }
                Task::none()
            }
            VimMsg::Submit => {
                let Some((prefix, text)) = self.vim_line.take() else { return Task::none() };
                self.focus = Focus::Editor;
                let Some(actions) = self.vim_ctx_run(|vim, ctx| vim.command_line(ctx, prefix, &text)) else {
                    return Task::none();
                };
                self.want_reveal = true;
                self.after_caret_move();
                self.vim_actions(actions)
            }
            VimMsg::Pasted(text, before) => {
                if let Some(text) = text {
                    let text = tinta_core::buffer::normalize_newlines(&text).into_owned();
                    self.vim_ctx_run(|vim, ctx| vim.paste_text(ctx, text, before));
                    self.want_reveal = true;
                }
                Task::none()
            }
        }
    }
}
