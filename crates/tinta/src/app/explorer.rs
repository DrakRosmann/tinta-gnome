//! The file tree.

use super::{App, DialogState, Focus, KeyEv};
use crate::fx::{self, Task};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    pub git: Option<char>,
    pub ignored: bool,
}

#[derive(Default)]
pub struct Explorer {
    pub expanded: BTreeSet<PathBuf>,
    pub rows: Vec<Row>,
    pub selected: Option<PathBuf>,
    pub scroll_y: f32,
    pub view_h: f32,
    pub status: HashMap<PathBuf, char>,
    pub ignored: HashSet<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum ExplorerMsg {
    Click(usize),
    Scrolled(f32, f32),
    NewFile(Option<PathBuf>),
    NewFolder(Option<PathBuf>),
    Rename(PathBuf),
    Trash(PathBuf),
    CopyPath(PathBuf),
    CopyRelative(PathBuf),
    Reveal(PathBuf),
    OpenSide(PathBuf),
    TerminalHere(PathBuf),
    Refresh,
    CollapseAll,
    RowMenu(usize, RowAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    OpenSide,
    NewFile,
    NewFolder,
    Rename,
    Trash,
    CopyPath,
    CopyRelative,
    Reveal,
    Terminal,
}

/// Languages whose compilers leave the program beside the source, named after it.
const COMPILED: &[&str] = &["c", "cc", "cpp", "cxx", "c++", "rs", "go", "zig", "hs", "m", "f", "f90", "pas", "nim", "d", "cr", "ml"];

/// A program built from a source next to it: no extension, executable, and `name.c` (or the
/// like) in the same folder.
fn is_build_output(name: &str, path: &Path, siblings: &std::collections::HashSet<String>) -> bool {
    use std::os::unix::fs::PermissionsExt;
    !name.contains('.')
        && COMPILED.iter().any(|ext| siblings.contains(&format!("{name}.{ext}")))
        && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn list_dir(dir: &Path, hidden: bool, hide_outputs: bool) -> Vec<(String, PathBuf, bool)> {
    let Ok(read) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut items: Vec<(String, PathBuf, bool)> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" || (!hidden && name.starts_with('.')) {
                return None;
            }
            let path = e.path();
            // Follow symlinks so a linked folder opens like a folder.
            let is_dir = path.is_dir();
            Some((name, path, is_dir))
        })
        .collect();
    if hide_outputs {
        let names: std::collections::HashSet<String> = items.iter().filter(|i| !i.2).map(|i| i.0.clone()).collect();
        items.retain(|(name, path, is_dir)| *is_dir || !is_build_output(name, path, &names));
    }
    items.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    items
}

impl App {
    pub fn reload_explorer(&mut self) {
        let Some(root) = self.folder.clone() else {
            self.explorer.rows.clear();
            return;
        };
        let (hidden, outputs) = (self.settings.show_hidden_files, self.settings.hide_build_outputs);
        let mut rows = Vec::new();
        self.flatten_into(&mut rows, list_dir(&root, hidden, outputs), 0);
        self.explorer.rows = rows;
        self.update_watches();
    }

    /// Depth first, walking down into the expanded folders.
    fn flatten_into(&self, rows: &mut Vec<Row>, items: Vec<(String, PathBuf, bool)>, depth: usize) {
        let (hidden, outputs) = (self.settings.show_hidden_files, self.settings.hide_build_outputs);
        for (name, path, is_dir) in items {
            let expanded = is_dir && self.explorer.expanded.contains(&path);
            rows.push(Row {
                git: self.explorer.status.get(&path).copied(),
                ignored: self.explorer.ignored.contains(&path),
                name,
                path: path.clone(),
                depth,
                is_dir,
                expanded,
            });
            if expanded {
                self.flatten_into(rows, list_dir(&path, hidden, outputs), depth + 1);
            }
        }
    }

    pub fn on_explorer(&mut self, m: ExplorerMsg) -> Task {
        match m {
            ExplorerMsg::Click(i) => {
                let Some(row) = self.explorer.rows.get(i).cloned() else { return Task::none() };
                self.explorer.selected = Some(row.path.clone());
                if row.is_dir {
                    // The arrows carry on from the folder clicked.
                    self.focus = Focus::Explorer;
                    if !self.explorer.expanded.remove(&row.path) {
                        self.explorer.expanded.insert(row.path);
                    }
                    self.reload_explorer();
                    self.mark_session_dirty();
                    return Task::none();
                }
                self.open_from_tree(&row.path)
            }
            ExplorerMsg::Scrolled(y, h) => {
                self.explorer.scroll_y = y;
                self.explorer.view_h = h;
                Task::none()
            }
            ExplorerMsg::NewFile(dir) => {
                let dir = dir.or_else(|| self.explorer_target_dir());
                if let Some(d) = dir {
                    self.dialog_input.clear();
                    self.dialog = Some(DialogState::NewFile(d));
                    return dialog_focus();
                }
                Task::none()
            }
            ExplorerMsg::NewFolder(dir) => {
                let dir = dir.or_else(|| self.explorer_target_dir());
                if let Some(d) = dir {
                    self.dialog_input.clear();
                    self.dialog = Some(DialogState::NewFolder(d));
                    return dialog_focus();
                }
                Task::none()
            }
            ExplorerMsg::Rename(p) => {
                self.dialog_input = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.dialog = Some(DialogState::RenamePath(p));
                dialog_focus()
            }
            ExplorerMsg::Trash(p) => {
                self.dialog = Some(DialogState::TrashPath(p));
                Task::none()
            }
            ExplorerMsg::CopyPath(p) => fx::clipboard_write(p.display().to_string()),
            ExplorerMsg::CopyRelative(p) => {
                let rel = self.folder.as_ref().and_then(|f| p.strip_prefix(f).ok()).unwrap_or(&p);
                fx::clipboard_write(rel.display().to_string())
            }
            ExplorerMsg::Reveal(p) => {
                let dir = if p.is_dir() { p } else { p.parent().map(PathBuf::from).unwrap_or(p) };
                let _ = open::that_detached(dir);
                Task::none()
            }
            ExplorerMsg::OpenSide(p) => {
                self.split_group();
                self.open_path(&p)
            }
            ExplorerMsg::TerminalHere(p) => {
                let dir = if p.is_dir() { p } else { p.parent().map(PathBuf::from).unwrap_or(p) };
                let id = self.new_id();
                self.terms.push(super::TermInfo { id, title: String::new() });
                self.active_term = self.terms.len() - 1;
                self.panel = Some(super::PanelView::Terminal);
                self.focus = super::Focus::Terminal;
                Task::batch([Task::effect(fx::Effect::TermSpawn { id, cwd: dir }), Task::effect(fx::Effect::TermFocus(id))])
            }
            ExplorerMsg::Refresh => {
                self.reload_explorer();
                self.index_files();
                self.refresh_git_panel();
                Task::none()
            }
            ExplorerMsg::RowMenu(i, action) => {
                let Some(row) = self.explorer.rows.get(i).cloned() else { return Task::none() };
                let dir = if row.is_dir { row.path.clone() } else { row.path.parent().map(PathBuf::from).unwrap_or_default() };
                let next = match action {
                    RowAction::OpenSide => ExplorerMsg::OpenSide(row.path),
                    RowAction::NewFile => ExplorerMsg::NewFile(Some(dir)),
                    RowAction::NewFolder => ExplorerMsg::NewFolder(Some(dir)),
                    RowAction::Rename => ExplorerMsg::Rename(row.path),
                    RowAction::Trash => ExplorerMsg::Trash(row.path),
                    RowAction::CopyPath => ExplorerMsg::CopyPath(row.path),
                    RowAction::CopyRelative => ExplorerMsg::CopyRelative(row.path),
                    RowAction::Reveal => ExplorerMsg::Reveal(row.path),
                    RowAction::Terminal => ExplorerMsg::TerminalHere(row.path),
                };
                self.on_explorer(next)
            }
            ExplorerMsg::CollapseAll => {
                self.explorer.expanded.clear();
                self.reload_explorer();
                Task::none()
            }
        }
    }

    /// A file picked in the tree. In a narrow window the tree covers the text, so it steps aside.
    fn open_from_tree(&mut self, path: &Path) -> Task {
        if self.narrow {
            self.sidebar = None;
        }
        self.open_path(path)
    }

    /// The keyboard goes to the tree, on the selected row, or the first one when the selected
    /// path isn't shown.
    pub fn focus_explorer(&mut self) {
        self.focus = Focus::Explorer;
        if let Some(i) = self.explorer_cursor() {
            self.explorer_select(i);
        }
    }

    /// The row the keyboard is on: the selected path, or the first row.
    fn explorer_cursor(&self) -> Option<usize> {
        let rows = &self.explorer.rows;
        if rows.is_empty() {
            return None;
        }
        Some(self.explorer.selected.as_ref().and_then(|p| rows.iter().position(|r| r.path == *p)).unwrap_or(0))
    }

    fn explorer_select(&mut self, i: usize) {
        if let Some(r) = self.explorer.rows.get(i) {
            self.explorer.selected = Some(r.path.clone());
        }
    }

    fn explorer_set_expanded(&mut self, path: &Path, open: bool) {
        let changed = if open { self.explorer.expanded.insert(path.to_path_buf()) } else { self.explorer.expanded.remove(path) };
        if changed {
            self.reload_explorer();
            self.mark_session_dirty();
        }
    }

    /// Keys with the file tree focused, as in a file manager's tree: the arrows move and open
    /// folders, Enter opens, F2 renames, Delete moves to the trash, a letter jumps to a name.
    /// `None` leaves the key to the shortcuts.
    pub fn explorer_key(&mut self, ev: &KeyEv) -> Option<Task> {
        if ev.mods.control() || ev.mods.alt() || ev.mods.logo() {
            return None;
        }
        if ev.name == "escape" {
            self.focus = Focus::Editor;
            return Some(fx::focus_editor());
        }
        let Some(cur) = self.explorer_cursor() else { return None };
        let last = self.explorer.rows.len() - 1;
        let row = self.explorer.rows[cur].clone();
        let page = ((self.explorer.view_h / 26.0) as usize).max(2) - 1;
        let go = |app: &mut App, i: usize| {
            app.explorer_select(i);
            Some(Task::none())
        };
        match ev.name.as_str() {
            "up" => go(self, cur.saturating_sub(1)),
            "down" => go(self, (cur + 1).min(last)),
            "home" => go(self, 0),
            "end" => go(self, last),
            "pageup" => go(self, cur.saturating_sub(page)),
            "pagedown" => go(self, (cur + page).min(last)),
            "right" if row.is_dir && !row.expanded => {
                self.explorer_set_expanded(&row.path, true);
                Some(Task::none())
            }
            // Into an open folder: its first entry, if it has any.
            "right" => match self.explorer.rows.get(cur + 1) {
                Some(next) if row.is_dir && next.depth > row.depth => go(self, cur + 1),
                _ => Some(Task::none()),
            },
            "left" if row.is_dir && row.expanded => {
                self.explorer_set_expanded(&row.path, false);
                Some(Task::none())
            }
            // Out to the folder this is in.
            "left" => match self.explorer.rows[..cur].iter().rposition(|r| r.depth < row.depth) {
                Some(parent) => go(self, parent),
                None => Some(Task::none()),
            },
            "enter" if row.is_dir => {
                self.explorer_set_expanded(&row.path, !row.expanded);
                Some(Task::none())
            }
            "enter" => Some(self.open_from_tree(&row.path)),
            // Opens a file and stays in the tree, to look through several.
            "space" if !row.is_dir => {
                let t = self.open_path(&row.path);
                self.focus = Focus::Explorer;
                Some(t)
            }
            "space" => {
                self.explorer_set_expanded(&row.path, !row.expanded);
                Some(Task::none())
            }
            "f2" => Some(self.on_explorer(ExplorerMsg::Rename(row.path))),
            "delete" => Some(self.on_explorer(ExplorerMsg::Trash(row.path))),
            // Type to jump: the next name starting with the letter, round the list.
            _ => {
                let c = ev.text.as_deref().and_then(|t| t.chars().next()).filter(|c| !c.is_control())?;
                let c = c.to_lowercase().to_string();
                let n = self.explorer.rows.len();
                let hit = (1..=n).map(|k| (cur + k) % n).find(|&i| self.explorer.rows[i].name.to_lowercase().starts_with(&c));
                match hit {
                    Some(i) => go(self, i),
                    None => Some(Task::none()),
                }
            }
        }
    }

    fn explorer_target_dir(&self) -> Option<PathBuf> {
        match &self.explorer.selected {
            Some(p) if p.is_dir() => Some(p.clone()),
            Some(p) => p.parent().map(PathBuf::from),
            None => self.folder.clone(),
        }
    }
}

pub fn dialog_focus() -> Task {
    Task::batch([fx::focus(super::DIALOG_INPUT), fx::select_all(super::DIALOG_INPUT)])
}
