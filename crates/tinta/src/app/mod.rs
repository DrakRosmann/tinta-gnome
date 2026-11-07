//! The application: all state lives here, every change goes through `update`.

mod commands;
mod debug;
mod diff;
mod docs;
mod editing;
mod explorer;
mod git;
mod keys;
mod lsp;
mod palette;
mod script;
mod search;
mod tasks;
mod vim;

pub use explorer::{Explorer, RowAction};
pub use git::{GitMsg, GitPanel};
pub use keys::KeyEv;
pub use lsp::{Completion, HoverPopup, SignaturePopup};
pub use editing::{FIND_INPUT, MAX_SHOWN_MATCHES, REPLACE_INPUT};
pub use explorer::ExplorerMsg;
pub use palette::{PAction, PItem, PaletteMode, PaletteMsg, PaletteState, PALETTE_INPUT};
pub use debug::{DEBUG_INPUT, DebugMsg, DebugPanel, WATCH_INPUT};
pub use tasks::{TaskMsg, TaskPanel};
pub use vim::{VIM_LINE, VimMsg};
pub use search::{SearchMsg, SearchPanel, SearchRow, SEARCH_INPUT};

use crate::fx;
use crate::gfx::Modifiers;
use crate::state::{DocEntry, DocId, Group, Metrics, Tab, TabId};
use crate::ui;
use crate::ui::editor_view::EditorEvent;

pub use crate::fx::Task;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tinta_core::keymap::{Command, Keymap};
use tinta_core::settings::{AppTheme, AutoSave, Settings};
use tinta_core::{find, lsp as core_lsp};

pub const APP_ID: &str = "io.github.DrakRosmann.TintaGnome";

/// The text field in dialogs that ask for something.
pub const DIALOG_INPUT: &str = "dialog-input";

pub struct Flags {
    pub targets: Vec<(PathBuf, Option<(usize, usize)>)>,
    pub new_session: bool,
    /// What the desktop says: dark or light, and its monospace font.
    pub dark: bool,
    pub system_mono: Option<String>,
    /// Monospace families installed, for the settings.
    pub font_names: Vec<String>,
}

/// A terminal in the panel. The terminal itself is a VTE widget on the GTK side.
#[derive(Debug, Clone)]
pub struct TermInfo {
    pub id: u64,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Terminal,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarView {
    Explorer,
    Search,
    Git,
    Outline,
    Debug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelView {
    Terminal,
    Tasks,
    Problems,
    Output,
    Debug,
}

#[derive(Debug, Clone, Default)]
pub struct FindState {
    pub open: bool,
    pub replace_open: bool,
    pub query: find::Query,
    pub replacement: String,
    pub in_selection: Option<Vec<(usize, usize)>>,
    pub error: Option<String>,
    pub current: Option<usize>,
    pub generation: u64,
}

#[derive(Debug, Clone)]
pub enum DialogState {
    CloseUnsaved(TabId),
    QuitUnsaved,
    ChangedOnDisk(DocId),
    NewFile(PathBuf),
    NewFolder(PathBuf),
    RenamePath(PathBuf),
    TrashPath(PathBuf),
    RenameSymbol,
    BreakpointCondition(PathBuf, usize),
    ConfirmDiscard(tinta_core::git::StatusEntry),
    About,
    Shortcuts,
}

#[derive(Debug, Clone)]
pub enum DialogMsg {
    Input(String),
    Primary,
    Secondary,
    Cancel,
    ShortcutsFilter(String),
}

#[derive(Debug, Clone)]
pub enum FindMsg {
    Query(String),
    Replacement(String),
    ToggleCase,
    ToggleWord,
    ToggleRegex,
    ToggleInSelection,
    ToggleReplace,
    Next,
    Prev,
    ReplaceOne,
    ReplaceAll,
    Close,
}

#[derive(Debug, Clone)]
pub enum SettingMsg {
    UiTheme(AppTheme),
    SyntaxTheme(String),
    FontFamily(String),
    FontSize(f32),
    LineHeight(f32),
    TabSize(usize),
    Bool(BoolSetting, bool),
    LineNumbers(tinta_core::settings::LineNumbers),
    Whitespace(tinta_core::settings::Whitespace),
    CursorStyle(tinta_core::settings::CursorStyle),
    AutoSave(AutoSave),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoolSetting {
    Minimap,
    WordWrap,
    CurrentLine,
    BracketColors,
    IndentGuides,
    StickyScroll,
    Folding,
    Occurrences,
    CursorBlink,
    InsertSpaces,
    DetectIndent,
    AutoPairs,
    AutoIndent,
    AutoComplete,
    WordCompletion,
    Snippets,
    GitGutter,
    InlineBlame,
    FormatOnSave,
    TrimTrailing,
    FinalNewline,
    RestoreSession,
    HotExit,
    ShowHidden,
    Lsp,
    Vim,
}

#[derive(Debug, Clone)]
pub enum Message {
    Noop,
    Key(KeyEv),
    Modifiers(Modifiers),
    InputFocus(bool),
    Cmd(Command),
    Editor(TabId, EditorEvent),
    Diff(TabId, ui::diff_view::DiffEvent),
    TabActivate(usize, TabId),
    TabClose(usize, TabId),
    /// A tab dragged to another place in its group.
    TabMoved(usize, TabId, usize),
    SidebarShow(SidebarView),
    SidebarResize(f32),
    PanelResize(f32),
    GroupResize(usize, f32),
    PanelShow(PanelView),
    PanelClose,
    TickFrame,
    TickSlow,
    Toast(String),
    OpenFolderPath(PathBuf),
    OpenFiles(Vec<PathBuf>),
    SaveAsTo(TabId, PathBuf),
    Pasted(Option<String>),
    CopyText(String),
    Find(FindMsg),
    Palette(palette::PaletteMsg),
    Explorer(explorer::ExplorerMsg),
    Search(search::SearchMsg),
    Git(git::GitMsg),
    Task(TaskMsg),
    Vim(VimMsg),
    Debug(DebugMsg),
    /// The desktop's style changed: dark or light, and its monospace font.
    StyleChanged { dark: bool, mono: Option<String> },
    TermLink(String, usize),
    OutlineJump(usize),
    CompletionPick(usize),
    Lsp(core_lsp::Event),
    Dialog(DialogMsg),
    Setting(SettingMsg),
    SettingsClose,
    FsEvent(Vec<PathBuf>),
    TermTitle(u64, Option<String>),
    TermExited(u64),
    TermFocus,
    TermSelect(usize),
    TermKill(usize),
    ProblemJump(PathBuf, usize, usize),
    CloseRequested,
    WindowFocused(bool),
    Dropped(Vec<PathBuf>),
    RemoteOpen(Vec<crate::Target>),
    Script(script::Step),
    ThemeChanged,
}

pub struct App {
    pub settings: Settings,
    pub base_font_size: f32,
    pub metrics: Metrics,
    pub keymap: Keymap,
    pub docs: HashMap<DocId, DocEntry>,
    pub tabs: HashMap<TabId, Tab>,
    pub groups: Vec<Group>,
    pub active_group: usize,
    pub next_id: u64,
    pub focus: Focus,
    pub folder: Option<PathBuf>,
    pub explorer: Explorer,
    pub search: SearchPanel,
    pub git: GitPanel,
    pub tasks: TaskPanel,
    pub debug: DebugPanel,
    pub system_mono: Option<String>,
    pub vim: tinta_core::vim::Vim,
    pub vim_line: Option<(char, String)>,
    pub vim_highlight: Option<String>,
    pub sidebar: Option<SidebarView>,
    pub last_sidebar: SidebarView,
    pub sidebar_w: f32,
    pub panel: Option<PanelView>,
    pub panel_h: f32,
    pub terms: Vec<TermInfo>,
    pub active_term: usize,
    pub find: FindState,
    pub palette: Option<PaletteState>,
    pub completion: Option<Completion>,
    pub hover: Option<HoverPopup>,
    pub signature: Option<SignaturePopup>,
    pub dialog: Option<DialogState>,
    pub dialog_input: String,
    pub toast: Option<String>,
    pub toast_gen: u64,
    pub lsp: HashMap<String, lsp::LspEntry>,
    pub lsp_pending: HashMap<(String, i64), lsp::Pending>,
    pub lsp_progress: Option<String>,
    pub project_diags: HashMap<PathBuf, Vec<core_lsp::Diagnostic>>,
    pub output: Vec<String>,
    pub closed_tabs: Vec<PathBuf>,
    pub recent_files: Vec<PathBuf>,
    pub recent_folders: Vec<PathBuf>,
    pub nav_back: Vec<(PathBuf, usize)>,
    pub nav_forward: Vec<(PathBuf, usize)>,
    pub clip_meta: Option<(String, Vec<String>, bool)>,
    pub zen: bool,
    pub fullscreen: bool,
    pub syntax_theme: Arc<syntect::highlighting::Theme>,
    /// Whether the syntax colors are the dark ones right now.
    pub dark: bool,
    /// What the desktop prefers.
    pub dark_pref: bool,
    pub edit_tab: Option<TabId>,
    pub want_reveal: bool,
    pub watcher: Option<notify::RecommendedWatcher>,
    pub watched: Vec<PathBuf>,
    pub file_index: Arc<Vec<String>>,
    pub file_index_loading: bool,
    pub settings_open: bool,
    pub settings_mtime: Option<std::time::SystemTime>,
    pub keymap_mtime: Option<std::time::SystemTime>,
    pub font_names: Vec<String>,
    pub syntax_theme_names: Vec<String>,
    pub modifiers: Modifiers,
    /// Set when the last key isn't the app's, so the focused widget gets it: a terminal, a field.
    pub key_passthrough: bool,
    pub shortcuts_filter: String,
    pub title: String,
    pub session_dirty: Option<Instant>,
    pub user_snippets: HashMap<String, Vec<tinta_core::snippet::Definition>>,
    pub new_session: bool,
    pub groups_changed: std::cell::Cell<bool>,
    pub groups_flip: std::cell::Cell<bool>,
}

impl App {
    pub fn new(flags: Flags) -> (Self, Task) {
        let (settings, settings_err) = Settings::load();
        let mut keymap = Keymap::with_defaults();
        let mut key_errs = Vec::new();
        let keymap_mtime = file_mtime(&tinta_core::settings::keybindings_path());
        if let Ok(src) = std::fs::read_to_string(tinta_core::settings::keybindings_path()) {
            key_errs = keymap.apply_user(&src);
        }
        let dark = flags.dark;
        let syntax_theme = tinta_core::syntax::theme_by_name(&settings.syntax_theme, dark);
        let metrics = Metrics::new(&editor_font(&settings, flags.system_mono.as_deref()), settings.font_size, settings.line_height);
        let mut app = App {
            base_font_size: settings.font_size,
            metrics,
            keymap,
            docs: HashMap::new(),
            tabs: HashMap::new(),
            groups: vec![Group::new()],
            active_group: 0,
            next_id: 1,
            focus: Focus::Editor,
            folder: None,
            explorer: Explorer::default(),
            search: SearchPanel::default(),
            git: GitPanel::default(),
            tasks: TaskPanel::default(),
            debug: DebugPanel::default(),
            system_mono: flags.system_mono.clone(),
            vim: tinta_core::vim::Vim::new(),
            vim_line: None,
            vim_highlight: None,
            sidebar: None,
            last_sidebar: SidebarView::Explorer,
            sidebar_w: 280.0,
            panel: None,
            panel_h: 260.0,
            terms: Vec::new(),
            active_term: 0,
            find: FindState::default(),
            palette: None,
            completion: None,
            hover: None,
            signature: None,
            dialog: None,
            dialog_input: String::new(),
            toast: None,
            toast_gen: 0,
            lsp: HashMap::new(),
            lsp_pending: HashMap::new(),
            lsp_progress: None,
            project_diags: HashMap::new(),
            output: Vec::new(),
            closed_tabs: Vec::new(),
            recent_files: Vec::new(),
            recent_folders: Vec::new(),
            nav_back: Vec::new(),
            nav_forward: Vec::new(),
            clip_meta: None,
            zen: false,
            fullscreen: false,
            syntax_theme,
            dark,
            dark_pref: dark,
            edit_tab: None,
            want_reveal: false,
            watcher: None,
            watched: Vec::new(),
            file_index: Arc::new(Vec::new()),
            file_index_loading: false,
            settings_open: false,
            settings_mtime: file_mtime(&tinta_core::settings::settings_path()),
            keymap_mtime,
            font_names: if flags.font_names.is_empty() { vec!["Monospace".into()] } else { flags.font_names.clone() },
            syntax_theme_names: tinta_core::syntax::theme_names(),
            modifiers: Modifiers::default(),
            key_passthrough: false,
            shortcuts_filter: String::new(),
            title: "Tinta".into(),
            session_dirty: None,
            user_snippets: load_user_snippets(),
            new_session: flags.new_session,
            groups_changed: std::cell::Cell::new(false),
            groups_flip: std::cell::Cell::new(false),
            settings,
        };
        if let Some(e) = settings_err {
            app.output.push(format!("settings.json: {e}"));
            app.toast = Some(format!("settings.json: {e}"));
        }
        for e in key_errs {
            app.output.push(format!("keybindings.json: {e}"));
        }
        let mut tasks = vec![app.restore(flags.targets)];
        tasks.push(app.update_title());
        script::start();
        (app, Task::batch(tasks))
    }

    pub fn update(&mut self, message: Message) -> Task {
        let task = self.handle(message);
        let after = self.after_update();
        Task::batch([task, after])
    }
}

/// The editor's font: the one in the settings, unless it's the default and the desktop has one.
pub fn editor_font(settings: &Settings, system_mono: Option<&str>) -> String {
    match system_mono {
        Some(m) if settings.font_family == Settings::default().font_family && !m.is_empty() => m.to_string(),
        _ => settings.font_family.clone(),
    }
}

impl App {
    pub fn editor_font_family(&self) -> String {
        editor_font(&self.settings, self.system_mono.as_deref())
    }
}

pub fn file_mtime(p: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn load_user_snippets() -> HashMap<String, Vec<tinta_core::snippet::Definition>> {
    let Ok(src) = std::fs::read_to_string(tinta_core::settings::snippets_path()) else { return HashMap::new() };
    serde_json::from_str(&tinta_core::settings::strip_jsonc(&src)).unwrap_or_default()
}

impl App {
    pub fn pt(&self) -> bool {
        crate::i18n::is_portuguese()
    }

    pub fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn active_tab_id(&self) -> Option<TabId> {
        self.groups.get(self.active_group).and_then(Group::active_tab)
    }

    pub fn active(&self) -> Option<(&Tab, &DocEntry)> {
        let t = self.tabs.get(&self.active_tab_id()?)?;
        Some((t, self.docs.get(&t.doc)?))
    }

    pub fn active_mut(&mut self) -> Option<(&mut Tab, &mut DocEntry)> {
        let id = self.active_tab_id()?;
        let t = self.tabs.get_mut(&id)?;
        let d = self.docs.get_mut(&t.doc)?;
        Some((t, d))
    }

    /// A short notice. The window shows it and takes it away by itself.
    pub fn toast(&mut self, text: impl Into<String>) -> Task {
        self.toast = Some(text.into());
        self.toast_gen += 1;
        Task::none()
    }

    pub fn needs_frame_tick(&self) -> bool {
        for g in &self.groups {
            if let Some(d) = g.active_tab().and_then(|t| self.tabs.get(&t)).and_then(|t| t.diff.as_ref()) {
                for side in [&d.left, &d.right] {
                    if side.highlighter.as_ref().is_some_and(|h| !h.is_done(side.len_lines())) {
                        return true;
                    }
                }
            }
            if let Some(t) = g.active_tab().and_then(|t| self.tabs.get(&t))
                && let Some(e) = self.docs.get(&t.doc)
                && let Some(h) = &e.doc.highlighter
                && !h.is_done(e.doc.len_lines())
            {
                return true;
            }
        }
        false
    }

    pub fn mark_session_dirty(&mut self) {
        if self.session_dirty.is_none() {
            self.session_dirty = Some(Instant::now());
        }
    }

    fn handle(&mut self, message: Message) -> Task {
        match message {
            Message::Noop => Task::none(),
            Message::Key(ev) => self.on_key(ev),
            Message::Modifiers(m) => {
                self.modifiers = m;
                Task::none()
            }
            Message::InputFocus(on) => {
                if on {
                    self.focus = Focus::Input;
                } else if self.focus == Focus::Input && self.palette.is_none() && self.dialog.is_none() {
                    self.focus = Focus::Editor;
                }
                Task::none()
            }
            Message::Cmd(c) => self.run(c),
            Message::Editor(tab, ev) => self.on_editor_event(tab, ev),
            Message::Diff(tab, ev) => self.on_diff_event(tab, ev),
            Message::TabActivate(g, e) => {
                self.active_group = g.min(self.groups.len() - 1);
                self.groups[self.active_group].model.activate(e);
                self.focus = Focus::Editor;
                self.close_popups();
                self.on_active_changed()
            }
            Message::TabMoved(g, e, to) => {
                if let Some(gr) = self.groups.get_mut(g) {
                    gr.model.position_set(e, to as u16);
                    self.mark_session_dirty();
                }
                Task::none()
            }
            Message::TabClose(g, e) => {
                let Some(id) = self.groups.get(g).and_then(|gr| gr.model.data::<TabId>(e).copied()) else {
                    return Task::none();
                };
                self.request_close_tab(id)
            }
            Message::SidebarShow(v) => self.toggle_sidebar_view(v),
            Message::SidebarResize(d) => {
                self.sidebar_w = (self.sidebar_w + d).clamp(150.0, 800.0);
                self.mark_session_dirty();
                Task::none()
            }
            Message::PanelResize(d) => {
                self.panel_h = (self.panel_h - d).clamp(80.0, 900.0);
                self.mark_session_dirty();
                Task::none()
            }
            Message::GroupResize(g, d) => {
                self.resize_groups(g, d);
                Task::none()
            }
            Message::PanelShow(p) => {
                self.panel = Some(p);
                if p == PanelView::Terminal {
                    return self.ensure_terminal();
                }
                Task::none()
            }
            Message::PanelClose => {
                self.panel = None;
                if self.focus == Focus::Terminal {
                    self.focus = Focus::Editor;
                }
                Task::none()
            }
            Message::TickFrame => {
                self.advance_highlighting(Duration::from_millis(5));
                Task::none()
            }
            Message::TickSlow => self.tick_slow(),
            Message::Toast(text) => {
                if let Some(g) = text.strip_prefix("\0clear") {
                    if g.parse::<u64>().ok() == Some(self.toast_gen) {
                        self.toast = None;
                    }
                    return Task::none();
                }
                self.toast(text)
            }
            Message::OpenFolderPath(p) => self.open_folder(p),
            Message::OpenFiles(ps) => {
                let mut tasks = Vec::new();
                for p in ps {
                    tasks.push(self.open_path(&p));
                }
                Task::batch(tasks)
            }
            Message::SaveAsTo(tab, p) => self.save_tab_as(tab, p),
            Message::Pasted(text) => self.on_pasted(text),
            Message::CopyText(s) => fx::clipboard_write(s),
            Message::Find(m) => self.on_find(m),
            Message::Palette(m) => self.on_palette(m),
            Message::Explorer(m) => self.on_explorer(m),
            Message::Search(m) => self.on_search_msg(m),
            Message::Git(m) => self.on_git(m),
            Message::Task(m) => self.on_task(m),
            Message::Vim(m) => self.on_vim(m),
            Message::Debug(m) => self.on_debug(m),
            Message::StyleChanged { dark, mono } => {
                let font_changed = mono != self.system_mono;
                self.system_mono = mono;
                if font_changed {
                    self.apply_font();
                }
                self.dark_pref = dark;
                self.refresh_syntax_theme();
                Task::none()
            }
            Message::TermLink(text, col) => self.on_term_link(text, col),
            Message::OutlineJump(line) => {
                self.push_nav();
                self.jump_to(line, 0);
                self.focus = Focus::Editor;
                Task::none()
            }
            Message::CompletionPick(i) => {
                if let Some(c) = &mut self.completion {
                    c.selected = i;
                }
                self.accept_completion()
            }
            Message::Lsp(ev) => self.on_lsp_event(ev),
            Message::Dialog(m) => self.on_dialog(m),
            Message::Setting(m) => self.on_setting(m),
            Message::SettingsClose => {
                self.settings_open = false;
                Task::none()
            }
            Message::FsEvent(paths) => self.on_fs_event(paths),
            Message::TermTitle(id, t) => {
                if let Some(s) = self.terms.iter_mut().find(|s| s.id == id) {
                    s.title = t.unwrap_or_default();
                }
                Task::none()
            }
            Message::TermExited(id) => {
                if let Some(i) = self.terms.iter().position(|s| s.id == id) {
                    self.terms.remove(i);
                    self.active_term = self.active_term.min(self.terms.len().saturating_sub(1));
                    if self.terms.is_empty() {
                        self.panel = None;
                        self.focus = Focus::Editor;
                    }
                }
                Task::none()
            }
            Message::TermFocus => {
                self.focus = Focus::Terminal;
                Task::none()
            }
            Message::TermSelect(i) => {
                self.active_term = i.min(self.terms.len().saturating_sub(1));
                self.focus = Focus::Terminal;
                Task::none()
            }
            Message::TermKill(i) => {
                if i < self.terms.len() {
                    let t = self.terms.remove(i);
                    let kill = Task::effect(fx::Effect::TermKill(t.id));
                    self.active_term = self.active_term.min(self.terms.len().saturating_sub(1));
                    if self.terms.is_empty() {
                        self.panel = None;
                        self.focus = Focus::Editor;
                    }
                    return kill;
                }
                Task::none()
            }
            Message::ProblemJump(p, l, c) => {
                self.push_nav();
                let t = self.open_path(&p);
                self.jump_to(l, c);
                self.focus = Focus::Editor;
                t
            }
            Message::CloseRequested => self.request_quit(),
            Message::WindowFocused(on) => {
                if !on && self.settings.auto_save == AutoSave::Focus {
                    return self.save_all_dirty_named();
                }
                Task::none()
            }
            Message::Dropped(paths) => {
                let mut tasks = Vec::new();
                for p in paths {
                    if p.is_dir() {
                        tasks.push(self.open_folder(p));
                    } else {
                        tasks.push(self.open_path(&p));
                    }
                }
                Task::batch(tasks)
            }
            Message::RemoteOpen(targets) => {
                let mut tasks = Vec::new();
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
                Task::batch(tasks)
            }
            Message::Script(s) => self.on_script(s),
            Message::ThemeChanged => {
                self.refresh_syntax_theme();
                Task::none()
            }
        }
    }

    fn resize_groups(&mut self, g: usize, d: f32) {
        if g + 1 >= self.groups.len() {
            return;
        }
        let total_w = self.tabs_area_width().max(100.0);
        let total_weight: f32 = self.groups.iter().map(|g| g.weight).sum();
        let dw = d / total_w * total_weight;
        let min = 0.1 * total_weight;
        let a = (self.groups[g].weight + dw).max(min);
        let b = (self.groups[g + 1].weight - dw).max(min);
        self.groups[g].weight = a;
        self.groups[g + 1].weight = b;
    }

    fn tabs_area_width(&self) -> f32 {
        self.groups
            .iter()
            .filter_map(|g| g.active_tab().and_then(|t| self.tabs.get(&t)).map(|t| t.view_w))
            .sum()
    }

    /// Dark or light, as the settings and the desktop decide together.
    pub fn is_dark(&self) -> bool {
        match self.settings.ui_theme {
            AppTheme::System => self.dark_pref,
            AppTheme::Dark => true,
            AppTheme::Light => false,
        }
    }

    pub fn refresh_syntax_theme(&mut self) {
        let dark = self.is_dark();
        let name = self.settings.syntax_theme.clone();
        if dark == self.dark && self.syntax_theme.name.as_deref() == Some(theme_name_for(&name, dark).as_str()) {
            return;
        }
        self.dark = dark;
        self.syntax_theme = tinta_core::syntax::theme_by_name(&name, dark);
        for e in self.docs.values_mut() {
            docs::attach_language(&mut e.doc, self.syntax_theme.clone());
        }
        for d in self.tabs.values_mut().filter_map(|t| t.diff.as_deref_mut()) {
            for side in [&mut d.left, &mut d.right] {
                side.highlighter = tinta_core::syntax::Highlighter::new(&side.language.name, self.syntax_theme.clone());
            }
        }
    }

    /// The window follows `settings.ui_theme` on its own; the text's colors follow here.
    pub fn apply_ui_theme(&mut self) -> Task {
        self.refresh_syntax_theme();
        Task::none()
    }
}

fn theme_name_for(name: &str, dark: bool) -> String {
    tinta_core::syntax::theme_by_name(name, dark).name.clone().unwrap_or_default()
}

pub fn save_session(app: &App) {
    let s = app.build_session();
    if let Err(e) = s.save() {
        log::warn!("could not save the session: {e}");
    }
}
