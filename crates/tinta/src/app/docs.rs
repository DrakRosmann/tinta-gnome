//! Opening, saving and closing; the session; the housekeeping that runs after every message.

use super::{App, DialogState, Focus, Message, PanelView};
use crate::bus;
use crate::i18n::tr;
use crate::state::{DocEntry, DocId, Group, Tab, TabId, TabKind};
use crate::fx::{self, Task};
use notify::Watcher;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tinta_core::buffer::{ChangeRec, Indent};
use tinta_core::session::{self, GroupState, Session, TabState};
use tinta_core::settings::AutoSave;
use tinta_core::syntax::Highlighter;
use tinta_core::{Cursors, Document, Selection, commands, display, git as core_git};

/// Sets up what depends on the language: colours, and the syntax tree where there's a parser.
pub fn attach_language(doc: &mut Document, theme: Arc<syntect::highlighting::Theme>) {
    doc.highlighter = Highlighter::new(&doc.language.name, theme);
    let name = doc.language.name.clone();
    doc.structure = (doc.len_chars() <= tinta_core::structure::MAX_CHARS).then(|| tinta_core::structure::Structure::new(&name)).flatten();
    doc.sync_structure();
}

/// Up to this size the syntax tree follows every key. Past it, it waits for a pause.
const SYNC_ON_EVERY_EDIT: usize = 200_000;

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

impl App {
    pub fn default_indent(&self) -> Indent {
        Indent { spaces: self.settings.insert_spaces, width: self.settings.tab_size }
    }

    pub fn doc_by_path(&self, path: &Path) -> Option<DocId> {
        let target = canonical(path);
        self.docs.iter().find(|(_, e)| e.doc.path.as_deref() == Some(target.as_path())).map(|(k, _)| *k)
    }

    fn make_entry(&mut self, mut doc: Document) -> DocId {
        attach_language(&mut doc, self.syntax_theme.clone());
        let id = self.new_id();
        let key = session::backup_key(doc.path.as_deref(), id);
        let entry = DocEntry::new(doc, key);
        self.docs.insert(id, entry);
        id
    }

    /// A new tab in the active group, showing `doc`.
    pub fn add_tab(&mut self, doc: DocId) -> TabId {
        let id = self.new_id();
        let entry = self.docs.get(&doc).expect("document exists");
        let tab = Tab::new(id, doc, entry);
        self.tabs.insert(id, tab);
        let title = self.tab_title(id);
        let group = &mut self.groups[self.active_group];
        let builder = group.model.insert().text(title).data::<TabId>(id).closable();
        builder.activate();
        id
    }

    pub fn tab_title(&self, id: TabId) -> String {
        let Some(t) = self.tabs.get(&id) else { return String::new() };
        let Some(e) = self.docs.get(&t.doc) else { return String::new() };
        let mut s = String::new();
        if t.pinned {
            s.push_str("📌 ");
        }
        if t.kind == TabKind::Preview {
            s.push_str(&format!("{} · {}", tr("Rendered preview"), e.doc.file_name()));
            return s;
        }
        if t.kind == TabKind::Diff {
            s.push_str("⇄ ");
        }
        if e.doc.is_dirty() && !e.is_read_only_view {
            s.push_str("● ");
        }
        s.push_str(&e.doc.file_name());
        s
    }

    pub fn refresh_tab_titles(&mut self) {
        let ids: Vec<TabId> = self.tabs.keys().copied().collect();
        for id in ids {
            let title = self.tab_title(id);
            for g in &mut self.groups {
                if let Some(e) = g.entity_of(id)
                    && g.model.text(e) != Some(title.as_str())
                {
                    g.model.text_set(e, title.clone());
                }
            }
        }
    }

    pub fn new_scratch(&mut self) -> Task {
        let mut doc = Document::new("");
        doc.indent = self.default_indent();
        let n = self.docs.values().filter(|e| e.doc.path.is_none() && !e.is_read_only_view).count() + 1;
        doc.scratch_name = format!("{} {n}", if self.pt() { "Sem título" } else { "Untitled" });
        let id = self.make_entry(doc);
        self.add_tab(id);
        self.focus = Focus::Editor;
        self.on_active_changed()
    }

    /// A read-only buffer for generated text: diffs, logs.
    pub fn open_generated(&mut self, name: String, text: &str, language: &str) -> Task {
        let mut doc = Document::new(text);
        doc.scratch_name = name;
        doc.readonly = true;
        doc.language = tinta_core::language::by_name(language);
        let id = self.make_entry(doc);
        if let Some(e) = self.docs.get_mut(&id) {
            e.is_read_only_view = true;
        }
        self.add_tab(id);
        self.focus = Focus::Editor;
        self.on_active_changed()
    }

    pub fn open_path(&mut self, path: &Path) -> Task {
        if path.is_dir() {
            return self.open_folder(path.to_path_buf());
        }
        let path = canonical(path);
        self.remember_recent_file(&path);
        // Already open: go there, preferring the active group.
        if let Some(doc) = self.doc_by_path(&path) {
            let order: Vec<usize> = std::iter::once(self.active_group).chain(0..self.groups.len()).collect();
            for g in order {
                let found = self.groups[g].tabs().into_iter().find(|t| self.tabs.get(t).is_some_and(|x| x.doc == doc && x.kind == TabKind::Text));
                if let Some(tab) = found {
                    self.active_group = g;
                    let e = self.groups[g].entity_of(tab).unwrap();
                    self.groups[g].model.activate(e);
                    self.focus = Focus::Editor;
                    return self.on_active_changed();
                }
            }
            self.add_tab(doc);
            self.focus = Focus::Editor;
            return self.on_active_changed();
        }
        let opened = if path.exists() {
            Document::open(&path, self.default_indent())
        } else {
            // A path that doesn't exist yet starts empty and appears on first save.
            let mut d = Document::new("");
            d.path = Some(path.clone());
            d.language = tinta_core::language::detect(&path, "");
            d.indent = self.default_indent();
            Ok(d)
        };
        let doc = match opened {
            Ok(mut d) => {
                if !self.settings.detect_indentation {
                    d.indent = self.default_indent();
                }
                d
            }
            Err(e) => {
                let why = if e.kind() == std::io::ErrorKind::InvalidData { tr("This looks like a binary file").to_string() } else { e.to_string() };
                return self.toast(format!("{} {}: {why}", tr("Could not open"), path.display()));
            }
        };
        let id = self.make_entry(doc);
        self.add_tab(id);
        self.focus = Focus::Editor;
        self.watch_parent(&path);
        self.refresh_git_for(id);
        self.attach_task_problems_to(&path);
        let t = self.lsp_open_doc(id);
        Task::batch([t, self.on_active_changed()])
    }

    fn remember_recent_file(&mut self, path: &Path) {
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_path_buf());
        self.recent_files.truncate(40);
    }

    pub fn open_folder(&mut self, path: PathBuf) -> Task {
        let path = canonical(&path);
        self.recent_folders.retain(|p| *p != path);
        self.recent_folders.insert(0, path.clone());
        self.recent_folders.truncate(20);
        self.folder = Some(path.clone());
        self.explorer = super::Explorer::default();
        self.explorer.expanded.insert(path.clone());
        self.sidebar = Some(super::SidebarView::Explorer);
        self.reload_explorer();
        self.index_files();
        self.refresh_git_panel();
        self.update_watches();
        self.mark_session_dirty();
        self.update_title()
    }

    pub fn close_folder(&mut self) -> Task {
        self.folder = None;
        self.explorer = super::Explorer::default();
        self.file_index = Arc::new(Vec::new());
        self.git = super::GitPanel::default();
        self.update_watches();
        self.update_title()
    }

    pub fn index_files(&mut self) {
        let Some(root) = self.folder.clone() else { return };
        self.file_index_loading = true;
        let hidden = self.settings.show_hidden_files;
        std::thread::spawn(move || {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let files = tinta_core::find::list_files(&root, hidden, &cancel, 200_000);
            bus::send(Message::Palette(super::palette::PaletteMsg::FilesIndexed(Arc::new(files))));
        });
    }

    pub fn update_title(&mut self) -> Task {
        let name = self.active().map(|(_, e)| {
            let dirty = if e.doc.is_dirty() && !e.is_read_only_view { "● " } else { "" };
            format!("{dirty}{}", e.doc.file_name())
        });
        let folder = self.folder.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned());
        let title = match (name, folder) {
            (Some(n), Some(f)) => format!("{n} — {f} — Tinta"),
            (Some(n), None) => format!("{n} — Tinta"),
            (None, Some(f)) => format!("{f} — Tinta"),
            (None, None) => "Tinta".to_string(),
        };
        if title == self.title {
            return Task::none();
        }
        self.title = title;
        Task::none()
    }

    // Saving -------------------------------------------------------------------------------

    pub fn save_active(&mut self) -> Task {
        let Some(id) = self.active_tab_id() else { return Task::none() };
        self.save_tab(id)
    }

    pub fn save_tab(&mut self, id: TabId) -> Task {
        let Some(t) = self.tabs.get(&id) else { return Task::none() };
        let doc_id = t.doc;
        let Some(e) = self.docs.get(&doc_id) else { return Task::none() };
        if e.is_read_only_view {
            return Task::none();
        }
        let Some(path) = e.doc.path.clone() else { return self.save_as_dialog(id) };
        if self.settings.format_on_save && self.lsp_format_then_save(id) {
            return Task::none();
        }
        self.finish_save(id, path)
    }

    pub fn save_as_dialog(&mut self, id: TabId) -> Task {
        let name = self.tabs.get(&id).and_then(|t| self.docs.get(&t.doc)).map(|e| e.doc.file_name()).unwrap_or_default();
        let dir = self.folder.clone();
        Task::effect(fx::Effect::SaveAs { name, folder: dir, then: Box::new(move |p| Message::SaveAsTo(id, p)) })
    }

    pub fn save_tab_as(&mut self, id: TabId, path: PathBuf) -> Task {
        let Some(doc_id) = self.tabs.get(&id).map(|t| t.doc) else { return Task::none() };
        if let Some(e) = self.docs.get_mut(&doc_id) {
            session::remove_backup(&e.backup_key);
            e.doc.path = Some(path.clone());
            e.doc.language = tinta_core::language::detect(&path, e.doc.rope.line(0).as_str().unwrap_or(""));
        }
        if let Some(e) = self.docs.get_mut(&doc_id) {
            attach_language(&mut e.doc, self.syntax_theme.clone());
        }
        let t = self.finish_save(id, path.clone());
        let t2 = self.lsp_open_doc(doc_id);
        self.watch_parent(&path);
        self.remember_recent_file(&path);
        Task::batch([t, t2])
    }

    /// The write itself, after any formatting. Applies the on-save cleanups.
    pub fn finish_save(&mut self, id: TabId, path: PathBuf) -> Task {
        let trim = self.settings.trim_trailing_whitespace;
        let newline = self.settings.insert_final_newline;
        let Some(t) = self.tabs.get_mut(&id) else { return Task::none() };
        let Some(e) = self.docs.get_mut(&t.doc) else { return Task::none() };
        if trim {
            commands::trim_trailing(&mut e.doc, &mut t.cursors);
        }
        if newline {
            commands::ensure_final_newline(&mut e.doc, &mut t.cursors);
        }
        let doc_id = t.doc;
        match e.doc.save_to(&path) {
            Ok(()) => {
                e.changed_on_disk = false;
                e.git_stale = true;
                session::remove_backup(&e.backup_key);
                self.edit_tab = Some(id);
                self.lsp_did_save(doc_id);
                self.refresh_git_for(doc_id);
                self.refresh_git_panel();
                let settings_saved = path == tinta_core::settings::settings_path();
                let keys_saved = path == tinta_core::settings::keybindings_path();
                let msg = format!("{} {}", tr("Saved"), path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                let mut tasks = vec![self.toast(msg), self.update_title()];
                if settings_saved || keys_saved {
                    tasks.push(self.reload_config_files());
                }
                Task::batch(tasks)
            }
            Err(err) => self.toast(format!("{}: {err}", tr("Could not save"))),
        }
    }

    pub fn save_all_dirty_named(&mut self) -> Task {
        let ids: Vec<TabId> = self
            .tabs
            .iter()
            .filter(|(_, t)| t.kind == TabKind::Text && self.docs.get(&t.doc).is_some_and(|e| e.doc.is_dirty() && e.doc.path.is_some() && !e.is_read_only_view))
            .map(|(k, _)| *k)
            .collect();
        let mut seen = std::collections::HashSet::new();
        let mut tasks = Vec::new();
        for id in ids {
            let doc = self.tabs[&id].doc;
            if seen.insert(doc)
                && let Some(p) = self.docs[&doc].doc.path.clone()
            {
                tasks.push(self.finish_save(id, p));
            }
        }
        Task::batch(tasks)
    }

    // Closing ------------------------------------------------------------------------------

    pub fn request_close_tab(&mut self, id: TabId) -> Task {
        let Some(t) = self.tabs.get(&id) else { return Task::none() };
        let doc = t.doc;
        let other_views = self.tabs.values().filter(|o| o.doc == doc && o.id != id).count();
        let dirty = self.docs.get(&doc).is_some_and(|e| e.doc.is_dirty() && !e.is_read_only_view);
        if dirty && other_views == 0 {
            self.dialog = Some(DialogState::CloseUnsaved(id));
            return Task::none();
        }
        self.close_tab(id)
    }

    pub fn close_tab(&mut self, id: TabId) -> Task {
        let Some(tab) = self.tabs.remove(&id) else { return Task::none() };
        let g_idx = self.groups.iter().position(|g| g.entity_of(id).is_some());
        if let Some(g_idx) = g_idx {
            let g = &mut self.groups[g_idx];
            if let Some(ent) = g.entity_of(id) {
                let pos = g.model.position(ent).unwrap_or(0);
                let was_active = g.model.active() == ent;
                g.model.remove(ent);
                if was_active && g.model.len() > 0 {
                    let next = pos.min(g.model.len() as u16 - 1);
                    g.model.activate_position(next);
                }
            }
            // An empty group goes away, unless it's the only one left.
            if g.model.len() == 0 && self.groups.len() > 1 {
                self.groups.remove(g_idx);
                self.groups_changed.set(true);
                if self.active_group >= g_idx && self.active_group > 0 {
                    self.active_group -= 1;
                }
            }
        }
        let doc_id = tab.doc;
        if !self.tabs.values().any(|t| t.doc == doc_id) {
            if let Some(e) = self.docs.remove(&doc_id) {
                session::remove_backup(&e.backup_key);
                if let Some(p) = &e.doc.path {
                    if !e.is_read_only_view {
                        self.closed_tabs.push(p.clone());
                    }
                    self.lsp_close_doc(p, e.lsp_key.as_deref());
                }
            }
        }
        self.close_popups();
        self.update_watches();
        self.mark_session_dirty();
        self.on_active_changed()
    }

    pub fn close_tabs_where(&mut self, keep: impl Fn(TabId, &Tab) -> bool) -> Task {
        let ids: Vec<TabId> = self.tabs.iter().filter(|(k, t)| !keep(**k, t) && !t.pinned).map(|(k, _)| *k).collect();
        // Clean ones just go; the first dirty one asks, and the user can repeat.
        let mut tasks = Vec::new();
        for id in ids {
            let dirty = self.tabs.get(&id).and_then(|t| self.docs.get(&t.doc)).is_some_and(|e| e.doc.is_dirty() && !e.is_read_only_view);
            if dirty {
                self.dialog = Some(DialogState::CloseUnsaved(id));
                break;
            }
            tasks.push(self.close_tab(id));
        }
        Task::batch(tasks)
    }

    pub fn split_group(&mut self) {
        let doc = self.active().map(|(t, _)| (t.doc, t.cursors.clone(), t.scroll_y));
        let mut g = Group::new();
        g.weight = self.groups[self.active_group].weight;
        self.groups.insert(self.active_group + 1, g);
        self.groups_changed.set(true);
        self.active_group += 1;
        if let Some((d, c, y)) = doc {
            let id = self.add_tab(d);
            if let Some(t) = self.tabs.get_mut(&id) {
                t.cursors = c;
                t.scroll_y = y;
            }
        }
        self.mark_session_dirty();
    }

    pub fn request_quit(&mut self) -> Task {
        let dirty = self.docs.values().any(|e| e.doc.is_dirty() && !e.is_read_only_view);
        // A separate instance keeps no backups, so its unsaved work has to be asked about.
        if dirty && (!self.settings.hot_exit || self.new_session) {
            self.dialog = Some(DialogState::QuitUnsaved);
            return Task::none();
        }
        self.quit_now()
    }

    pub fn quit_now(&mut self) -> Task {
        self.persist_on_quit();
        Task::effect(fx::Effect::Quit)
    }

    /// Only the main instance owns the session and the backups.
    pub fn persist_on_quit(&mut self) {
        self.debug_shutdown();
        if !self.new_session {
            self.write_backups(true);
            super::save_session(self);
        }
    }

    // Session ------------------------------------------------------------------------------

    pub fn build_session(&self) -> Session {
        let mut groups = Vec::new();
        for g in &self.groups {
            let mut tabs = Vec::new();
            for id in g.tabs() {
                let Some(t) = self.tabs.get(&id) else { continue };
                let Some(e) = self.docs.get(&t.doc) else { continue };
                if e.is_read_only_view || t.kind != TabKind::Text {
                    continue;
                }
                tabs.push(TabState {
                    path: e.doc.path.clone(),
                    cursors: t.cursors.clone(),
                    scroll_y: t.scroll_y,
                    folds: t.folds.clone(),
                    pinned: t.pinned,
                    backup: e.doc.is_dirty().then(|| e.backup_key.clone()),
                    scratch_name: e.doc.scratch_name.clone(),
                    language: Some(e.doc.language.name.clone()),
                });
            }
            let active = g.active_tab().and_then(|a| g.tabs().iter().position(|t| *t == a)).unwrap_or(0);
            groups.push(GroupState { tabs, active, weight: g.weight });
        }
        Session {
            folder: self.folder.clone(),
            groups,
            active_group: self.active_group,
            // Hidden only because the window is narrow, it still counts as open.
            sidebar_visible: if self.narrow { self.sidebar_wide.is_some() } else { self.sidebar.is_some() },
            sidebar_width: self.sidebar_w,
            panel_height: self.panel_h,
            recent_files: self.recent_files.clone(),
            recent_folders: self.recent_folders.clone(),
            expanded: self.explorer.expanded.iter().cloned().collect(),
            breakpoints: self.debug.breakpoints.clone(),
            watches: self.debug.watches.clone(),
        }
    }

    pub fn restore(&mut self, targets: Vec<(PathBuf, Option<(usize, usize)>)>) -> Task {
        let mut tasks = Vec::new();
        let saved = (!self.new_session && self.settings.restore_session).then(Session::load).flatten();
        if let Some(s) = &saved {
            self.recent_files = s.recent_files.clone();
            self.recent_folders = s.recent_folders.clone();
            if s.sidebar_width > 0.0 {
                self.sidebar_w = s.sidebar_width;
            }
            if s.panel_height > 0.0 {
                self.panel_h = s.panel_height;
            }
        }
        // Files on the command line open on top: skipping the session would lose its unsaved work.
        if let Some(s) = saved {
            self.debug.breakpoints = s.breakpoints.clone();
            self.debug.breakpoints.retain(|p, _| p.is_file());
            self.debug.watches = s.watches.clone();
            if let Some(f) = s.folder.clone().filter(|f| f.is_dir()) {
                tasks.push(self.open_folder(f));
                for p in &s.expanded {
                    if p.is_dir() {
                        self.explorer.expanded.insert(p.clone());
                    }
                }
                self.reload_explorer();
            }
            if !s.sidebar_visible {
                self.sidebar = None;
            }
            for (i, gs) in s.groups.iter().enumerate() {
                if i > 0 {
                    self.groups.push(Group::new());
                }
                self.active_group = i;
                self.groups[i].weight = if gs.weight > 0.0 { gs.weight } else { 1.0 };
                for ts in &gs.tabs {
                    tasks.push(self.restore_tab(ts));
                }
                if self.groups[i].model.len() > 0 {
                    let pos = gs.active.min(self.groups[i].model.len() - 1) as u16;
                    self.groups[i].model.activate_position(pos);
                }
            }
            self.groups_changed.set(true);
            self.groups.retain(|g| g.model.len() > 0);
            if self.groups.is_empty() {
                self.groups.push(Group::new());
            }
            self.active_group = s.active_group.min(self.groups.len() - 1);
        }
        for (path, at) in targets {
            if path.is_dir() {
                tasks.push(self.open_folder(path));
            } else {
                tasks.push(self.open_path(&path));
                if let Some((l, c)) = at {
                    self.jump_to(l, c);
                }
            }
        }
        tasks.push(self.on_active_changed());
        Task::batch(tasks)
    }

    fn restore_tab(&mut self, ts: &TabState) -> Task {
        let mut task = Task::none();
        let backup = ts.backup.as_deref().and_then(session::read_backup);
        let doc_id = match &ts.path {
            Some(p) => {
                if let Some(d) = self.doc_by_path(p) {
                    Some(d)
                } else if p.exists() || backup.is_some() {
                    let mut doc = Document::open(p, self.default_indent()).unwrap_or_else(|_| {
                        let mut d = Document::new("");
                        d.path = Some(p.clone());
                        d.language = tinta_core::language::detect(p, "");
                        d
                    });
                    if let Some(text) = &backup {
                        let mut c = Cursors::default();
                        doc.replace_all(text, &mut c);
                        // The tab's carets are saved for this text: the edit that put it there
                        // isn't news to them, or they'd be moved again by its length.
                        let _ = doc.take_log();
                    }
                    let id = self.make_entry(doc);
                    self.watch_parent(p);
                    self.refresh_git_for(id);
                    task = self.lsp_open_doc(id);
                    Some(id)
                } else {
                    None
                }
            }
            None => backup.as_ref().map(|text| {
                let mut doc = Document::new("");
                doc.indent = self.default_indent();
                doc.scratch_name = ts.scratch_name.clone();
                if let Some(l) = &ts.language {
                    doc.language = tinta_core::language::by_name(l);
                }
                let mut c = Cursors::default();
                doc.replace_all(text, &mut c);
                let _ = doc.take_log();
                self.make_entry(doc)
            }),
        };
        let Some(doc_id) = doc_id else { return task };
        let id = self.add_tab(doc_id);
        if let Some(t) = self.tabs.get_mut(&id) {
            let n = self.docs[&doc_id].doc.len_chars();
            t.cursors = ts.cursors.clone();
            t.cursors.clamp(n);
            t.scroll_y = ts.scroll_y;
            t.folds = ts.folds.clone();
            t.pinned = ts.pinned;
        }
        task
    }

    /// Copies unsaved text to the state folder, so a crash or quit loses nothing.
    pub fn write_backups(&mut self, prune: bool) {
        let mut keep = Vec::new();
        if self.settings.hot_exit {
            for e in self.docs.values_mut() {
                if e.is_read_only_view || !e.doc.is_dirty() {
                    continue;
                }
                keep.push(e.backup_key.clone());
                if e.backup_rev != e.doc.revision {
                    if let Err(err) = session::write_backup(&e.backup_key, &e.doc.rope.to_string()) {
                        log::warn!("backup failed: {err}");
                    }
                    e.backup_rev = e.doc.revision;
                }
            }
        }
        if prune {
            session::prune_backups(&keep);
        }
    }

    // File watching ------------------------------------------------------------------------

    fn ensure_watcher(&mut self) {
        if self.watcher.is_some() {
            return;
        }
        let w = notify::recommended_watcher(|res: notify::Result<notify::Event>| {
            if let Ok(ev) = res
                && !matches!(ev.kind, notify::EventKind::Access(_))
            {
                bus::send(Message::FsEvent(ev.paths));
            }
        });
        match w {
            Ok(w) => self.watcher = Some(w),
            Err(e) => log::warn!("file watching is off: {e}"),
        }
    }

    fn watch_parent(&mut self, path: &Path) {
        if let Some(dir) = path.parent() {
            self.watch_dir(dir);
        }
    }

    fn watch_dir(&mut self, dir: &Path) {
        if self.watched.iter().any(|d| d == dir) {
            return;
        }
        self.ensure_watcher();
        if let Some(w) = &mut self.watcher
            && w.watch(dir, notify::RecursiveMode::NonRecursive).is_ok()
        {
            self.watched.push(dir.to_path_buf());
        }
    }

    /// Watches open files' folders and the expanded folders of the tree, nothing more: recursive
    /// watches on a repository with a big build folder run out of inotify handles.
    pub fn update_watches(&mut self) {
        let mut want: Vec<PathBuf> = self.explorer.expanded.iter().cloned().collect();
        for e in self.docs.values() {
            if let Some(d) = e.doc.path.as_deref().and_then(Path::parent) {
                want.push(d.to_path_buf());
            }
        }
        if let Some(f) = &self.folder {
            want.push(f.join(".git"));
        }
        want.sort();
        want.dedup();
        let stale: Vec<PathBuf> = self.watched.iter().filter(|d| !want.contains(d)).cloned().collect();
        for d in stale {
            if let Some(w) = &mut self.watcher {
                let _ = w.unwatch(&d);
            }
            self.watched.retain(|x| *x != d);
        }
        for d in want {
            if d.is_dir() {
                self.watch_dir(&d);
            }
        }
    }

    pub fn on_fs_event(&mut self, paths: Vec<PathBuf>) -> Task {
        let mut tree_changed = false;
        let mut git_changed = false;
        let mut tasks = Vec::new();
        for p in &paths {
            let in_git = p.components().any(|c| c.as_os_str() == ".git");
            if in_git {
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                // Ignore git's own lock files, and our own status calls.
                if matches!(name, "HEAD" | "index" | "ORIG_HEAD" | "MERGE_HEAD") {
                    git_changed = true;
                }
                continue;
            }
            if p.file_name().and_then(|s| s.to_str()).is_some_and(|n| n.contains(".tinta-")) {
                continue;
            }
            tree_changed = true;
            git_changed = true;
            let Some(id) = self.doc_by_path(p) else { continue };
            let Some(e) = self.docs.get_mut(&id) else { continue };
            let mtime = super::file_mtime(p);
            if mtime.is_none() || mtime == e.doc.disk_mtime {
                continue;
            }
            if e.doc.is_dirty() {
                e.changed_on_disk = true;
                if self.dialog.is_none() {
                    self.dialog = Some(DialogState::ChangedOnDisk(id));
                }
            } else {
                tasks.push(self.reload_doc(id));
            }
        }
        if tree_changed {
            self.reload_explorer();
        }
        if git_changed {
            self.refresh_git_panel();
            let ids: Vec<DocId> = self.docs.keys().copied().collect();
            for id in ids {
                self.refresh_git_for(id);
            }
        }
        Task::batch(tasks)
    }

    pub fn reload_doc(&mut self, id: DocId) -> Task {
        let tab = self.tabs.values_mut().find(|t| t.doc == id);
        let Some(e) = self.docs.get_mut(&id) else { return Task::none() };
        let mut c = tab.as_ref().map(|t| t.cursors.clone()).unwrap_or_default();
        if let Err(err) = e.doc.reload(&mut c) {
            return self.toast(format!("{}: {err}", tr("Could not open")));
        }
        e.changed_on_disk = false;
        if let Some(t) = tab {
            t.cursors = c;
            self.edit_tab = Some(t.id);
        }
        self.refresh_git_for(id);
        Task::none()
    }

    // After every message -------------------------------------------------------------------

    pub fn after_update(&mut self) -> Task {
        let mut tasks = Vec::new();
        let mut any_change = false;
        let doc_ids: Vec<DocId> = self.docs.keys().copied().collect();
        for id in doc_ids {
            let log = self.docs.get_mut(&id).map(|e| e.doc.take_log()).unwrap_or_default();
            if log.is_empty() {
                continue;
            }
            any_change = true;
            self.apply_change_log(id, &log);
            if let Some(e) = self.docs.get_mut(&id)
                && e.doc.len_chars() <= SYNC_ON_EVERY_EDIT
            {
                e.doc.sync_structure();
            }
        }
        let (visible, wrap_changed) = self.sync_visible_tabs();
        let _ = wrap_changed;
        if self.want_reveal {
            self.want_reveal = false;
            self.reveal_active();
        }
        for id in visible {
            self.clamp_scroll(id);
        }
        if any_change {
            self.advance_highlighting(Duration::from_millis(3));
            self.refresh_tab_titles();
            self.after_caret_move();
            self.mark_session_dirty();
            self.refresh_find_matches();
            self.refresh_previews();
            tasks.push(self.update_title());
        } else {
            self.refresh_find_matches();
        }
        Task::batch(tasks)
    }

    fn apply_change_log(&mut self, id: DocId, log: &[ChangeRec]) {
        let editor = self.edit_tab.take();
        let now = Instant::now();
        let map_pos = |mut pos: usize, log: &[ChangeRec]| {
            for c in log {
                let old_end = c.at + c.removed;
                if pos >= old_end {
                    pos = pos + c.inserted - c.removed;
                } else if pos > c.at {
                    pos = c.at + c.inserted;
                }
            }
            pos
        };
        for t in self.tabs.values_mut().filter(|t| t.doc == id) {
            for c in log {
                t.folds.on_change(c);
                if let Some(s) = &mut t.snippet
                    && !s.on_change(c)
                {
                    t.snippet = None;
                }
            }
            if Some(t.id) != editor {
                t.cursors.map(|s| Selection { anchor: map_pos(s.anchor, log), head: map_pos(s.head, log), goal: s.goal });
            }
            t.expand_stack.clear();
            t.caret_stamp += 1;
        }
        let Some(e) = self.docs.get_mut(&id) else { return };
        for d in &mut e.diags {
            d.start = map_pos(d.start, log);
            d.end = map_pos(d.end, log);
        }
        for c in log {
            if c.inserted_lines != c.removed_lines {
                e.bookmarks = e.bookmarks.iter().filter_map(|l| c.map_line(*l)).collect();
            }
        }
        if let Some(path) = e.doc.path.clone() {
            for c in log.iter().filter(|c| c.inserted_lines != c.removed_lines) {
                self.debug_follow_edit(&path, c);
            }
        }
        let Some(e) = self.docs.get_mut(&id) else { return };
        e.git_stale = true;
        e.lsp_dirty = true;
        e.last_edit = Some(now);
        e.matches_key = None;
        e.symbols_rev = None;
        e.blame = None;
        // Find-in-selection ranges follow the text.
        if let Some(ranges) = &mut self.find.in_selection {
            for r in ranges {
                r.0 = map_pos(r.0, log);
                r.1 = map_pos(r.1, log);
            }
        }
    }

    /// Display maps and wrap widths for what's on screen.
    fn sync_visible_tabs(&mut self) -> (Vec<TabId>, bool) {
        let mut out = Vec::new();
        let wrap_on = self.settings.word_wrap;
        let settings = self.settings.clone();
        let metrics = self.metrics;
        let mut changed = false;
        for g in &self.groups {
            let Some(id) = g.active_tab() else { continue };
            let Some(t) = self.tabs.get_mut(&id) else { continue };
            let Some(e) = self.docs.get(&t.doc) else { continue };
            let geo = crate::ui::editor_view::Geometry::new(e.doc.len_lines(), t.view_w, &metrics, &settings);
            // Wrapping a giant file on every key would be noticeable: such files stay unwrapped.
            let wrap = (wrap_on && e.doc.len_chars() < 4_000_000).then(|| geo.wrap_cols(&metrics));
            let before = t.map.wrap;
            t.map.update(&e.doc, &t.folds, wrap, e.doc.indent.width.max(1));
            changed |= before != wrap;
            out.push(id);
        }
        (out, changed)
    }

    pub fn clamp_scroll(&mut self, id: TabId) {
        let metrics = self.metrics;
        let Some(t) = self.tabs.get_mut(&id) else { return };
        let Some(e) = self.docs.get(&t.doc) else { return };
        let max = {
            let rows = t.map.row_count(&e.doc) as f32;
            ((rows - 1.0) * metrics.line_h).max(0.0)
        };
        t.scroll_y = t.scroll_y.clamp(0.0, max);
        t.scroll_x = t.scroll_x.max(0.0);
        if t.map.wrap.is_some() {
            t.scroll_x = 0.0;
        }
    }

    pub fn advance_highlighting(&mut self, budget: Duration) {
        let shown_diffs: Vec<TabId> = self.groups.iter().filter_map(Group::active_tab).collect();
        for id in shown_diffs {
            if let Some(d) = self.tabs.get_mut(&id).and_then(|t| t.diff.as_deref_mut()) {
                for side in [&mut d.left, &mut d.right] {
                    if let Some(h) = &mut side.highlighter {
                        h.advance(&side.rope, budget);
                    }
                }
            }
        }
        let visible: Vec<DocId> = self.groups.iter().filter_map(|g| g.active_tab().and_then(|t| self.tabs.get(&t)).map(|t| t.doc)).collect();
        for id in visible {
            if let Some(e) = self.docs.get_mut(&id)
                && let Some(h) = &mut e.doc.highlighter
            {
                h.advance(&e.doc.rope, budget);
            }
        }
    }

    /// The pattern-based outline, refreshed only for what's on screen and only now and then:
    /// on a big file it's too slow for every key.
    pub fn refresh_symbols(&mut self) {
        let visible: Vec<DocId> = self.groups.iter().filter_map(|g| g.active_tab().and_then(|t| self.tabs.get(&t)).map(|t| t.doc)).collect();
        for id in visible {
            if let Some(e) = self.docs.get_mut(&id)
                && e.symbols_rev != Some(e.doc.revision)
                && e.last_edit.is_none_or(|t| t.elapsed() > Duration::from_millis(400))
            {
                e.doc.sync_structure();
                e.symbols = if e.doc.len_lines() < 100_000 { tinta_core::outline::symbols(&e.doc) } else { Vec::new() };
                e.symbols_rev = Some(e.doc.revision);
            }
        }
    }

    /// Runs once a second.
    pub fn tick_slow(&mut self) -> Task {
        let mut tasks = vec![self.debug_tick()];
        self.refresh_symbols();
        // Debounced git diff of the buffer against HEAD.
        for e in self.docs.values_mut() {
            if e.git_stale
                && e.last_edit.is_none_or(|t| t.elapsed() > Duration::from_millis(300))
                && let Some(base) = &e.git_base
            {
                e.git_diff = core_git::line_diff(base, &e.doc.rope.to_string());
                e.git_stale = false;
            }
        }
        // Auto save.
        if self.settings.auto_save == AutoSave::Delay {
            let delay = Duration::from_millis(self.settings.auto_save_delay_ms);
            let due = self.docs.values().any(|e| e.doc.is_dirty() && e.doc.path.is_some() && !e.is_read_only_view && e.last_edit.is_some_and(|t| t.elapsed() >= delay));
            if due {
                tasks.push(self.save_all_dirty_named());
            }
        }
        // Hot exit copies and the session file.
        if self.session_dirty.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            self.session_dirty = None;
            if !self.new_session {
                self.write_backups(false);
                super::save_session(self);
            }
        }
        // Hand-edited settings, picked up without a restart.
        let m = super::file_mtime(&tinta_core::settings::settings_path());
        let k = super::file_mtime(&tinta_core::settings::keybindings_path());
        if m != self.settings_mtime || k != self.keymap_mtime {
            self.settings_mtime = m;
            self.keymap_mtime = k;
            tasks.push(self.reload_config_files());
        }
        tasks.push(self.update_blame());
        // Late diagnostics or reordering can leave a pending didChange behind.
        self.lsp_flush_all();
        Task::batch(tasks)
    }

    pub fn on_active_changed(&mut self) -> Task {
        self.close_popups();
        self.vim_reset();
        self.refresh_symbols();
        self.after_caret_move();
        self.mark_session_dirty();
        if self.sidebar == Some(super::SidebarView::Outline) {
            self.request_symbols();
        }
        self.refresh_git_for_active();
        if let Some((t, e)) = self.active_mut() {
            e.matches_key = None;
            t.caret_stamp += 1;
        }
        self.update_title()
    }

    pub fn ensure_terminal(&mut self) -> Task {
        if self.terms.is_empty() {
            return self.new_terminal();
        }
        self.panel = Some(PanelView::Terminal);
        self.focus = Focus::Terminal;
        Task::none()
    }

    pub fn new_terminal(&mut self) -> Task {
        let id = self.new_id();
        let cwd = self
            .folder
            .clone()
            .or_else(|| self.active().and_then(|(_, e)| e.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf)));
        let cwd = cwd.unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        self.terms.push(super::TermInfo { id, title: String::new() });
        self.active_term = self.terms.len() - 1;
        self.panel = Some(PanelView::Terminal);
        self.focus = Focus::Terminal;
        Task::batch([Task::effect(fx::Effect::TermSpawn { id, cwd }), Task::effect(fx::Effect::TermFocus(id))])
    }

    pub fn jump_to(&mut self, line: usize, col: usize) {
        let Some((t, e)) = self.active_mut() else { return };
        let pos = e.doc.pos(line, col);
        t.cursors.set_single(Selection::cursor(pos));
        if t.folds.reveal(line) {
            let wrap = t.map.wrap;
            t.map.update(&e.doc, &t.folds, wrap, e.doc.indent.width.max(1));
        }
        e.doc.break_undo();
        self.center_active();
        self.after_caret_move();
    }

    /// Before anything that acts on folds: on big files the tree may be waiting for a pause.
    pub fn sync_active_structure(&mut self) {
        if let Some((_, e)) = self.active_mut() {
            e.doc.sync_structure();
        }
    }

    pub fn fold_line_range(doc: &Document, line: usize) -> Option<usize> {
        display::fold_range(doc, line, doc.indent.width.max(1))
    }
}
