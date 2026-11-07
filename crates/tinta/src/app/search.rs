//! Find and replace across the folder.

use super::{App, Focus, Message, SidebarView};
use crate::bus;
use crate::i18n::tr;
use crate::fx::{self, Task};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tinta_core::find::{self, FileMatches, Query, SearchScope};

pub const SEARCH_INPUT: &str = "search-input";
const MAX_LISTED_MATCHES: usize = 50_000;

#[derive(Debug, Clone)]
pub enum SearchRow {
    File { path: PathBuf, count: usize, collapsed: bool },
    Match { path: PathBuf, line: usize, start: usize, end: usize, text: String },
}

#[derive(Default)]
pub struct SearchPanel {
    pub query: Query,
    pub replacement: String,
    pub include: String,
    pub exclude: String,
    pub replace_open: bool,
    pub running: bool,
    pub error: Option<String>,
    pub results: Vec<FileMatches>,
    pub rows: Vec<SearchRow>,
    pub collapsed: std::collections::HashSet<PathBuf>,
    pub generation: u64,
    pub cancel: Option<Arc<AtomicBool>>,
    pub total: usize,
    /// The list stopped at the limit: there's more than it shows.
    pub truncated: bool,
    pub replacing: bool,
    pub scroll_y: f32,
    pub view_h: f32,
}

#[derive(Debug, Clone)]
pub enum SearchMsg {
    Query(String),
    Replacement(String),
    Include(String),
    Exclude(String),
    ToggleCase,
    ToggleWord,
    ToggleRegex,
    ToggleReplace,
    Run,
    Found(u64, FileMatches),
    Done(u64, Result<usize, String>),
    ToggleFile(PathBuf),
    Open(PathBuf, usize, usize, usize),
    ReplaceAll,
    ReplaceIn(Vec<PathBuf>),
    Scrolled(f32, f32),
}

impl App {
    pub fn open_search(&mut self, replace: bool) -> Task {
        self.sidebar = Some(SidebarView::Search);
        self.last_sidebar = SidebarView::Search;
        self.search.replace_open |= replace;
        if let Some((t, e)) = self.active() {
            let p = t.cursors.primary();
            if !p.is_empty() && e.doc.char_to_line(p.start()) == e.doc.char_to_line(p.end()) {
                self.search.query.pattern = e.doc.slice_string(p.start(), p.end());
                self.run_search();
            }
        }
        self.focus = Focus::Input;
        Task::batch([fx::focus(SEARCH_INPUT), fx::select_all(SEARCH_INPUT)])
    }

    fn run_search(&mut self) {
        let Some(root) = self.folder.clone() else {
            self.search.error = Some(tr("No folder open").to_string());
            return;
        };
        if let Some(c) = self.search.cancel.take() {
            c.store(true, Ordering::Relaxed);
        }
        self.search.results.clear();
        self.search.rows.clear();
        self.search.total = 0;
        self.search.truncated = false;
        self.search.error = None;
        self.search.generation += 1;
        if self.search.query.pattern.is_empty() {
            self.search.running = false;
            return;
        }
        self.search.running = true;
        let gen_id = self.search.generation;
        let cancel = Arc::new(AtomicBool::new(false));
        self.search.cancel = Some(cancel.clone());
        let query = self.search.query.clone();
        let scope = SearchScope {
            include: self.search.include.clone(),
            exclude: self.search.exclude.clone(),
            hidden: self.settings.show_hidden_files,
        };
        std::thread::spawn(move || {
            let r = find::search_files(&root, &query, &scope, &cancel, MAX_LISTED_MATCHES, |f| {
                bus::send(Message::Search(SearchMsg::Found(gen_id, f)));
            });
            bus::send(Message::Search(SearchMsg::Done(gen_id, r)));
        });
    }

    fn rebuild_search_rows(&mut self) {
        let mut rows = Vec::new();
        for f in &self.search.results {
            let collapsed = self.search.collapsed.contains(&f.path);
            rows.push(SearchRow::File { path: f.path.clone(), count: f.matches.len(), collapsed });
            if !collapsed {
                for m in &f.matches {
                    rows.push(SearchRow::Match { path: f.path.clone(), line: m.line, start: m.start, end: m.end, text: m.text.clone() });
                }
            }
        }
        self.search.rows = rows;
    }

    pub fn on_search_msg(&mut self, m: SearchMsg) -> Task {
        match m {
            SearchMsg::Query(s) => {
                self.search.query.pattern = s;
                self.run_search();
            }
            SearchMsg::Replacement(s) => self.search.replacement = s,
            SearchMsg::Include(s) => {
                self.search.include = s;
                self.run_search();
            }
            SearchMsg::Exclude(s) => {
                self.search.exclude = s;
                self.run_search();
            }
            SearchMsg::ToggleCase => {
                self.search.query.case_sensitive = !self.search.query.case_sensitive;
                self.run_search();
            }
            SearchMsg::ToggleWord => {
                self.search.query.whole_word = !self.search.query.whole_word;
                self.run_search();
            }
            SearchMsg::ToggleRegex => {
                self.search.query.regex = !self.search.query.regex;
                self.run_search();
            }
            SearchMsg::ToggleReplace => self.search.replace_open = !self.search.replace_open,
            SearchMsg::Run => self.run_search(),
            SearchMsg::Found(g, f) => {
                if g == self.search.generation {
                    self.search.total += f.matches.len();
                    let at = self.search.results.partition_point(|x| x.path < f.path);
                    self.search.results.insert(at, f);
                    self.rebuild_search_rows();
                }
            }
            SearchMsg::Done(g, r) => {
                if g == self.search.generation {
                    self.search.running = false;
                    match r {
                        Ok(n) => self.search.truncated = n >= MAX_LISTED_MATCHES,
                        Err(e) => self.search.error = Some(e),
                    }
                }
            }
            SearchMsg::ToggleFile(p) => {
                if !self.search.collapsed.remove(&p) {
                    self.search.collapsed.insert(p);
                }
                self.rebuild_search_rows();
            }
            SearchMsg::Open(p, line, start, end) => {
                self.push_nav();
                let t = self.open_path(&p);
                if let Some((tab, e)) = self.active_mut() {
                    let a = e.doc.pos(line, start);
                    let b = e.doc.pos(line, end);
                    tab.cursors.set_single(tinta_core::Selection::new(a, b));
                }
                self.center_active();
                self.after_caret_move();
                return t;
            }
            SearchMsg::ReplaceAll => self.start_replace_in_files(),
            SearchMsg::ReplaceIn(paths) => return self.replace_in_files(paths),
            SearchMsg::Scrolled(y, h) => {
                self.search.scroll_y = y;
                self.search.view_h = h;
            }
        }
        Task::none()
    }

    /// The list on screen stops at a limit, so replacing walks the folder again, without one.
    fn start_replace_in_files(&mut self) {
        let Some(root) = self.folder.clone() else { return };
        if self.search.replacing || self.search.query.pattern.is_empty() {
            return;
        }
        self.search.replacing = true;
        let query = self.search.query.clone();
        let scope = SearchScope {
            include: self.search.include.clone(),
            exclude: self.search.exclude.clone(),
            hidden: self.settings.show_hidden_files,
        };
        std::thread::spawn(move || {
            let cancel = Arc::new(AtomicBool::new(false));
            let mut paths = Vec::new();
            let _ = find::search_files(&root, &query, &scope, &cancel, usize::MAX, |f| paths.push(f.path));
            bus::send(Message::Search(SearchMsg::ReplaceIn(paths)));
        });
    }

    /// Open files change in the buffer, with undo; the rest change on disk.
    fn replace_in_files(&mut self, mut paths: Vec<PathBuf>) -> Task {
        self.search.replacing = false;
        let q = self.search.query.clone();
        let Ok(re) = q.compile() else { return Task::none() };
        let rep = self.search.replacement.clone();
        // The walk read the disk: text only in an unsaved buffer counts too, unless filters are on.
        if self.search.include.trim().is_empty() && self.search.exclude.trim().is_empty() {
            let root = self.folder.clone().unwrap_or_default();
            for e in self.docs.values().filter(|e| e.doc.is_dirty()) {
                if let Some(p) = e.doc.path.clone().filter(|p| p.starts_with(&root) && !paths.contains(p)) {
                    paths.push(p);
                }
            }
        }
        let mut total = 0;
        let mut failures = Vec::new();
        for p in paths {
            if let Some(id) = self.doc_by_path(&p) {
                let Some(tab) = self.tabs.values_mut().find(|t| t.doc == id) else { continue };
                let Some(e) = self.docs.get_mut(&id) else { continue };
                let found = find::find_all(&e.doc, &re, usize::MAX);
                let edits: Vec<_> = found
                    .iter()
                    .map(|&(a, b)| tinta_core::Edit::new(a, b, find::expand(&re, &q, &e.doc.slice_string(a, b), &rep)))
                    .collect();
                total += edits.len();
                let tid = tab.id;
                e.doc.apply(edits, &mut tab.cursors, tinta_core::EditKind::Other, |m, c| {
                    c.map(|s| tinta_core::Selection::cursor(m.map(s.head, tinta_core::buffer::Assoc::Before)));
                });
                self.edit_tab = Some(tid);
            } else {
                match find::replace_in_file(&p, &q, &rep) {
                    Ok(n) => total += n,
                    Err(e) => failures.push(format!("{}: {e}", p.display())),
                }
            }
        }
        self.run_search();
        self.refresh_git_panel();
        let failed = failures.len();
        self.output.extend(failures);
        if failed > 0 {
            return self.toast(format!("{total} {}; {failed} {}", tr("replacements"), tr("files failed, see Output")));
        }
        self.toast(format!("{total} {}", tr("replacements")))
    }
}
