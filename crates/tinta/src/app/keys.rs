//! From raw key events to commands or text.

use super::{App, Focus, Message};
use crate::fx::{self, Task};
use crate::gfx::Modifiers;
use crate::i18n::tr;
use tinta_core::commands::{self, Motion};
use tinta_core::keymap::{Command, KeyResult, Stroke};

/// A key press, stripped of the toolkit's types.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyEv {
    /// Lowercase key name without modifiers: `a`, `enter`, `f1`, `/`.
    pub name: String,
    /// The key with shift applied, when it differs: `+` for shift and `=`.
    pub shifted: Option<String>,
    pub mods: Modifiers,
    pub text: Option<String>,
    pub captured: bool,
}

fn stroke(name: &str, mods: Modifiers) -> Stroke {
    Stroke { ctrl: mods.control(), shift: mods.shift(), alt: mods.alt(), logo: mods.logo(), key: name.to_string() }
}

/// With focus in the terminal, these keep working; the rest belongs to the program running there.
fn terminal_passes(c: Command, s: &Stroke) -> bool {
    use Command as C;
    if s.ctrl && s.shift {
        return !matches!(c, C::Copy | C::Paste);
    }
    if !s.ctrl && !s.alt && s.key.starts_with('f') && s.key.len() > 1 {
        return matches!(c, C::CommandPalette | C::ToggleFullscreen);
    }
    matches!(c, C::ToggleTerminal | C::TogglePanel | C::ToggleSidebar | C::QuickOpen | C::NextTab | C::PrevTab | C::CommandPalette | C::OpenSettings)
}

impl App {
    pub fn on_key(&mut self, ev: KeyEv) -> Task {
        // Something else took it, a text field most likely. Shortcuts with Ctrl still count,
        // since fields only claim the ones they use.
        if ev.captured {
            // ...but a text field also claims Escape and the arrows, and those drive what it's in.
            let overlay = self.palette.is_some() && matches!(ev.name.as_str(), "escape" | "up" | "down" | "pageup" | "pagedown");
            let closes = ev.name == "escape" && (self.dialog.is_some() || self.find.open);
            if ev.name == "escape" && self.vim_line.take().is_some() {
                self.focus = Focus::Editor;
                return fx::focus_editor();
            }
            if !overlay && !closes {
                // Escape in any other field hands the keyboard back to the text.
                if ev.name == "escape" {
                    self.focus = Focus::Editor;
                    return fx::focus_editor();
                }
                // Anything else is the field's.
                self.key_passthrough = true;
                return Task::none();
            }
        }
        if self.dialog.is_some() {
            if ev.name == "escape" {
                return self.on_dialog(super::DialogMsg::Cancel);
            }
            return Task::none();
        }
        if self.palette.is_some() {
            return self.palette_key(&ev);
        }
        // F11 steps in while debugging, as everywhere else, and is full screen the rest of the time.
        if ev.name == "f11" && ev.mods.is_empty() && self.debug.is_live() {
            return self.run(tinta_core::keymap::Command::StepInto);
        }
        if let Some(t) = self.vim_on_key(&ev) {
            return t;
        }
        if ev.name == "escape" {
            return self.on_escape_key();
        }
        let has_suggestions = self.completion.as_ref().is_some_and(|c| !c.visible.is_empty());
        if has_suggestions && self.focus == Focus::Editor && !ev.mods.control() && !ev.mods.alt() {
            match ev.name.as_str() {
                "up" => return self.completion_move(-1),
                "down" => return self.completion_move(1),
                "pageup" => return self.completion_move(-8),
                "pagedown" => return self.completion_move(8),
                "tab" => return self.accept_completion(),
                // Enter ends the line unless the list is what the user is working with: a word
                // from the file popping up must not eat the newline after `x = 1`.
                "enter" => {
                    if self.completion.as_ref().is_some_and(|c| c.enter_accepts()) {
                        return self.accept_completion();
                    }
                    self.completion = None;
                }
                _ => {}
            }
        }
        if self.focus == Focus::Editor && ev.name == "tab" && !ev.mods.control() && !ev.mods.alt() && self.snippet_step(!ev.mods.shift()) {
            return Task::none();
        }

        let s1 = stroke(&ev.name, ev.mods);
        let s2 = ev.shifted.as_ref().map(|k| {
            let mut m = ev.mods;
            m.remove(Modifiers::SHIFT);
            stroke(k, m)
        });
        let was_pending = self.keymap.is_pending();
        let chosen = if self.keymap.would_match(&s1) {
            s1.clone()
        } else if let Some(s) = s2.filter(|s| self.keymap.would_match(s)) {
            s
        } else {
            s1.clone()
        };
        match self.keymap.press(chosen.clone()) {
            KeyResult::Run(c) => {
                if self.focus == Focus::Terminal {
                    if chosen.ctrl && chosen.shift && chosen.key == "c" {
                        return self.terminal_copy();
                    }
                    if chosen.ctrl && chosen.shift && chosen.key == "v" {
                        return self.terminal_paste();
                    }
                    if !was_pending && !terminal_passes(c, &chosen) {
                        return self.terminal_key(&ev);
                    }
                }
                self.run(c)
            }
            KeyResult::Pending => {
                let t = self.keymap.pending_label().unwrap_or_default();
                self.toast(format!("{t}: {}", tr("Waiting for the next key…")))
            }
            KeyResult::None => {
                if was_pending {
                    self.toast = None;
                    return Task::none();
                }
                match self.focus {
                    Focus::Terminal => {
                        if ev.mods.control() && ev.mods.shift() {
                            match ev.name.as_str() {
                                "c" => return self.terminal_copy(),
                                "v" => return self.terminal_paste(),
                                _ => {}
                            }
                        }
                        self.terminal_key(&ev)
                    }
                    // In Vim's other modes a key it passed on stops here, or it would get typed.
                    Focus::Editor if self.settings.vim_mode && !self.vim.is_insert() => Task::none(),
                    Focus::Editor => self.editor_key(&ev),
                    Focus::Input => {
                        self.key_passthrough = true;
                        Task::none()
                    }
                }
            }
        }
    }

    fn terminal_copy(&mut self) -> Task {
        match self.terms.get(self.active_term) {
            Some(t) => Task::effect(fx::Effect::TermCopy(t.id)),
            None => Task::none(),
        }
    }

    fn terminal_paste(&mut self) -> Task {
        match self.terms.get(self.active_term) {
            Some(t) => Task::effect(fx::Effect::TermPaste(t.id)),
            None => Task::none(),
        }
    }

    /// The terminal widget takes the key itself.
    fn terminal_key(&mut self, _ev: &KeyEv) -> Task {
        self.key_passthrough = true;
        Task::none()
    }

    /// Keys with a fixed meaning in the text: the ones not worth remapping.
    fn editor_key(&mut self, ev: &KeyEv) -> Task {
        if !ev.mods.control() && !ev.mods.alt() && self.diff_key(&ev.name) {
            return Task::none();
        }
        let m = ev.mods;
        let (ctrl, shift, alt) = (m.control(), m.shift(), m.alt());
        let motion = match (ev.name.as_str(), ctrl, alt) {
            ("left", false, false) => Some(Motion::Left),
            ("right", false, false) => Some(Motion::Right),
            ("left", true, false) => Some(Motion::WordLeft),
            ("right", true, false) => Some(Motion::WordRight),
            ("left", false, true) => Some(Motion::SubwordLeft),
            ("right", false, true) => Some(Motion::SubwordRight),
            ("up", false, false) => Some(Motion::Up),
            ("down", false, false) => Some(Motion::Down),
            ("home", false, _) => Some(Motion::LineStart),
            ("end", false, _) => Some(Motion::LineEnd),
            ("home", true, _) => Some(Motion::DocStart),
            ("end", true, _) => Some(Motion::DocEnd),
            ("pageup", false, false) => Some(Motion::PageUp(self.page_rows())),
            ("pagedown", false, false) => Some(Motion::PageDown(self.page_rows())),
            ("up", true, true) | ("down", true, true) => None,
            _ => None,
        };
        if let Some(mo) = motion {
            self.move_caret(mo, shift);
            return Task::none();
        }
        if ctrl && shift {
            let para = match ev.name.as_str() {
                "up" => Some(Motion::ParagraphUp),
                "down" => Some(Motion::ParagraphDown),
                _ => None,
            };
            if let Some(mo) = para {
                self.move_caret(mo, true);
                return Task::none();
            }
        }
        match ev.name.as_str() {
            "enter" if !ctrl && !alt => return self.key_enter(),
            "tab" if !ctrl && !alt => return self.key_tab(shift),
            "backspace" if !ctrl && !alt => return self.key_backspace(),
            "delete" if !ctrl && !alt && !shift => return self.key_delete(),
            _ => {}
        }
        // Everything else that produces text. AltGr arrives without Alt or Ctrl set; layouts that
        // report it as Ctrl+Alt still produce text, so those count as typing too.
        if let Some(t) = &ev.text {
            let printable = !t.is_empty() && !t.chars().any(char::is_control);
            let plain = (!ctrl && !alt && !m.logo()) || (ctrl && alt);
            if printable && plain {
                return self.type_text(t);
            }
        }
        Task::none()
    }

    pub fn page_rows(&self) -> usize {
        match self.active() {
            Some((t, _)) => ((t.view_h / self.metrics.line_h) as usize).saturating_sub(2).max(1),
            None => 20,
        }
    }

    pub fn move_caret(&mut self, m: Motion, extend: bool) {
        let Some((t, e)) = self.active_mut() else { return };
        commands::move_cursors(&e.doc, &mut t.cursors, &t.map, m, extend);
        t.expand_stack.clear();
        e.doc.break_undo();
        self.want_reveal = true;
        self.after_caret_move();
    }
}
