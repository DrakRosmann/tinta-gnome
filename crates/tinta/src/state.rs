//! The editor's model: documents, the tabs that show them and everything hanging off each.

use crate::gfx::Font;
use std::collections::BTreeSet;
use std::path::PathBuf;
use tinta_core::git::LineDiff;
use tinta_core::lsp::Severity;
use tinta_core::snippet::Session as SnippetSession;
use tinta_core::{Cursors, DisplayMap, Document, display::Folds};

pub type DocId = u64;
pub type TabId = u64;

/// A diagnostic in editor coordinates, kept up to date as the text changes.
#[derive(Debug, Clone)]
pub struct Diag {
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
    pub message: String,
    pub source: Option<String>,
    pub raw: serde_json::Value,
}

pub struct DocEntry {
    pub doc: Document,
    pub diags: Vec<Diag>,
    pub bookmarks: BTreeSet<usize>,
    /// What HEAD has for this file, for the gutter.
    pub git_base: Option<String>,
    pub git_diff: LineDiff,
    pub git_stale: bool,
    pub git_rel: Option<(PathBuf, String)>,
    pub blame: Option<(usize, String)>,
    /// Parts of the find state tied to this document's text.
    pub matches: Vec<(usize, usize)>,
    pub matches_key: Option<(u64, u64)>,
    pub lsp_key: Option<String>,
    pub lsp_dirty: bool,
    pub last_edit: Option<std::time::Instant>,
    pub symbols: Vec<tinta_core::outline::Symbol>,
    pub symbols_rev: Option<u64>,
    pub lsp_symbols: Option<(u64, Vec<tinta_core::lsp::DocSymbol>)>,
    pub backup_key: String,
    pub backup_rev: u64,
    pub changed_on_disk: bool,
    pub is_read_only_view: bool,
}

impl DocEntry {
    pub fn new(doc: Document, backup_key: String) -> Self {
        Self {
            doc,
            diags: Vec::new(),
            bookmarks: BTreeSet::new(),
            git_base: None,
            git_diff: LineDiff::default(),
            git_stale: true,
            git_rel: None,
            blame: None,
            matches: Vec::new(),
            matches_key: None,
            lsp_key: None,
            lsp_dirty: false,
            last_edit: None,
            symbols: Vec::new(),
            symbols_rev: None,
            lsp_symbols: None,
            backup_key,
            backup_rev: 0,
            changed_on_disk: false,
            is_read_only_view: false,
        }
    }

    pub fn diag_severity_at_line(&self, line: usize) -> Option<Severity> {
        let mut best: Option<Severity> = None;
        for d in &self.diags {
            let l0 = self.doc.char_to_line(d.start);
            let l1 = self.doc.char_to_line(d.end);
            if l0 <= line && line <= l1 {
                best = Some(match (best, d.severity) {
                    (Some(Severity::Error), _) | (_, Severity::Error) => Severity::Error,
                    (Some(Severity::Warning), _) | (_, Severity::Warning) => Severity::Warning,
                    (Some(s), _) => s,
                    (None, s) => s,
                });
            }
        }
        best
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TabKind {
    Text,
    /// Rendered Markdown of the document.
    Preview,
    /// Two versions of a file, side by side.
    Diff,
}

/// What a side-by-side tab shows. Both sides are snapshots, with their own colours.
pub struct DiffData {
    pub left: Document,
    pub right: Document,
    pub left_title: String,
    pub right_title: String,
    pub rows: Vec<tinta_core::diff::DiffRow>,
    pub hunks: Vec<usize>,
    pub added: usize,
    pub removed: usize,
    pub scroll_y: f32,
    pub scroll_x: f32,
    pub view_h: f32,
    pub current: Option<usize>,
    pub new_path: Option<PathBuf>,
}

pub struct Tab {
    pub id: TabId,
    pub doc: DocId,
    pub kind: TabKind,
    pub cursors: Cursors,
    pub folds: Folds,
    pub map: DisplayMap,
    pub scroll_y: f32,
    pub scroll_x: f32,
    /// Size of the whole editor widget.
    pub view_w: f32,
    pub view_h: f32,
    pub pinned: bool,
    pub expand_stack: Vec<Cursors>,
    pub column_anchor: Option<(usize, usize)>,
    pub drag_origin: Option<DragOrigin>,
    pub snippet: Option<SnippetSession>,
    pub preedit: String,
    pub bracket_pair: Option<(usize, usize)>,
    pub occurrence: Option<(String, bool)>,
    /// Bumped on every caret change: restarts the blink.
    pub caret_stamp: u64,
    pub preview_cache: Option<(u64, Vec<crate::ui::markdown::Block>)>,
    pub diff: Option<Box<DiffData>>,
}

/// What a drag extends from: double click drags by word, triple click by line.
#[derive(Debug, Clone, Copy)]
pub struct DragOrigin {
    pub start: usize,
    pub end: usize,
    pub clicks: u8,
    pub added_cursor: bool,
}

impl Tab {
    pub fn new(id: TabId, doc: DocId, entry: &DocEntry) -> Self {
        Self {
            id,
            doc,
            kind: TabKind::Text,
            cursors: Cursors::default(),
            folds: Folds::default(),
            map: DisplayMap::new(&entry.doc, &Folds::default(), None, entry.doc.indent.width.max(1)),
            scroll_y: 0.0,
            scroll_x: 0.0,
            view_w: 800.0,
            view_h: 600.0,
            pinned: false,
            expand_stack: Vec::new(),
            column_anchor: None,
            drag_origin: None,
            snippet: None,
            preedit: String::new(),
            bracket_pair: None,
            occurrence: None,
            caret_stamp: 0,
            preview_cache: None,
            diff: None,
        }
    }
}

/// The tabs of one split, in order, and which one shows. Tabs are known by their own id.
pub type Entity = TabId;

/// No tab: what `active` gives when the group is empty.
pub const NO_TAB: Entity = u64::MAX;

#[derive(Debug, Default, Clone)]
pub struct TabModel {
    items: Vec<(TabId, String)>,
    active: Option<TabId>,
}

pub struct TabBuilder<'a> {
    model: &'a mut TabModel,
    text: String,
    id: Option<TabId>,
}

impl TabBuilder<'_> {
    pub fn text(mut self, t: String) -> Self {
        self.text = t;
        self
    }
    pub fn data<T>(mut self, id: TabId) -> Self {
        self.id = Some(id);
        self
    }
    pub fn closable(self) -> Self {
        self
    }
    /// Adds the tab after the active one, as editors do, and shows it.
    pub fn activate(self) {
        let Some(id) = self.id else { return };
        let at = self.model.active.and_then(|a| self.model.items.iter().position(|(t, _)| *t == a)).map_or(self.model.items.len(), |i| i + 1);
        self.model.items.insert(at, (id, self.text));
        self.model.active = Some(id);
    }
}

impl TabModel {
    pub fn insert(&mut self) -> TabBuilder<'_> {
        TabBuilder { model: self, text: String::new(), id: None }
    }
    pub fn text(&self, e: Entity) -> Option<&str> {
        self.items.iter().find(|(t, _)| *t == e).map(|(_, s)| s.as_str())
    }
    pub fn text_set(&mut self, e: Entity, text: String) {
        if let Some(item) = self.items.iter_mut().find(|(t, _)| *t == e) {
            item.1 = text;
        }
    }
    pub fn activate(&mut self, e: Entity) {
        if self.items.iter().any(|(t, _)| *t == e) {
            self.active = Some(e);
        }
    }
    pub fn active(&self) -> Entity {
        self.active.unwrap_or(NO_TAB)
    }
    pub fn position(&self, e: Entity) -> Option<u16> {
        self.items.iter().position(|(t, _)| *t == e).map(|i| i as u16)
    }
    pub fn position_set(&mut self, e: Entity, pos: u16) {
        if let Some(i) = self.items.iter().position(|(t, _)| *t == e) {
            let item = self.items.remove(i);
            let at = (pos as usize).min(self.items.len());
            self.items.insert(at, item);
        }
    }
    pub fn remove(&mut self, e: Entity) {
        self.items.retain(|(t, _)| *t != e);
        if self.active == Some(e) {
            self.active = None;
        }
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn activate_position(&mut self, pos: u16) {
        if let Some((t, _)) = self.items.get(pos as usize) {
            self.active = Some(*t);
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = Entity> + '_ {
        self.items.iter().map(|(t, _)| *t)
    }
    pub fn data<T>(&self, e: Entity) -> Option<&TabId> {
        self.items.iter().find(|(t, _)| *t == e).map(|(t, _)| t)
    }
}

pub struct Group {
    pub model: TabModel,
    pub weight: f32,
}

impl Group {
    pub fn new() -> Self {
        Self { model: TabModel::default(), weight: 1.0 }
    }

    pub fn active_tab(&self) -> Option<TabId> {
        self.model.data::<TabId>(self.model.active()).copied()
    }

    pub fn tabs(&self) -> Vec<TabId> {
        self.model.iter().collect()
    }

    pub fn entity_of(&self, tab: TabId) -> Option<Entity> {
        self.model.iter().find(|e| *e == tab)
    }
}

/// Font and sizes everything in the editor is measured with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub font: Font,
    pub size: f32,
    pub line_h: f32,
    pub cell_w: f32,
}

impl Metrics {
    pub fn new(family: &str, size: f32, line_height: f32) -> Self {
        Self {
            font: mono_font(family),
            size,
            line_h: (size * line_height).round(),
            cell_w: (size * 0.6).max(1.0),
        }
    }
}

/// Fonts name their family with a `&'static str`; each family is leaked once at most.
pub fn mono_font(family: &str) -> Font {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let names = NAMES.get_or_init(Default::default);
    let mut names = names.lock().unwrap();
    let name: &'static str = match names.get(family) {
        Some(n) => n,
        None => {
            let leaked: &'static str = Box::leak(family.to_string().into_boxed_str());
            names.insert(family.to_string(), leaked);
            leaked
        }
    };
    Font {
        family: name,
        ..Font::MONOSPACE
    }
}
