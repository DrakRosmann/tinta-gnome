//! The sidebar: files, search across the folder, source control, the file's outline, debugging.

use super::Shell;
use super::rows::{self, RowList};
use crate::app::{App, ExplorerMsg, Focus, symbols_of, GitMsg, Message, RowAction, SEARCH_INPUT, SearchMsg, SearchRow, SidebarView};
use crate::i18n::tr;
use crate::state::{DocEntry, Tab};
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use tinta_core::git::StatusEntry;
use tinta_core::keymap::Command;

pub struct Sidebar {
    pub root: gtk::Box,
    shell: Weak<Shell>,
    stack: gtk::Stack,
    explorer_title: gtk::Label,
    explorer: RowList,
    explorer_empty: adw::StatusPage,
    search_query: gtk::Entry,
    search_replace: gtk::Entry,
    search_replace_row: gtk::Box,
    search_include: gtk::Entry,
    search_exclude: gtk::Entry,
    search_toggles: [gtk::ToggleButton; 4],
    search_summary: gtk::Label,
    search: RowList,
    git_message: gtk::Entry,
    git_commit: gtk::Button,
    git_list: gtk::ListBox,
    /// The git list's file rows, in `App::git_rows` order.
    git_rows: RefCell<Vec<gtk::ListBoxRow>>,
    git_form: gtk::Box,
    git_none: gtk::Label,
    outline: RowList,
    debug: super::debug::DebugSidebar,
    last: RefCell<Last>,
}

#[derive(Default)]
struct Last {
    view: Option<SidebarView>,
    git: u64,
    /// The tree's keyboard row last scrolled to and focused.
    explorer_cursor: Option<usize>,
    /// The same for the other lists, with the view it was in.
    list_cursor: Option<(SidebarView, usize)>,
}

fn view_name(v: SidebarView) -> &'static str {
    match v {
        SidebarView::Explorer => "explorer",
        SidebarView::Search => "search",
        SidebarView::Git => "git",
        SidebarView::Outline => "outline",
        SidebarView::Debug => "debug",
    }
}

fn header(title: &gtk::Label, buttons: &[gtk::Button]) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    b.set_margin_start(12);
    b.set_margin_end(6);
    b.set_margin_top(6);
    b.set_margin_bottom(4);
    title.add_css_class("heading");
    title.set_hexpand(true);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    b.append(title);
    for x in buttons {
        b.append(x);
    }
    b
}

fn small(shell: &Rc<Shell>, icon: &str, tip: &str, msg: impl Fn() -> Message + 'static) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tip));
    let s = Rc::downgrade(shell);
    b.connect_clicked(move |_| {
        if let Some(s) = s.upgrade() {
            s.dispatch(msg());
        }
    });
    b
}

/// The explorer's less used actions, and its two filters, in a popover.
fn explorer_more(shell: &Rc<Shell>) -> gtk::MenuButton {
    use crate::app::{BoolSetting, SettingMsg};
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let pop = gtk::Popover::builder().child(&body).has_arrow(true).build();
    pop.add_css_class("menu");
    let entry = |icon: &str, label: &str, msg: fn() -> Message| {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&gtk::Image::from_icon_name(icon));
        row.append(&gtk::Label::new(Some(label)));
        let b = gtk::Button::builder().child(&row).build();
        b.add_css_class("flat");
        let (s, p) = (Rc::downgrade(shell), pop.clone());
        b.connect_clicked(move |_| {
            p.popdown();
            if let Some(s) = s.upgrade() {
                s.dispatch(msg());
            }
        });
        body.append(&b);
    };
    entry("folder-new-symbolic", tr("New Folder"), || Message::Explorer(ExplorerMsg::NewFolder(None)));
    entry("view-refresh-symbolic", tr("Refresh"), || Message::Explorer(ExplorerMsg::Refresh));
    entry("view-restore-symbolic", tr("Collapse All"), || Message::Explorer(ExplorerMsg::CollapseAll));
    body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let toggles: Vec<(gtk::CheckButton, BoolSetting)> = [("Show hidden files", BoolSetting::ShowHidden), ("Hide compiled programs", BoolSetting::HideBuildOutputs)]
        .into_iter()
        .map(|(label, key)| {
            let c = gtk::CheckButton::with_label(tr(label));
            c.set_margin_start(6);
            c.set_margin_end(6);
            body.append(&c);
            (c, key)
        })
        .collect();
    // The checks follow the settings each time the menu opens; a click is a user's change only
    // after that.
    let syncing = Rc::new(std::cell::Cell::new(false));
    for (c, key) in &toggles {
        let (s, key, syncing) = (Rc::downgrade(shell), *key, syncing.clone());
        c.connect_toggled(move |c| {
            if syncing.get() {
                return;
            }
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Setting(SettingMsg::Bool(key, c.is_active())));
            }
        });
    }
    let s = Rc::downgrade(shell);
    pop.connect_show(move |_| {
        let Some(s) = s.upgrade() else { return };
        let Ok(app) = s.app.try_borrow() else { return };
        syncing.set(true);
        for (c, key) in &toggles {
            c.set_active(match key {
                BoolSetting::ShowHidden => app.settings.show_hidden_files,
                _ => app.settings.hide_build_outputs,
            });
        }
        syncing.set(false);
    });
    let b = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text(tr("More")).popover(&pop).build();
    b.add_css_class("flat");
    b
}

fn git_class(mark: char) -> &'static str {
    match mark {
        'A' | '?' => "success",
        'D' | 'U' => "error",
        _ => "warning",
    }
}


/// The symbols around the caret, outermost first.
pub fn current_symbols(e: &DocEntry, t: &Tab) -> Vec<String> {
    let line = e.doc.char_to_line(t.cursors.head());
    let mut chain: Vec<(usize, &str, Option<usize>)> = Vec::new();
    for s in e.symbols.iter().take_while(|s| s.line <= line) {
        while chain.last().is_some_and(|(d, _, _)| *d >= s.depth) {
            chain.pop();
        }
        chain.push((s.depth, s.name.as_str(), s.end));
    }
    let tab = e.doc.indent.width.max(1);
    let indent = tinta_core::display::line_indent_vcols(&e.doc, line, tab) / tab;
    let blank = e.doc.line_is_blank(line);
    let on_header = e.symbols.iter().any(|s| s.line == line);
    chain.retain(|(d, _, end)| match end {
        Some(end) => line <= *end,
        None => blank || *d < indent || (on_header && *d == indent),
    });
    chain.into_iter().map(|(_, n, _)| n.to_string()).collect()
}

/// Where the caret is: the file's path in the folder, then the symbols around it.
pub fn breadcrumbs(app: &App, group: usize) -> String {
    let Some(t) = app.groups.get(group).and_then(|g| g.active_tab()).and_then(|id| app.tabs.get(&id)) else { return String::new() };
    let Some(e) = app.docs.get(&t.doc) else { return String::new() };
    let mut parts: Vec<String> = match &e.doc.path {
        // Inside the open folder, the way down to the file; anywhere else, just its name.
        Some(p) => match app.folder.as_ref().and_then(|f| p.strip_prefix(f).ok()) {
            Some(rel) => rel.iter().map(|c| c.to_string_lossy().into_owned()).collect(),
            None => vec![e.doc.file_name()],
        },
        None => vec![e.doc.file_name()],
    };
    parts.extend(current_symbols(e, t));
    parts.join("  ›  ")
}

impl Sidebar {
    pub fn shell(&self) -> Option<Rc<Shell>> {
        self.shell.upgrade()
    }

    pub fn fields(&self) -> Vec<(&'static str, gtk::Widget)> {
        let mut v = vec![(SEARCH_INPUT, self.search_query.clone().upcast()), (crate::app::GIT_INPUT, self.git_message.clone().upcast())];
        v.extend(self.debug.fields());
        v
    }

    pub fn build(shell: &Rc<Shell>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("tinta-sidebar");
        root.set_size_request(180, -1);
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        // Only the page shown counts for the width: the widest of the others would keep the
        // sidebar, and a narrow window, wider than the page in view needs.
        stack.set_hhomogeneous(false);
        root.append(&stack);

        // Files.
        let explorer_title = gtk::Label::new(Some(tr("Explorer")));
        let bind_shell = Rc::downgrade(shell);
        let bind: rows::Bind = Rc::new(move |b, i| {
            let Some(s) = bind_shell.upgrade() else { return };
            let app = s.app.borrow();
            let Some(r) = app.explorer.rows.get(i) else { return };
            b.append(&rows::spacer(r.depth as i32 * 14));
            let chevron = gtk::Image::from_icon_name(if r.expanded { "pan-down-symbolic" } else { "pan-end-symbolic" });
            chevron.set_opacity(if r.is_dir { 0.7 } else { 0.0 });
            b.append(&chevron);
            b.append(&rows::file_icon(&r.name, r.is_dir, r.expanded));
            let name = rows::label(&r.name);
            name.set_hexpand(true);
            if let Some(m) = r.git {
                name.add_css_class(git_class(m));
            } else if r.ignored {
                name.add_css_class("dim-label");
            }
            let active = app.active().and_then(|(_, d)| d.doc.path.as_ref()) == Some(&r.path);
            if active {
                name.add_css_class("heading");
            }
            if app.focus == Focus::Explorer && app.explorer.selected.as_ref() == Some(&r.path) {
                b.add_css_class("tinta-cursor");
            }
            b.append(&name);
            if let Some(m) = r.git.filter(|_| !r.is_dir) {
                let l = gtk::Label::new(Some(&m.to_string()));
                l.add_css_class(git_class(m));
                l.add_css_class("caption");
                b.append(&l);
            }
        });
        let s = Rc::downgrade(shell);
        let secondary_shell = Rc::downgrade(shell);
        let secondary: Rc<dyn Fn(usize, &gtk::Widget, f64, f64)> = Rc::new(move |i, w, x, y| {
            let Some(s) = secondary_shell.upgrade() else { return };
            let is_dir = s.app.borrow().explorer.rows.get(i).is_some_and(|r| r.is_dir);
            row_menu(&s, i, is_dir, w, x, y);
        });
        let explorer = RowList::new(
            bind,
            move |i| {
                if let Some(s) = s.upgrade() {
                    s.dispatch(Message::Explorer(ExplorerMsg::Click(i)));
                }
            },
            Some(secondary),
        );
        let explorer_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // Two buttons in sight leave room for the folder's name; the rest wait under "⋯".
        let explorer_header = header(
            &explorer_title,
            &[
                small(shell, "document-new-symbolic", tr("New File"), || Message::Explorer(ExplorerMsg::NewFile(None))),
                small(shell, "folder-open-symbolic", tr("Open Folder"), || Message::Cmd(Command::OpenFolder)),
            ],
        );
        explorer_header.append(&explorer_more(shell));
        explorer_page.append(&explorer_header);
        let explorer_empty = adw::StatusPage::builder().icon_name("folder-symbolic").title(tr("No folder open")).vexpand(true).build();
        explorer_empty.add_css_class("compact");
        let open = gtk::Button::with_label(tr("Open Folder"));
        open.add_css_class("pill");
        open.add_css_class("suggested-action");
        open.set_halign(gtk::Align::Center);
        let s = Rc::downgrade(shell);
        open.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Cmd(Command::OpenFolder));
            }
        });
        explorer_empty.set_child(Some(&open));
        explorer_page.append(&explorer_empty);
        explorer_page.append(&explorer.scroll);
        stack.add_named(&explorer_page, Some("explorer"));

        // Search.
        let search_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        search_page.append(&header(&gtk::Label::new(Some(tr("Search"))), &[small(shell, "view-refresh-symbolic", tr("Refresh"), || Message::Search(SearchMsg::Run))]));
        let form = gtk::Box::new(gtk::Orientation::Vertical, 6);
        form.set_margin_start(10);
        form.set_margin_end(10);
        let search_query = rows::entry(shell, tr("Find"), |t| Message::Search(SearchMsg::Query(t)));
        down_to_list(shell, &search_query, SidebarView::Search);
        let s = Rc::downgrade(shell);
        search_query.connect_activate(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Search(SearchMsg::Run));
            }
        });
        form.append(&search_query);
        let toggles_box = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let mk = |label: &str, tip: &str, msg: SearchMsg| {
            let b = gtk::ToggleButton::with_label(label);
            b.add_css_class("flat");
            b.set_tooltip_text(Some(tip));
            let s = Rc::downgrade(shell);
            b.connect_clicked(move |b| {
                if let Some(s) = s.upgrade()
                    && !b.has_css_class("syncing")
                {
                    s.dispatch(Message::Search(msg.clone()));
                }
            });
            toggles_box.append(&b);
            b
        };
        let search_toggles = [
            mk("⇄", tr("Toggle replace"), SearchMsg::ToggleReplace),
            mk("Aa", tr("Match case"), SearchMsg::ToggleCase),
            mk("ab", tr("Whole word"), SearchMsg::ToggleWord),
            mk(".*", tr("Regular expression"), SearchMsg::ToggleRegex),
        ];
        form.append(&toggles_box);
        let search_replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let search_replace = rows::entry(shell, tr("Replace"), |t| Message::Search(SearchMsg::Replacement(t)));
        search_replace_row.append(&search_replace);
        let all = gtk::Button::with_label(tr("Replace All"));
        let s = Rc::downgrade(shell);
        all.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Search(SearchMsg::ReplaceAll));
            }
        });
        search_replace_row.append(&all);
        form.append(&search_replace_row);
        let search_include = rows::entry(shell, tr("files to include"), |t| Message::Search(SearchMsg::Include(t)));
        let search_exclude = rows::entry(shell, tr("files to exclude"), |t| Message::Search(SearchMsg::Exclude(t)));
        form.append(&search_include);
        form.append(&search_exclude);
        let search_summary = rows::dim("");
        form.append(&search_summary);
        search_page.append(&form);
        let bind_shell = Rc::downgrade(shell);
        let bind: rows::Bind = Rc::new(move |b, i| {
            let Some(s) = bind_shell.upgrade() else { return };
            let app = s.app.borrow();
            if list_cursor(&app, SidebarView::Search) == Some(i) {
                b.add_css_class("tinta-cursor");
            }
            match app.search.rows.get(i) {
                Some(SearchRow::File { path, count, collapsed }) => {
                    let rel = app.folder.as_ref().and_then(|f| path.strip_prefix(f).ok()).unwrap_or(path);
                    let name = rel.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let dir = rel.parent().map(|d| d.display().to_string()).unwrap_or_default();
                    b.append(&gtk::Image::from_icon_name(if *collapsed { "pan-end-symbolic" } else { "pan-down-symbolic" }));
                    b.append(&rows::file_icon(&name, false, false));
                    b.append(&rows::label(&name));
                    let d = rows::dim(&dir);
                    d.set_hexpand(true);
                    b.append(&d);
                    b.append(&rows::dim(&count.to_string()));
                }
                Some(SearchRow::Match { text, start, end, .. }) => {
                    let ws = text.chars().take_while(|c| c.is_whitespace()).count();
                    let from = start.saturating_sub(16).max(ws);
                    let chars: Vec<char> = text.chars().collect();
                    let esc = |a: usize, z: usize| glib::markup_escape_text(&chars[a.min(chars.len())..z.min(chars.len())].iter().collect::<String>()).to_string();
                    let markup = format!("{}<b>{}</b>{}", esc(from, *start), esc(*start, *end), esc(*end, chars.len()));
                    b.append(&rows::spacer(22));
                    let l = gtk::Label::new(None);
                    l.set_markup(&markup);
                    l.set_xalign(0.0);
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    l.set_hexpand(true);
                    b.append(&l);
                }
                None => {}
            }
        });
        let s = Rc::downgrade(shell);
        let search = RowList::new(
            bind,
            move |i| {
                let Some(s) = s.upgrade() else { return };
                let msg = match s.app.borrow().search.rows.get(i) {
                    Some(SearchRow::File { path, .. }) => Message::Search(SearchMsg::ToggleFile(path.clone())),
                    Some(SearchRow::Match { path, line, start, end, .. }) => Message::Search(SearchMsg::Open(path.clone(), *line, *start, *end)),
                    None => return,
                };
                s.dispatch(msg);
            },
            None,
        );
        search_page.append(&search.scroll);
        stack.add_named(&search_page, Some("search"));

        // Source control.
        let git_page = gtk::Box::new(gtk::Orientation::Vertical, 6);
        git_page.append(&header(
            &gtk::Label::new(Some(tr("Source Control"))),
            &[
                small(shell, "go-down-symbolic", tr("Pull"), || Message::Cmd(Command::GitPull)),
                small(shell, "go-up-symbolic", tr("Push"), || Message::Cmd(Command::GitPush)),
                small(shell, "view-refresh-symbolic", tr("Refresh status"), || Message::Cmd(Command::GitRefresh)),
            ],
        ));
        let git_form = gtk::Box::new(gtk::Orientation::Vertical, 6);
        git_form.set_margin_start(10);
        git_form.set_margin_end(10);
        let git_message = rows::entry(shell, tr("Message"), |t| Message::Git(GitMsg::MessageInput(t)));
        down_to_list(shell, &git_message, SidebarView::Git);
        let s = Rc::downgrade(shell);
        git_message.connect_activate(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Git(GitMsg::Commit));
            }
        });
        let git_commit = gtk::Button::with_label(tr("Commit"));
        git_commit.add_css_class("suggested-action");
        let s = Rc::downgrade(shell);
        git_commit.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Git(GitMsg::Commit));
            }
        });
        git_form.append(&git_message);
        git_form.append(&git_commit);
        git_page.append(&git_form);
        let git_none = rows::dim(tr("Not a git repository"));
        git_none.set_margin_start(14);
        git_page.append(&git_none);
        let git_list = gtk::ListBox::new();
        git_list.add_css_class("navigation-sidebar");
        git_list.set_selection_mode(gtk::SelectionMode::None);
        git_page.append(&gtk::ScrolledWindow::builder().child(&git_list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build());
        stack.add_named(&git_page, Some("git"));

        // Outline.
        let bind_shell = Rc::downgrade(shell);
        let bind: rows::Bind = Rc::new(move |b, i| {
            let Some(s) = bind_shell.upgrade() else { return };
            let app = s.app.borrow();
            let Some((t, e)) = app.active() else { return };
            if list_cursor(&app, SidebarView::Outline) == Some(i) {
                b.add_css_class("tinta-cursor");
            }
            let symbols = symbols_of(e);
            let Some((name, kind, line, depth)) = symbols.get(i) else { return };
            let caret = e.doc.char_to_line(t.cursors.head());
            let current = symbols.iter().rposition(|s| s.2 <= caret) == Some(i);
            b.append(&rows::spacer(*depth as i32 * 12 + 6));
            let n = rows::label(name);
            if current {
                n.add_css_class("heading");
            }
            b.append(&n);
            let k = rows::dim(kind);
            k.set_hexpand(true);
            b.append(&k);
            let _ = line;
        });
        let s = Rc::downgrade(shell);
        let outline = RowList::new(
            bind,
            move |i| {
                let Some(s) = s.upgrade() else { return };
                let line = s.app.borrow().active().and_then(|(_, e)| symbols_of(e).get(i).map(|x| x.2));
                if let Some(l) = line {
                    s.dispatch(Message::OutlineJump(l));
                }
            },
            None,
        );
        let outline_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outline_page.append(&header(&gtk::Label::new(Some(tr("Outline"))), &[]));
        outline_page.append(&outline.scroll);
        stack.add_named(&outline_page, Some("outline"));

        let debug = super::debug::DebugSidebar::build(shell);
        stack.add_named(&debug.root, Some("debug"));

        Rc::new(Sidebar {
            root,
            shell: Rc::downgrade(shell),
            stack,
            explorer_title,
            explorer,
            explorer_empty,
            search_query,
            search_replace,
            search_replace_row,
            search_include,
            search_exclude,
            search_toggles,
            search_summary,
            search,
            git_message,
            git_commit,
            git_list,
            git_rows: RefCell::new(Vec::new()),
            git_form,
            git_none,
            outline,
            debug,
            last: RefCell::new(Last::default()),
        })
    }

    pub fn sync(&self, shell: &Rc<Shell>, app: &App) {
        let Some(view) = app.sidebar else { return };
        let mut last = self.last.borrow_mut();
        if last.view != Some(view) {
            self.stack.set_visible_child_name(view_name(view));
            last.view = Some(view);
        }
        match view {
            SidebarView::Explorer => {
                let e = &app.explorer;
                self.explorer_empty.set_visible(app.folder.is_none());
                self.explorer.scroll.set_visible(app.folder.is_some());
                let name = app.folder.as_ref().and_then(|f| f.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| tr("Explorer").to_string());
                self.explorer_title.set_label(&name);
                let active = app.active().and_then(|(_, d)| d.doc.path.clone());
                let keyboard = app.focus == Focus::Explorer;
                let key = rows::hash_of((e.rows.iter().map(|r| (&r.path, r.expanded, r.git, r.ignored)).collect::<Vec<_>>(), active, keyboard, &e.selected));
                let rebuilt = self.explorer.set(e.rows.len(), key);
                // The keyboard's row stays in view and holds GTK's focus, which a rebuilt list drops.
                let cursor = keyboard.then(|| e.selected.as_ref().and_then(|p| e.rows.iter().position(|r| r.path == *p)).unwrap_or(0)).filter(|_| !e.rows.is_empty());
                if cursor != last.explorer_cursor || (rebuilt && cursor.is_some()) {
                    if let Some(i) = cursor {
                        let view = self.explorer.view.clone();
                        glib::idle_add_local_once(move || view.scroll_to(i as u32, gtk::ListScrollFlags::FOCUS, None));
                    }
                    last.explorer_cursor = cursor;
                }
            }
            SidebarView::Search => {
                let s = &app.search;
                rows::set_text(&self.search_query, &s.query.pattern);
                rows::set_text(&self.search_replace, &s.replacement);
                rows::set_text(&self.search_include, &s.include);
                rows::set_text(&self.search_exclude, &s.exclude);
                for (b, on) in self.search_toggles.iter().zip([s.replace_open, s.query.case_sensitive, s.query.whole_word, s.query.regex]) {
                    if b.is_active() != on {
                        b.add_css_class("syncing");
                        b.set_active(on);
                        b.remove_css_class("syncing");
                    }
                }
                self.search_replace_row.set_visible(s.replace_open);
                let summary = if let Some(e) = &s.error {
                    e.clone()
                } else if s.running {
                    tr("Searching…").to_string()
                } else if s.query.pattern.is_empty() {
                    String::new()
                } else if s.total == 0 {
                    tr("No results").to_string()
                } else {
                    let plus = if s.truncated { "+" } else { "" };
                    format!("{}{plus} {} {} {}", s.total, tr("matches in"), s.results.len(), tr("files"))
                };
                self.search_summary.set_label(&summary);
                let cursor = list_cursor(app, SidebarView::Search);
                let key = rows::hash_of((s.generation, s.rows.len(), s.collapsed.len(), s.total, cursor));
                let rebuilt = self.search.set(s.rows.len(), key);
                follow(&self.search, SidebarView::Search, cursor, rebuilt, &mut last);
            }
            SidebarView::Git => {
                let g = &app.git;
                self.git_none.set_visible(g.root.is_none());
                self.git_form.set_visible(g.root.is_some());
                rows::set_text(&self.git_message, &g.message);
                self.git_commit.set_label(&format!("{} · {}", tr("Commit"), g.branch.clone().unwrap_or_default()));
                self.git_commit.set_sensitive(!g.message.trim().is_empty());
                let key = rows::hash_of(g.entries.iter().map(|e| (&e.path, e.index, e.worktree)).collect::<Vec<_>>());
                let rebuilt = key != last.git;
                if rebuilt {
                    last.git = key;
                    self.fill_git(shell, app);
                }
                // The keyboard's row: marked, and holding GTK's focus.
                let cursor = list_cursor(app, SidebarView::Git);
                if rebuilt || last.list_cursor != cursor.map(|c| (SidebarView::Git, c)) {
                    for (i, row) in self.git_rows.borrow().iter().enumerate() {
                        if Some(i) == cursor {
                            row.add_css_class("tinta-cursor");
                            let row = row.clone();
                            glib::idle_add_local_once(move || {
                                row.grab_focus();
                            });
                        } else {
                            row.remove_css_class("tinta-cursor");
                        }
                    }
                    last.list_cursor = cursor.map(|c| (SidebarView::Git, c));
                }
            }
            SidebarView::Outline => {
                let n = app.active().map_or(0, |(_, e)| symbols_of(e).len());
                let cursor = list_cursor(app, SidebarView::Outline);
                let key = app.active().map(|(t, e)| rows::hash_of((e.doc.revision, e.doc.char_to_line(t.cursors.head()), t.doc, e.lsp_symbols.as_ref().map(|(r, l)| (*r, l.len())), cursor))).unwrap_or(0);
                let rebuilt = self.outline.set(n, key);
                follow(&self.outline, SidebarView::Outline, cursor, rebuilt, &mut last);
            }
            SidebarView::Debug => {}
        }
        drop(last);
        self.debug.sync(shell, app);
    }

    fn fill_git(&self, shell: &Rc<Shell>, app: &App) {
        while let Some(c) = self.git_list.first_child() {
            self.git_list.remove(&c);
        }
        self.git_rows.borrow_mut().clear();
        let g = &app.git;
        let staged: Vec<&StatusEntry> = g.entries.iter().filter(|e| e.is_staged()).collect();
        let changed: Vec<&StatusEntry> = g.entries.iter().filter(|e| e.is_unstaged()).collect();
        let heading = |text: String, button: Option<gtk::Button>| {
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            b.set_margin_top(8);
            b.set_margin_start(6);
            let l = rows::label(&text);
            l.add_css_class("heading");
            l.set_hexpand(true);
            b.append(&l);
            if let Some(x) = button {
                b.append(&x);
            }
            let row = gtk::ListBoxRow::builder().child(&b).activatable(false).build();
            self.git_list.append(&row);
        };
        let entry = |e: &StatusEntry, staged: bool| {
            let mark = if staged { e.index } else if e.worktree == '?' { 'A' } else { e.worktree };
            let (name, dir) = match e.path.rsplit_once('/') {
                Some((d, n)) => (n.to_string(), d.to_string()),
                None => (e.path.clone(), String::new()),
            };
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            b.append(&rows::file_icon(&name, false, false));
            let n = rows::label(&name);
            n.add_css_class(git_class(mark));
            b.append(&n);
            let d = rows::dim(&dir);
            d.set_hexpand(true);
            b.append(&d);
            if staged {
                let p = e.path.clone();
                b.append(&small(shell, "list-remove-symbolic", tr("Unstage"), move || Message::Git(GitMsg::Unstage(p.clone()))));
            } else {
                let x = e.clone();
                b.append(&small(shell, "edit-undo-symbolic", tr("Discard"), move || Message::Git(GitMsg::Discard(x.clone()))));
                let p = e.path.clone();
                b.append(&small(shell, "list-add-symbolic", tr("Stage"), move || Message::Git(GitMsg::Stage(p.clone()))));
            }
            let m = gtk::Label::new(Some(&mark.to_string()));
            m.add_css_class(git_class(mark));
            m.set_width_chars(2);
            b.append(&m);
            let row = gtk::ListBoxRow::builder().child(&b).build();
            let s = Rc::downgrade(shell);
            let p = e.path.clone();
            let click = gtk::GestureClick::new();
            click.connect_released(move |_, n, _, _| {
                if n >= 1 && let Some(s) = s.upgrade() {
                    s.dispatch(Message::Git(GitMsg::Open(p.clone(), staged)));
                }
            });
            n.add_controller(click);
            self.git_list.append(&row);
            self.git_rows.borrow_mut().push(row);
        };
        if !staged.is_empty() {
            heading(format!("{} ({})", tr("Staged Changes"), staged.len()), None);
            for e in staged {
                entry(e, true);
            }
        }
        heading(format!("{} ({})", tr("Changes"), changed.len()), Some(small(shell, "list-add-symbolic", tr("Stage"), || Message::Git(GitMsg::StageAll))));
        for e in changed {
            entry(e, false);
        }
    }
}

/// A right click on a file or folder in the tree.
fn row_menu(shell: &Rc<Shell>, i: usize, is_dir: bool, w: &gtk::Widget, x: f64, y: f64) {
    let menu = gio::Menu::new();
    let actions = gio::SimpleActionGroup::new();
    let mut items: Vec<(&str, RowAction)> = Vec::new();
    if !is_dir {
        items.push(("Open to the Side", RowAction::OpenSide));
    }
    items.extend([
        ("New File", RowAction::NewFile),
        ("New Folder", RowAction::NewFolder),
        ("Rename", RowAction::Rename),
        ("Move to Trash", RowAction::Trash),
        ("Copy Path", RowAction::CopyPath),
        ("Copy Relative Path", RowAction::CopyRelative),
        ("Show in Files", RowAction::Reveal),
        ("Open in Terminal", RowAction::Terminal),
    ]);
    for (n, (label, act)) in items.into_iter().enumerate() {
        let name = format!("a{n}");
        let a = gio::SimpleAction::new(&name, None);
        let s = Rc::downgrade(shell);
        a.connect_activate(move |_, _| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Explorer(ExplorerMsg::RowMenu(i, act)));
            }
        });
        actions.add_action(&a);
        menu.append(Some(tr(label)), Some(&format!("row.{name}")));
    }
    w.insert_action_group("row", Some(&actions));
    let pop = gtk::PopoverMenu::from_model(Some(&menu));
    pop.set_parent(w);
    pop.set_has_arrow(false);
    pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop.popup();
}

/// The row the keyboard is on in `view`'s list, when that's where the keyboard is.
fn list_cursor(app: &App, view: SidebarView) -> Option<usize> {
    (app.focus == Focus::List && app.sidebar == Some(view)).then_some(app.list_cursor)
}

/// Keeps a list's keyboard row in view and focused, which a rebuilt list would drop.
fn follow(list: &RowList, view: SidebarView, cursor: Option<usize>, rebuilt: bool, last: &mut Last) {
    let now = cursor.map(|c| (view, c));
    if now != last.list_cursor || (rebuilt && now.is_some()) {
        if let Some(i) = cursor {
            let v = list.view.clone();
            glib::idle_add_local_once(move || v.scroll_to(i as u32, gtk::ListScrollFlags::FOCUS, None));
        }
        last.list_cursor = now;
    }
}

/// Down in the field above a list goes into the list.
fn down_to_list(shell: &Rc<Shell>, field: &gtk::Entry, view: SidebarView) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let s = Rc::downgrade(shell);
    keys.connect_key_pressed(move |_, key, _, mods| {
        if key == gtk::gdk::Key::Down
            && !mods.intersects(gtk::gdk::ModifierType::CONTROL_MASK | gtk::gdk::ModifierType::ALT_MASK)
            && let Some(s) = s.upgrade()
        {
            s.dispatch(Message::ListFocus(view));
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    field.add_controller(keys);
}
