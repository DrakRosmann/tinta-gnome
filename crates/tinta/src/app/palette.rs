//! The command palette and every other quick pick built on it.

use super::{App, Focus, KeyEv};
use crate::i18n::tr;
use crate::fx::{self, Task};
use std::path::PathBuf;
use std::sync::Arc;
use tinta_core::keymap::Command;
use tinta_core::{Cursors, fuzzy, git as core_git, language, outline, snippet};

pub const PALETTE_INPUT: &str = "palette-input";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteMode {
    Commands,
    Files,
    Line,
    Symbols,
    Language,
    Theme,
    Recent,
    Encoding,
    Indent,
    Branch,
    Snippet,
    Actions,
    Locations,
}

#[derive(Debug, Clone)]
pub enum PAction {
    Cmd(Command),
    Open(PathBuf),
    OpenFolder(PathBuf),
    Line(usize),
    Theme(String),
    Language(String),
    Encoding(&'static str),
    Indent(usize),
    Branch(String),
    NewBranch(String),
    Snippet(String),
    Action(usize),
    Location(PathBuf, usize, usize),
    Task(Box<tinta_core::tasks::Task>),
    DebugConfig(String),
}

#[derive(Debug, Clone)]
pub struct PItem {
    pub label: String,
    pub detail: String,
    pub hint: String,
    pub positions: Vec<u32>,
    pub action: PAction,
}

pub struct PaletteState {
    pub mode: PaletteMode,
    pub query: String,
    pub items: Vec<PItem>,
    pub selected: usize,
    /// To put things back on Escape after previews moved the caret.
    pub restore: Option<(Cursors, f32)>,
    pub actions: Vec<tinta_core::lsp::CodeAction>,
    pub locations: Vec<PItem>,
    pub theme_before: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PaletteMsg {
    Input(String),
    Submit,
    Click(usize),
    Close,
    FilesIndexed(Arc<Vec<String>>),
}

const ENCODINGS: &[(&str, &str)] = &[
    ("UTF-8", "UTF-8"),
    ("UTF-16 LE", "UTF-16LE"),
    ("UTF-16 BE", "UTF-16BE"),
    ("Western (Windows 1252)", "windows-1252"),
    ("Western (ISO 8859-1)", "ISO-8859-1"),
    ("Central European (ISO 8859-2)", "ISO-8859-2"),
    ("Cyrillic (Windows 1251)", "windows-1251"),
    ("Greek (ISO 8859-7)", "ISO-8859-7"),
    ("Japanese (Shift JIS)", "Shift_JIS"),
    ("Japanese (EUC-JP)", "EUC-JP"),
    ("Korean (EUC-KR)", "EUC-KR"),
    ("Simplified Chinese (GBK)", "GBK"),
    ("Traditional Chinese (Big5)", "Big5"),
];

impl App {
    pub fn open_palette(&mut self, mode: PaletteMode) -> Task {
        let restore = self.active().map(|(t, _)| (t.cursors.clone(), t.scroll_y));
        if mode == PaletteMode::Files && self.file_index.is_empty() && self.folder.is_some() {
            self.index_files();
        }
        self.palette = Some(PaletteState {
            mode,
            query: String::new(),
            items: Vec::new(),
            selected: 0,
            restore,
            actions: Vec::new(),
            locations: Vec::new(),
            theme_before: None,
        });
        self.close_popups();
        self.focus = Focus::Input;
        self.refilter_palette();
        fx::focus(PALETTE_INPUT)
    }

    /// A pick list of things already known, like code actions or references.
    pub fn open_palette_with(&mut self, mode: PaletteMode, items: Vec<PItem>, actions: Vec<tinta_core::lsp::CodeAction>) -> Task {
        let t = self.open_palette(mode);
        if let Some(p) = &mut self.palette {
            p.locations = items.clone();
            p.items = items;
            p.actions = actions;
        }
        t
    }

    fn all_commands(&self) -> Vec<PItem> {
        let pt = self.pt();
        Command::ALL
            .iter()
            .map(|c| PItem {
                label: c.title(pt).to_string(),
                detail: String::new(),
                hint: self.keymap.label(*c).unwrap_or("").to_string(),
                positions: Vec::new(),
                action: PAction::Cmd(*c),
            })
            .collect()
    }

    pub fn snippets_for_active(&self) -> Vec<snippet::Definition> {
        let Some((_, e)) = self.active() else { return Vec::new() };
        let lang = e.doc.language.name.as_str();
        let mut v = snippet::builtin(lang);
        if let Some(user) = self.user_snippets.get(lang) {
            v.extend(user.iter().cloned());
        }
        v
    }

    fn items_for_mode(&self, mode: &PaletteMode, query: &str) -> (Vec<PItem>, bool) {
        let item = |label: String, detail: String, action: PAction| PItem { label, detail, hint: String::new(), positions: Vec::new(), action };
        match mode {
            PaletteMode::Commands => (self.all_commands(), false),
            PaletteMode::Files => {
                let mut out = Vec::new();
                // Recently used first when nothing's typed.
                if query.is_empty() {
                    for p in self.recent_files.iter().take(12) {
                        let rel = self.folder.as_ref().and_then(|f| p.strip_prefix(f).ok()).unwrap_or(p);
                        out.push(item(
                            p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                            rel.parent().map(|d| d.display().to_string()).unwrap_or_default(),
                            PAction::Open(p.clone()),
                        ));
                    }
                }
                if let Some(f) = &self.folder {
                    for rel in self.file_index.iter() {
                        out.push(PItem {
                            label: rel.clone(),
                            detail: String::new(),
                            hint: String::new(),
                            positions: Vec::new(),
                            action: PAction::Open(f.join(rel)),
                        });
                    }
                }
                (out, true)
            }
            PaletteMode::Symbols => {
                let Some((_, e)) = self.active() else { return (Vec::new(), false) };
                let lsp = e.lsp_symbols.as_ref().filter(|(rev, _)| *rev == e.doc.revision).map(|(_, s)| s.clone());
                let items = match lsp {
                    Some(list) if !list.is_empty() => list
                        .into_iter()
                        .map(|s| item(format!("{}{}", "  ".repeat(s.depth), s.name), s.detail, PAction::Line(s.line)))
                        .collect(),
                    _ => outline::symbols(&e.doc)
                        .into_iter()
                        .map(|s| item(format!("{}{}", "  ".repeat(s.depth), s.name), s.kind.to_string(), PAction::Line(s.line)))
                        .collect(),
                };
                (items, false)
            }
            PaletteMode::Language => (
                language::all().iter().map(|l| item(l.name.clone(), String::new(), PAction::Language(l.name.clone()))).collect(),
                false,
            ),
            PaletteMode::Theme => (
                self.syntax_theme_names.iter().map(|n| item(n.clone(), String::new(), PAction::Theme(n.clone()))).collect(),
                false,
            ),
            PaletteMode::Recent => {
                let mut out: Vec<PItem> = self
                    .recent_folders
                    .iter()
                    .map(|p| item(
                        p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                        p.display().to_string(),
                        PAction::OpenFolder(p.clone()),
                    ))
                    .collect();
                out.extend(self.recent_files.iter().map(|p| item(
                    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                    p.display().to_string(),
                    PAction::Open(p.clone()),
                )));
                (out, false)
            }
            PaletteMode::Encoding => (
                ENCODINGS.iter().map(|(n, id)| item(n.to_string(), id.to_string(), PAction::Encoding(id))).collect(),
                false,
            ),
            PaletteMode::Indent => (
                [2usize, 3, 4, 6, 8].iter().map(|n| item(n.to_string(), String::new(), PAction::Indent(*n))).collect(),
                false,
            ),
            PaletteMode::Branch => {
                let mut out = Vec::new();
                if let Some(r) = &self.git.root {
                    for b in core_git::branches(r).unwrap_or_default() {
                        out.push(item(b.clone(), String::new(), PAction::Branch(b)));
                    }
                }
                if !query.trim().is_empty() {
                    out.push(item(format!("+ {}", query.trim()), String::new(), PAction::NewBranch(query.trim().to_string())));
                }
                (out, false)
            }
            PaletteMode::Snippet => (
                self.snippets_for_active()
                    .into_iter()
                    .map(|d| item(d.prefix.clone(), d.description.clone(), PAction::Snippet(d.body)))
                    .collect(),
                false,
            ),
            PaletteMode::Actions | PaletteMode::Locations => {
                (self.palette.as_ref().map(|p| p.locations.clone()).unwrap_or_default(), false)
            }
            PaletteMode::Line => (Vec::new(), false),
        }
    }

    pub fn refilter_palette(&mut self) {
        let Some(p) = &self.palette else { return };
        let (mode, query) = (p.mode.clone(), p.query.clone());
        if mode == PaletteMode::Line {
            let total = self.active().map(|(_, e)| e.doc.len_lines()).unwrap_or(1);
            let parsed = parse_line_query(&query);
            let label = match parsed {
                Some((l, _)) => format!("{} {}", tr("Ln"), (l + 1).min(total)),
                None => format!("1 – {total}"),
            };
            let items = match parsed {
                Some((l, c)) => vec![PItem { label, detail: String::new(), hint: String::new(), positions: Vec::new(), action: PAction::Location(PathBuf::new(), l, c) }],
                None => Vec::new(),
            };
            if let Some(p) = &mut self.palette {
                p.items = items;
                p.selected = 0;
            }
            self.preview_selected();
            return;
        }
        let (items, path_mode) = self.items_for_mode(&mode, &query);
        let hits = fuzzy::filter(items.iter().map(|i| i.label.as_str()), query.trim(), path_mode, 200);
        let filtered: Vec<PItem> = hits
            .into_iter()
            .map(|h| {
                let mut i = items[h.index].clone();
                i.positions = h.positions;
                i
            })
            .collect();
        if let Some(p) = &mut self.palette {
            p.items = filtered;
            p.selected = 0;
        }
        self.preview_selected();
    }

    /// Going through lines and symbols shows them right away.
    fn preview_selected(&mut self) {
        let Some(p) = &self.palette else { return };
        let target = match (&p.mode, p.items.get(p.selected).map(|i| &i.action)) {
            (PaletteMode::Line | PaletteMode::Symbols, Some(PAction::Line(l))) => Some((*l, 0)),
            (PaletteMode::Line, Some(PAction::Location(_, l, c))) => Some((*l, *c)),
            _ => None,
        };
        if let Some((l, c)) = target {
            self.jump_to(l, c);
            self.focus = Focus::Input;
        }
        if let Some(p) = &self.palette
            && p.mode == PaletteMode::Theme
            && let Some(PAction::Theme(n)) = p.items.get(p.selected).map(|i| &i.action)
        {
            let n = n.clone();
            let before = self.settings.syntax_theme.clone();
            if let Some(pal) = &mut self.palette
                && pal.theme_before.is_none()
            {
                pal.theme_before = Some(before);
            }
            self.settings.syntax_theme = n;
            self.refresh_syntax_theme();
        }
    }

    fn close_palette(&mut self, cancelled: bool) -> Task {
        let Some(p) = self.palette.take() else { return Task::none() };
        self.focus = Focus::Editor;
        if cancelled {
            if matches!(p.mode, PaletteMode::Line | PaletteMode::Symbols)
                && let Some((c, y)) = p.restore
                && let Some((t, e)) = self.active_mut()
            {
                // The text may have changed while the palette was open.
                let mut c = c;
                c.clamp(e.doc.len_chars());
                t.cursors = c;
                t.scroll_y = y;
                self.after_caret_move();
            }
            if let Some(n) = p.theme_before {
                self.settings.syntax_theme = n;
                self.refresh_syntax_theme();
            }
        }
        Task::none()
    }

    pub fn palette_key(&mut self, ev: &KeyEv) -> Task {
        let Some(p) = &mut self.palette else { return Task::none() };
        let n = p.items.len();
        match ev.name.as_str() {
            "escape" => return self.close_palette(true),
            "down" if n > 0 => p.selected = (p.selected + 1) % n,
            "up" if n > 0 => p.selected = (p.selected + n - 1) % n,
            "pagedown" if n > 0 => p.selected = (p.selected + 10).min(n - 1),
            "pageup" if n > 0 => p.selected = p.selected.saturating_sub(10),
            "enter" => return self.palette_accept(),
            _ => return Task::none(),
        }
        self.preview_selected();
        Task::none()
    }

    fn palette_accept(&mut self) -> Task {
        let Some(p) = &self.palette else { return Task::none() };
        let Some(item) = p.items.get(p.selected).cloned() else {
            return self.close_palette(true);
        };
        let actions = p.actions.clone();
        let mode = p.mode.clone();
        // Accepting a preview keeps what the preview did.
        if let Some(pal) = &mut self.palette {
            pal.theme_before = None;
        }
        let _ = self.close_palette(false);
        self.focus = Focus::Editor;
        match item.action {
            PAction::Cmd(c) => self.run(c),
            PAction::Open(p) => self.open_path(&p),
            PAction::OpenFolder(p) => self.open_folder(p),
            PAction::Line(l) => {
                self.jump_to(l, 0);
                Task::none()
            }
            PAction::Location(p, l, c) => {
                if mode == PaletteMode::Line || p.as_os_str().is_empty() {
                    self.jump_to(l, c);
                    return Task::none();
                }
                self.push_nav();
                let t = self.open_path(&p);
                self.jump_to(l, c);
                t
            }
            PAction::Theme(n) => {
                self.settings.syntax_theme = n;
                self.refresh_syntax_theme();
                self.persist_settings();
                Task::none()
            }
            PAction::Language(n) => {
                let theme = self.syntax_theme.clone();
                if let Some((t, e)) = self.active_mut() {
                    e.doc.language = language::by_name(&n);
                    super::docs::attach_language(&mut e.doc, theme);
                    let d = t.doc;
                    return self.lsp_open_doc(d);
                }
                Task::none()
            }
            PAction::Encoding(id) => {
                self.set_encoding(id);
                Task::none()
            }
            PAction::Indent(n) => {
                if let Some((_, e)) = self.active_mut() {
                    e.doc.indent.width = n;
                }
                Task::none()
            }
            PAction::Branch(b) => {
                let Some(r) = self.git.root.clone() else { return Task::none() };
                match core_git::checkout(&r, &b) {
                    Ok(()) => {
                        self.refresh_git_panel();
                        self.toast(b)
                    }
                    Err(e) => self.toast(e),
                }
            }
            PAction::NewBranch(b) => {
                let Some(r) = self.git.root.clone() else { return Task::none() };
                match core_git::create_branch(&r, &b) {
                    Ok(()) => {
                        self.refresh_git_panel();
                        self.toast(b)
                    }
                    Err(e) => self.toast(e),
                }
            }
            PAction::Task(t) => self.run_task(*t),
            PAction::DebugConfig(name) => {
                self.debug.selected = Some(name);
                self.debug_start(false)
            }
            PAction::Snippet(body) => {
                self.insert_snippet(&body, None);
                Task::none()
            }
            PAction::Action(i) => match actions.get(i).cloned() {
                Some(a) => self.run_code_action(a),
                None => Task::none(),
            },
        }
    }

    pub fn on_palette(&mut self, m: PaletteMsg) -> Task {
        match m {
            PaletteMsg::Input(s) => {
                let Some(p) = &mut self.palette else { return Task::none() };
                // In the file picker, a prefix switches to another mode, as in VS Code.
                if p.mode == PaletteMode::Files && p.query.is_empty() {
                    let (mode, rest) = match s.chars().next() {
                        Some('>') => (Some(PaletteMode::Commands), &s[1..]),
                        Some(':') => (Some(PaletteMode::Line), &s[1..]),
                        Some('@') => (Some(PaletteMode::Symbols), &s[1..]),
                        _ => (None, s.as_str()),
                    };
                    if let Some(mode) = mode {
                        p.mode = mode;
                        p.query = rest.to_string();
                        self.refilter_palette();
                        return Task::none();
                    }
                }
                p.query = s;
                self.refilter_palette();
                Task::none()
            }
            PaletteMsg::Submit => self.palette_accept(),
            PaletteMsg::Click(i) => {
                if let Some(p) = &mut self.palette {
                    p.selected = i;
                }
                self.palette_accept()
            }
            PaletteMsg::Close => self.close_palette(true),
            PaletteMsg::FilesIndexed(files) => {
                self.file_index = files;
                self.file_index_loading = false;
                if self.palette.as_ref().is_some_and(|p| p.mode == PaletteMode::Files) {
                    self.refilter_palette();
                }
                Task::none()
            }
        }
    }

    /// Re-reads the file in another encoding when clean; otherwise only changes how it saves.
    fn set_encoding(&mut self, id: &'static str) {
        let Some(enc) = encoding_rs::Encoding::for_label(id.as_bytes()) else { return };
        let Some((t, e)) = self.active_mut() else { return };
        e.doc.encoding = enc;
        e.doc.has_bom = false;
        if !e.doc.is_dirty()
            && let Some(p) = e.doc.path.clone()
            && let Ok(bytes) = std::fs::read(&p)
        {
            let (text, _, _) = enc.decode(&bytes);
            let text = tinta_core::buffer::normalize_newlines(&text).into_owned();
            e.doc.replace_all(&text, &mut t.cursors);
            e.doc.mark_saved();
        }
    }
}

/// `12`, `:12`, `12:4`.
fn parse_line_query(q: &str) -> Option<(usize, usize)> {
    let q = q.trim().trim_start_matches(':');
    let (l, c) = q.split_once([':', ',']).unwrap_or((q, ""));
    let line = l.trim().parse::<usize>().ok()?.saturating_sub(1);
    let col = c.trim().parse::<usize>().unwrap_or(1).saturating_sub(1);
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::parse_line_query;

    #[test]
    fn line_queries() {
        assert_eq!(parse_line_query("12"), Some((11, 0)));
        assert_eq!(parse_line_query(":12:4"), Some((11, 3)));
        assert_eq!(parse_line_query("x"), None);
    }
}
