//! Opening and moving around in side-by-side diffs.

use super::{App, Focus, Message};
use crate::i18n::tr;
use crate::state::{DiffData, DocEntry, TabId, TabKind};
use crate::ui::diff_view::DiffEvent;
use crate::fx::Task;
use std::path::{Path, PathBuf};
use tinta_core::diff::{hunk_starts, side_by_side};
use tinta_core::{Document, git as core_git, language};

/// Top of the view, for scroll limits: the same as the widget's title row.
const HEADER_H: f32 = 26.0;

impl App {
    /// Shows two texts side by side. `new_path` is where a double click on the right side goes.
    pub fn open_diff(&mut self, name: String, old: (&str, String), new: (&str, String), language_of: &Path, new_path: Option<PathBuf>) -> Task {
        // Comparing the same thing again replaces the old view.
        let again: Vec<TabId> = self
            .tabs
            .values()
            .filter(|t| t.kind == TabKind::Diff && self.docs.get(&t.doc).is_some_and(|e| e.doc.scratch_name == name))
            .map(|t| t.id)
            .collect();
        for id in again {
            let _ = self.close_tab(id);
        }
        let make = |text: &str, theme: &std::sync::Arc<syntect::highlighting::Theme>| {
            let mut d = Document::new(text);
            d.language = language::detect(language_of, d.rope.line(0).as_str().unwrap_or(""));
            d.highlighter = tinta_core::syntax::Highlighter::new(&d.language.name, theme.clone());
            d
        };
        let (left, right) = (make(old.0, &self.syntax_theme), make(new.0, &self.syntax_theme));
        let rows = side_by_side(old.0, new.0);
        let hunks = hunk_starts(&rows);
        let (added, removed) = tinta_core::diff::stats(&rows);

        // A tab needs a document behind it, even if nothing in it gets edited.
        let mut holder = Document::new("");
        holder.scratch_name = name;
        holder.readonly = true;
        let id = self.new_id();
        let mut entry = DocEntry::new(holder, format!("diff-{id}"));
        entry.is_read_only_view = true;
        self.docs.insert(id, entry);
        let tab = self.add_tab(id);

        let data = DiffData {
            left,
            right,
            left_title: old.1,
            right_title: new.1,
            rows,
            hunks,
            added,
            removed,
            scroll_y: 0.0,
            scroll_x: 0.0,
            view_h: 600.0,
            current: None,
            new_path,
        };
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.kind = TabKind::Diff;
            t.diff = Some(Box::new(data));
        }
        self.focus = Focus::Editor;
        self.refresh_tab_titles();
        // Straight to the first change: that's what the view was opened for.
        self.diff_step(true);
        self.on_active_changed()
    }

    pub fn active_diff_mut(&mut self) -> Option<&mut DiffData> {
        let id = self.active_tab_id()?;
        self.tabs.get_mut(&id)?.diff.as_deref_mut()
    }

    fn clamp_diff(&mut self) {
        let line_h = self.metrics.line_h;
        if let Some(d) = self.active_diff_mut() {
            let max = ((d.rows.len() as f32 - 1.0) * line_h).max(0.0);
            d.scroll_y = d.scroll_y.clamp(0.0, max);
            d.scroll_x = d.scroll_x.max(0.0);
        }
    }

    /// True if there's a diff on screen to move in.
    pub fn diff_step(&mut self, forward: bool) -> bool {
        let line_h = self.metrics.line_h;
        let Some(d) = self.active_diff_mut() else { return false };
        if d.hunks.is_empty() {
            return true;
        }
        let n = d.hunks.len();
        let i = match d.current {
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        d.current = Some(i);
        // A few rows of context above the change.
        d.scroll_y = (d.hunks[i] as f32 - 3.0).max(0.0) * line_h;
        self.clamp_diff();
        true
    }

    /// Arrow keys and friends. True if the key was for the diff.
    pub fn diff_key(&mut self, name: &str) -> bool {
        let line_h = self.metrics.line_h;
        let cell_w = self.metrics.cell_w;
        let Some(d) = self.active_diff_mut() else { return false };
        let page = ((d.view_h - HEADER_H) / line_h).max(2.0).floor() * line_h - line_h;
        match name {
            "up" => d.scroll_y -= line_h,
            "down" => d.scroll_y += line_h,
            "left" => d.scroll_x -= cell_w * 4.0,
            "right" => d.scroll_x += cell_w * 4.0,
            "pageup" => d.scroll_y -= page,
            "pagedown" | "space" => d.scroll_y += page,
            "home" => d.scroll_y = 0.0,
            "end" => d.scroll_y = f32::MAX / 2.0,
            _ => return false,
        }
        self.clamp_diff();
        true
    }

    pub fn on_diff_event(&mut self, tab: TabId, ev: DiffEvent) -> Task {
        match ev {
            DiffEvent::Viewport { w, h, cell_w } => {
                if self.metrics.cell_w != cell_w {
                    self.metrics.cell_w = cell_w;
                }
                if let Some(t) = self.tabs.get_mut(&tab) {
                    t.view_w = w;
                    t.view_h = h;
                    if let Some(d) = &mut t.diff {
                        d.view_h = h;
                    }
                }
            }
            DiffEvent::Scroll { dx, dy } => {
                if let Some(d) = self.tabs.get_mut(&tab).and_then(|t| t.diff.as_deref_mut()) {
                    d.scroll_y += dy;
                    d.scroll_x += dx;
                }
                self.clamp_diff();
            }
            DiffEvent::ScrollTo { y } => {
                if let Some(d) = self.tabs.get_mut(&tab).and_then(|t| t.diff.as_deref_mut()) {
                    d.scroll_y = y;
                }
                self.clamp_diff();
            }
            DiffEvent::OpenRight { line } => {
                let path = self.tabs.get(&tab).and_then(|t| t.diff.as_ref()).and_then(|d| d.new_path.clone());
                if let Some(p) = path {
                    return self.open_at(&p, line, 0);
                }
            }
        }
        Task::none()
    }

    /// What a file's text is right now: the open buffer's, unsaved changes included, or the disk's.
    fn current_text(&self, path: &Path) -> String {
        match self.doc_by_path(path).and_then(|id| self.docs.get(&id)) {
            Some(e) => e.doc.rope.to_string(),
            None => std::fs::read_to_string(path).unwrap_or_default(),
        }
    }

    /// From the Source Control list: what's staged against HEAD, or the rest against what's staged.
    pub fn open_git_diff(&mut self, root: PathBuf, rel: String, staged: bool) -> Task {
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        let path = root.join(&rel);
        let (old, new) = if staged {
            let head = core_git::file_at_head(&root, &rel).unwrap_or_default();
            let index = core_git::file_at_index(&root, &rel).unwrap_or_default();
            ((head, "HEAD".to_string()), (index, tr("Staged").to_string()))
        } else {
            let base = core_git::file_at_index(&root, &rel).unwrap_or_default();
            ((base, tr("Staged").to_string()), (self.current_text(&path), tr("Working tree").to_string()))
        };
        let title = format!("{name} ({})", if staged { tr("Staged") } else { tr("Working tree") });
        self.open_diff(title, (&old.0, old.1), (&new.0, new.1), &path, Some(path.clone()))
    }

    /// The active file against what's committed.
    pub fn show_file_diff(&mut self) -> Task {
        let Some((_, e)) = self.active() else { return Task::none() };
        let Some((root, rel)) = e.git_rel.clone() else { return self.toast(tr("Not a git repository")) };
        let text = e.doc.rope.to_string();
        let name = e.doc.file_name();
        let path = root.join(&rel);
        let head = core_git::file_at_head(&root, &rel).unwrap_or_default();
        self.open_diff(format!("{name} (HEAD)"), (&head, "HEAD".to_string()), (&text, tr("Working tree").to_string()), &path, Some(path.clone()))
    }

    pub fn compare_with_saved(&mut self) -> Task {
        let Some((_, e)) = self.active() else { return Task::none() };
        let Some(p) = e.doc.path.clone() else { return Task::none() };
        let now = e.doc.rope.to_string();
        let name = e.doc.file_name();
        let saved = std::fs::read_to_string(&p).unwrap_or_default();
        self.open_diff(format!("{name} ({})", tr("Saved")), (&saved, tr("Saved").to_string()), (&now, tr("Unsaved changes").to_string()), &p, Some(p.clone()))
    }
}
