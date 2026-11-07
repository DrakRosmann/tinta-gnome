//! The sidebar's other lists by keyboard: search results, git changes, the outline and the
//! debugger's variables. The app keeps the row the keyboard is on, as it does for the file tree.

use super::{App, DebugMsg, Focus, GitMsg, KeyEv, Message, SearchMsg, SearchRow, SidebarView};
use crate::fx::{self, Task};
use tinta_core::git::StatusEntry;

pub const GIT_INPUT: &str = "git-input";

impl App {
    /// The git list as it's shown: staged changes, then the rest. A file half staged is in both.
    pub fn git_rows(&self) -> Vec<(StatusEntry, bool)> {
        let e = &self.git.entries;
        e.iter().filter(|e| e.is_staged()).map(|e| (e.clone(), true)).chain(e.iter().filter(|e| e.is_unstaged()).map(|e| (e.clone(), false))).collect()
    }

    fn list_len(&self, view: SidebarView) -> usize {
        match view {
            SidebarView::Search => self.search.rows.len(),
            SidebarView::Git => self.git_rows().len(),
            SidebarView::Outline => self.active().map_or(0, |(_, e)| super::symbols_of(e).len()),
            SidebarView::Debug => self.debug.session.as_ref().map_or(0, |s| s.variable_rows().len()),
            SidebarView::Explorer => 0,
        }
    }

    /// The keyboard goes to the list on show, on the row it was on, or near the caret in the
    /// outline. False when there's nothing in it.
    pub fn focus_list(&mut self, view: SidebarView) -> bool {
        let n = self.list_len(view);
        if n == 0 {
            return false;
        }
        if view == SidebarView::Outline
            && let Some((t, e)) = self.active()
        {
            let line = e.doc.char_to_line(t.cursors.head());
            self.list_cursor = super::symbols_of(e).iter().rposition(|s| s.2 <= line).unwrap_or(0);
        }
        self.list_cursor = self.list_cursor.min(n - 1);
        self.focus = Focus::List;
        true
    }

    /// The field above the list, where Up from its first row goes back to.
    fn list_field(view: SidebarView) -> Option<&'static str> {
        match view {
            SidebarView::Search => Some(super::SEARCH_INPUT),
            SidebarView::Git => Some(GIT_INPUT),
            _ => None,
        }
    }

    /// Keys with a sidebar list focused. `None` leaves the key to the shortcuts.
    pub fn list_key(&mut self, ev: &KeyEv) -> Option<Task> {
        if ev.mods.control() || ev.mods.alt() || ev.mods.logo() {
            return None;
        }
        let view = self.sidebar?;
        if ev.name == "escape" {
            self.focus = Focus::Editor;
            return Some(fx::focus_editor());
        }
        let n = self.list_len(view);
        if n == 0 {
            self.focus = Focus::Editor;
            return Some(fx::focus_editor());
        }
        let cur = self.list_cursor.min(n - 1);
        let page = 10;
        let go = |app: &mut App, i: usize| {
            app.list_cursor = i;
            Some(Task::none())
        };
        match ev.name.as_str() {
            "up" if cur == 0 => match Self::list_field(view) {
                Some(field) => {
                    self.focus = Focus::Input;
                    Some(fx::focus(field))
                }
                None => Some(Task::none()),
            },
            "up" => go(self, cur - 1),
            "down" => go(self, (cur + 1).min(n - 1)),
            "home" => go(self, 0),
            "end" => go(self, n - 1),
            "pageup" => go(self, cur.saturating_sub(page)),
            "pagedown" => go(self, (cur + page).min(n - 1)),
            "enter" => Some(self.list_activate(view, cur)),
            _ => self.list_extra_key(view, cur, &ev.name),
        }
    }

    /// Enter: what a click on the row does.
    fn list_activate(&mut self, view: SidebarView, i: usize) -> Task {
        match view {
            SidebarView::Search => match self.search.rows.get(i).cloned() {
                Some(SearchRow::File { path, .. }) => self.update(Message::Search(SearchMsg::ToggleFile(path))),
                Some(SearchRow::Match { path, line, start, end, .. }) => self.update(Message::Search(SearchMsg::Open(path, line, start, end))),
                None => Task::none(),
            },
            SidebarView::Git => match self.git_rows().get(i).cloned() {
                Some((e, staged)) => self.update(Message::Git(GitMsg::Open(e.path, staged))),
                None => Task::none(),
            },
            SidebarView::Outline => match self.active().and_then(|(_, e)| super::symbols_of(e).get(i).map(|s| s.2)) {
                Some(line) => self.update(Message::OutlineJump(line)),
                None => Task::none(),
            },
            SidebarView::Debug => match self.debug.session.as_ref().and_then(|s| s.variable_rows().get(i).cloned()) {
                Some(row) if row.expandable => self.update(Message::Debug(DebugMsg::ToggleRow(row))),
                Some(row) => self.update(Message::Debug(DebugMsg::EditValue(row))),
                None => Task::none(),
            },
            SidebarView::Explorer => Task::none(),
        }
    }

    /// Keys a list has of its own: opening and closing in the trees, staging in git.
    fn list_extra_key(&mut self, view: SidebarView, i: usize, key: &str) -> Option<Task> {
        match (view, key) {
            // A file's results open and close; left on a result goes up to its file.
            (SidebarView::Search, "right" | "left") => {
                let row = self.search.rows.get(i).cloned()?;
                match row {
                    SearchRow::File { path, collapsed, .. } if collapsed == (key == "right") => Some(self.update(Message::Search(SearchMsg::ToggleFile(path)))),
                    SearchRow::Match { .. } if key == "left" => {
                        self.list_cursor = self.search.rows[..i].iter().rposition(|r| matches!(r, SearchRow::File { .. })).unwrap_or(0);
                        Some(Task::none())
                    }
                    _ => Some(Task::none()),
                }
            }
            // Space stages a change, or takes it back out.
            (SidebarView::Git, "space") => {
                let (e, staged) = self.git_rows().get(i).cloned()?;
                let m = if staged { GitMsg::Unstage(e.path) } else { GitMsg::Stage(e.path) };
                Some(self.update(Message::Git(m)))
            }
            (SidebarView::Debug, "right" | "left") => {
                let row = self.debug.session.as_ref()?.variable_rows().get(i).cloned()?;
                if row.expandable && row.expanded == (key == "left") {
                    return Some(self.update(Message::Debug(DebugMsg::ToggleRow(row))));
                }
                if key == "left" {
                    let rows = self.debug.session.as_ref()?.variable_rows();
                    self.list_cursor = rows[..i].iter().rposition(|r| r.depth < row.depth).unwrap_or(i);
                }
                Some(Task::none())
            }
            (SidebarView::Debug, "f2") => {
                let row = self.debug.session.as_ref()?.variable_rows().get(i).cloned()?;
                Some(self.update(Message::Debug(DebugMsg::EditValue(row))))
            }
            _ => None,
        }
    }
}
