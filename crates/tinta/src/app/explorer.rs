//! The file tree.

use super::{App, DialogState, Message};
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

fn list_dir(dir: &Path, hidden: bool) -> Vec<(String, PathBuf, bool)> {
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
    items.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
    items
}

impl App {
    pub fn reload_explorer(&mut self) {
        let Some(root) = self.folder.clone() else {
            self.explorer.rows.clear();
            return;
        };
        let hidden = self.settings.show_hidden_files;
        let mut rows = Vec::new();
        self.flatten_into(&mut rows, list_dir(&root, hidden), 0);
        self.explorer.rows = rows;
        self.update_watches();
    }

    /// Depth first, walking down into the expanded folders.
    fn flatten_into(&self, rows: &mut Vec<Row>, items: Vec<(String, PathBuf, bool)>, depth: usize) {
        let hidden = self.settings.show_hidden_files;
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
                self.flatten_into(rows, list_dir(&path, hidden), depth + 1);
            }
        }
    }

    pub fn on_explorer(&mut self, m: ExplorerMsg) -> Task {
        match m {
            ExplorerMsg::Click(i) => {
                let Some(row) = self.explorer.rows.get(i).cloned() else { return Task::none() };
                self.explorer.selected = Some(row.path.clone());
                if row.is_dir {
                    if !self.explorer.expanded.remove(&row.path) {
                        self.explorer.expanded.insert(row.path);
                    }
                    self.reload_explorer();
                    self.mark_session_dirty();
                    return Task::none();
                }
                self.open_path(&row.path)
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
