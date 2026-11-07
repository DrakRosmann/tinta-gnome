//! The main window: header bar, the editor's tab groups, sidebar, bottom panel and status bar.
//! `sync` brings the widgets in line with the app's state after every batch of messages.

use super::editor::Editor;
use super::panel::Panel;
use super::sidebar::Sidebar;
use super::{Shell, keys};
use crate::app::{App, Focus, Message};
use crate::i18n::tr;
use crate::state::{TabId, TabKind};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use tinta_core::keymap::Command;

const CSS: &str = r#"
.tinta-panel { border-top: 1px solid alpha(currentColor, 0.12); }
.tinta-status { padding: 0 6px; min-height: 26px; border-top: 1px solid alpha(currentColor, 0.10); }
.tinta-status button { padding: 0 8px; min-height: 22px; font-size: 0.9em; }
.tinta-breadcrumbs { padding: 3px 12px; font-size: 0.9em; }
.tinta-sidebar row { min-height: 26px; }
.tinta-popup { background: @popover_bg_color; color: @popover_fg_color; border-radius: 12px; box-shadow: 0 2px 8px alpha(black, 0.25); border: 1px solid alpha(currentColor, 0.10); }
.tinta-findbar { padding: 6px 10px; border-bottom: 1px solid alpha(currentColor, 0.10); }
.panel-text { background: transparent; }
"#;

pub struct GroupView {
    pub root: gtk::Box,
    view: adw::TabView,
    pages: RefCell<Vec<(TabId, adw::TabPage, gtk::Overlay, Editor)>>,
    syncing: Cell<bool>,
    find: super::findbar::FindBar,
    crumbs: gtk::Label,
}

pub struct Window {
    pub window: adw::ApplicationWindow,
    pub overlay: gtk::Overlay,
    vim_line: super::findbar::VimLine,
    toasts: adw::ToastOverlay,
    toolbar: adw::ToolbarView,
    title: adw::WindowTitle,
    main: gtk::Paned,
    right: gtk::Paned,
    groups_box: gtk::Box,
    welcome: adw::StatusPage,
    groups: RefCell<Vec<Rc<GroupView>>>,
    pub panel: Rc<Panel>,
    pub sidebar: Rc<Sidebar>,
    status: super::status::StatusBar,
    popups: super::popups::Popups,
    dialogs: super::dialogs::Dialogs,
    fields: RefCell<HashMap<&'static str, gtk::Widget>>,
    allow_close: Cell<bool>,
    last: RefCell<Last>,
}

#[derive(Default)]
struct Last {
    title: String,
    toast_gen: u64,
    zen: Option<bool>,
    sidebar: Option<Option<crate::app::SidebarView>>,
    panel_open: Option<bool>,
    focus: Option<Focus>,
    active_tab: Option<TabId>,
    ui_theme: Option<tinta_core::settings::AppTheme>,
    sidebar_w: i32,
    panel_h: i32,
}

impl Window {
    pub fn build(gapp: &adw::Application, shell: &Rc<Shell>) -> Self {
        let css = gtk::CssProvider::new();
        css.load_from_string(CSS);
        gtk::style_context_add_provider_for_display(&gdk::Display::default().expect("display"), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);

        let window = adw::ApplicationWindow::builder().application(gapp).default_width(1280).default_height(820).title("Tinta").build();
        window.set_size_request(480, 320);

        let header = adw::HeaderBar::new();
        let title = adw::WindowTitle::new("Tinta", "");
        header.set_title_widget(Some(&title));
        let sidebar_btn = icon_button("sidebar-show-symbolic", Command::ToggleSidebar, shell);
        header.pack_start(&sidebar_btn);
        let menu = gtk::MenuButton::builder().icon_name("open-menu-symbolic").primary(true).tooltip_text(tr("Main Menu")).build();
        menu.set_menu_model(Some(&super::menu::main_menu(&shell.app.borrow())));
        header.pack_end(&menu);
        header.pack_end(&icon_button("emblem-system-symbolic", Command::OpenSettings, shell));
        header.pack_end(&icon_button("utilities-terminal-symbolic", Command::ToggleTerminal, shell));
        header.pack_end(&icon_button("view-dual-symbolic", Command::SplitRight, shell));
        super::menu::install_actions(gapp, &window, shell);

        let groups_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        groups_box.set_hexpand(true);
        groups_box.set_vexpand(true);
        let welcome = adw::StatusPage::builder()
            .icon_name("io.github.DrakRosmann.TintaGnome")
            .title("Tinta")
            .description(tr("Open a file or a folder to start"))
            .vexpand(true)
            .build();
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        buttons.set_halign(gtk::Align::Center);
        for (label, cmd, suggested) in [("Open Folder", Command::OpenFolder, true), ("Open File", Command::OpenFile, false), ("New File", Command::NewFile, false)] {
            let b = gtk::Button::with_label(tr(label));
            b.add_css_class("pill");
            if suggested {
                b.add_css_class("suggested-action");
            }
            let s = Rc::downgrade(shell);
            b.connect_clicked(move |_| {
                if let Some(s) = s.upgrade() {
                    s.dispatch(Message::Cmd(cmd));
                }
            });
            buttons.append(&b);
        }
        welcome.set_child(Some(&buttons));
        groups_box.append(&welcome);

        let panel = Panel::build(shell);
        let right = gtk::Paned::new(gtk::Orientation::Vertical);
        right.set_start_child(Some(&groups_box));
        right.set_end_child(Some(&panel.root));
        right.set_resize_start_child(true);
        right.set_resize_end_child(false);
        right.set_shrink_end_child(false);
        right.set_wide_handle(false);

        let sidebar = Sidebar::build(shell);
        let main = gtk::Paned::new(gtk::Orientation::Horizontal);
        main.set_start_child(Some(&sidebar.root));
        main.set_end_child(Some(&right));
        main.set_resize_start_child(false);
        main.set_shrink_start_child(false);
        main.set_position(280);

        let status = super::status::StatusBar::build(shell);
        let vim_line = super::findbar::VimLine::build(shell);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&main);
        content.append(&vim_line.root);
        main.set_vexpand(true);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        toolbar.add_bottom_bar(&status.root);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&toolbar));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&overlay));
        window.set_content(Some(&toasts));

        let w = Window {
            window,
            overlay,
            vim_line,
            toasts,
            toolbar,
            title,
            main,
            right,
            groups_box,
            welcome,
            groups: RefCell::new(Vec::new()),
            panel,
            sidebar,
            status,
            popups: super::popups::Popups::default(),
            dialogs: super::dialogs::Dialogs::default(),
            fields: RefCell::new(HashMap::new()),
            allow_close: Cell::new(false),
            last: RefCell::new(Last::default()),
        };
        w.connect(shell);
        for (id, f) in w.sidebar.fields().into_iter().chain(w.panel.debug.fields()) {
            w.register_field(id, &f);
        }
        let (id, f) = w.vim_line.field();
        w.register_field(id, &f);
        w
    }

    pub fn dialogs_ref(&self) -> &super::dialogs::Dialogs {
        &self.dialogs
    }

    fn connect(&self, shell: &Rc<Shell>) {
        // Keys go to the app first; what it doesn't want goes on to the focused widget.
        let keyc = gtk::EventControllerKey::new();
        keyc.set_propagation_phase(gtk::PropagationPhase::Capture);
        let s = Rc::downgrade(shell);
        let win = self.window.clone();
        keyc.connect_key_pressed(move |c, keyval, keycode, state| {
            let Some(s) = s.upgrade() else { return glib::Propagation::Proceed };
            let focus = gtk::prelude::GtkWindowExt::focus(&win);
            let in_field = focus.as_ref().is_some_and(|f| f.is::<gtk::Text>() || f.is::<gtk::TextView>());
            // Accents and other scripts: the input method sees text keys first, outside Vim's
            // command modes where every key is a command.
            if let Some(ed) = focus.as_ref().and_then(|f| f.downcast_ref::<Editor>()) {
                let vim_cmd = { let a = s.app.borrow(); a.settings.vim_mode && !a.vim.is_insert() };
                let plain = !state.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK | gdk::ModifierType::SUPER_MASK);
                if !vim_cmd && plain && let Some(ev) = c.current_event() && ed.filter_key(&ev) {
                    return glib::Propagation::Stop;
                }
            }
            if in_field && keys::is_dead(keyval) {
                return glib::Propagation::Proceed;
            }
            let Some(mut ev) = keys::key_event(&WidgetExt::display(&win), keyval, keycode, state) else { return glib::Propagation::Proceed };
            ev.captured = in_field;
            s.app.borrow_mut().key_passthrough = false;
            s.dispatch(Message::Key(ev));
            let pass = std::mem::take(&mut s.app.borrow_mut().key_passthrough);
            if pass { glib::Propagation::Proceed } else { glib::Propagation::Stop }
        });
        let s = Rc::downgrade(shell);
        keyc.connect_modifiers(move |_, state| {
            if let Some(s) = s.upgrade() {
                s.app.borrow_mut().modifiers = crate::gfx::Modifiers::from_gdk(state);
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keyc);

        let s = Rc::downgrade(shell);
        let allow = self.allow_close.clone();
        let _ = allow;
        self.window.connect_close_request(move |w| {
            let Some(s) = s.upgrade() else { return glib::Propagation::Proceed };
            let ok = s.window.get().is_some_and(|x| x.allow_close.get());
            if ok {
                return glib::Propagation::Proceed;
            }
            s.dispatch(Message::CloseRequested);
            let _ = w;
            glib::Propagation::Stop
        });
        let s = Rc::downgrade(shell);
        self.window.connect_is_active_notify(move |w| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::WindowFocused(w.is_active()));
            }
        });
        // Files dropped on the window open.
        let drop = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
        let s = Rc::downgrade(shell);
        drop.connect_drop(move |_, value, _, _| {
            let (Some(s), Ok(list)) = (s.upgrade(), value.get::<gdk::FileList>()) else { return false };
            let paths: Vec<_> = list.files().iter().filter_map(|f| f.path()).collect();
            s.dispatch(Message::Dropped(paths));
            true
        });
        self.window.add_controller(drop);
        let s = Rc::downgrade(shell);
        self.main.connect_position_notify(move |p| {
            if let Some(s) = s.upgrade()
                && let Ok(mut a) = s.app.try_borrow_mut()
                && p.position() > 50
            {
                a.sidebar_w = p.position() as f32;
            }
        });
        let s = Rc::downgrade(shell);
        self.right.connect_position_notify(move |p| {
            if let Some(s) = s.upgrade()
                && let Ok(mut a) = s.app.try_borrow_mut()
                && a.panel.is_some()
            {
                let h = (p.height() - p.position()) as f32;
                if h > 60.0 {
                    a.panel_h = h;
                }
            }
        });
    }

    pub fn register_field(&self, id: &'static str, w: &impl IsA<gtk::Widget>) {
        self.fields.borrow_mut().insert(id, w.clone().upcast());
    }

    pub fn focus_field(&self, id: &'static str, select: bool) {
        let Some(w) = self.fields.borrow().get(id).cloned() else { return };
        glib::idle_add_local_once(move || {
            w.grab_focus();
            if select && let Some(e) = w.downcast_ref::<gtk::Editable>() {
                e.select_region(0, -1);
            }
        });
    }

    pub fn active_editor(&self, app: &App) -> Option<Editor> {
        let g = self.groups.borrow().get(app.active_group).cloned()?;
        let tab = app.groups.get(app.active_group)?.active_tab()?;
        g.pages.borrow().iter().find(|(id, ..)| *id == tab).map(|(.., e)| e.clone())
    }

    pub fn focus_editor(&self) {
        let Some(shell) = self.shell_of() else { return };
        if let Some(e) = self.active_editor(&shell.app.borrow()) {
            glib::idle_add_local_once(move || {
                e.grab_focus();
            });
        }
    }

    fn shell_of(&self) -> Option<Rc<Shell>> {
        self.sidebar.shell()
    }

    pub fn quit(&self) {
        self.allow_close.set(true);
        self.window.close();
        if let Some(app) = self.window.application() {
            app.quit();
        }
    }

    /// A picture of the window as it's drawn, header bar included, for the test scripts.
    pub fn screenshot(&self, path: &Path) {
        let w = self.window.width();
        let h = self.window.height();
        let paintable = gtk::WidgetPaintable::new(Some(&self.window));
        let snap = gtk::Snapshot::new();
        paintable.snapshot(&snap, w as f64, h as f64);
        let Some(node) = snap.to_node() else { return };
        let Some(native) = self.window.native() else { return };
        let Some(renderer) = native.renderer() else { return };
        let texture = renderer.render_texture(&node, Some(&gtk::graphene::Rect::new(0.0, 0.0, w as f32, h as f32)));
        if let Err(e) = texture.save_to_png(path) {
            eprintln!("script: {e}");
        }
    }

    pub fn sync(&self, shell: &Rc<Shell>) {
        let Ok(app) = shell.app.try_borrow() else { return };
        let mut last = self.last.borrow_mut();
        if last.title != app.title {
            let (title, subtitle) = match app.title.rsplit_once(" — Tinta").map(|(t, _)| t) {
                Some(t) => match t.split_once(" — ") {
                    Some((file, folder)) => (file.to_string(), folder.to_string()),
                    None => (t.to_string(), String::new()),
                },
                None => ("Tinta".to_string(), String::new()),
            };
            self.title.set_title(&title);
            self.title.set_subtitle(&subtitle);
            self.window.set_title(Some(&app.title));
            last.title = app.title.clone();
        }
        if last.toast_gen != app.toast_gen {
            last.toast_gen = app.toast_gen;
            if let Some(t) = &app.toast {
                let toast = adw::Toast::new(&glib::markup_escape_text(t));
                toast.set_timeout(4);
                self.toasts.add_toast(toast);
            }
        }
        if last.ui_theme != Some(app.settings.ui_theme) {
            use tinta_core::settings::AppTheme;
            adw::StyleManager::default().set_color_scheme(match app.settings.ui_theme {
                AppTheme::System => adw::ColorScheme::Default,
                AppTheme::Dark => adw::ColorScheme::ForceDark,
                AppTheme::Light => adw::ColorScheme::ForceLight,
            });
            last.ui_theme = Some(app.settings.ui_theme);
        }
        if last.zen != Some(app.zen) {
            self.toolbar.set_reveal_top_bars(!app.zen);
            self.toolbar.set_reveal_bottom_bars(!app.zen);
            last.zen = Some(app.zen);
        }
        let sidebar = if app.zen { None } else { app.sidebar };
        if last.sidebar != Some(sidebar) {
            self.sidebar.root.set_visible(sidebar.is_some());
            last.sidebar = Some(sidebar);
        }
        if sidebar.is_some() && (app.sidebar_w as i32 - last.sidebar_w).abs() > 2 && (app.sidebar_w as i32 - self.main.position()).abs() > 2 {
            self.main.set_position(app.sidebar_w as i32);
            last.sidebar_w = app.sidebar_w as i32;
        }
        let panel_open = app.panel.is_some() && !app.zen;
        if last.panel_open != Some(panel_open) {
            self.panel.root.set_visible(panel_open);
            last.panel_open = Some(panel_open);
            last.panel_h = 0;
        }
        if panel_open && (app.panel_h as i32 - last.panel_h).abs() > 2 && self.right.height() > 0 {
            self.right.set_position((self.right.height() - app.panel_h as i32).max(80));
            last.panel_h = app.panel_h as i32;
        }
        let groups_changed = self.sync_groups(shell, &app);
        let active_tab = app.groups.get(app.active_group).and_then(|g| g.active_tab());
        // The keyboard goes where the app says, but only when that changes: clicks move it too.
        if last.focus != Some(app.focus) || (app.focus == Focus::Editor && (last.active_tab != active_tab || groups_changed)) {
            if app.focus == Focus::Editor && app.palette.is_none() && app.dialog.is_none() {
                if let Some(e) = self.active_editor(&app) {
                    glib::idle_add_local_once(move || {
                        e.grab_focus();
                    });
                }
            }
            last.focus = Some(app.focus);
            last.active_tab = active_tab;
        }
        drop(last);
        for g in self.groups.borrow().iter() {
            if let Some(tab) = g.view.selected_page().and_then(|p| g.pages.borrow().iter().find(|(_, pg, ..)| *pg == p).map(|(.., e)| e.clone())) {
                tab.queue_draw();
            }
        }
        self.sidebar.sync(shell, &app);
        self.panel.sync(shell, &app);
        self.status.sync(&app);
        self.vim_line.sync(&app);
        self.popups.sync(self, shell, &app);
        self.dialogs.sync(self, shell, &app);
        for (i, g) in self.groups.borrow().iter().enumerate() {
            g.find.sync(shell, &app, i == app.active_group);
            g.crumbs.set_visible(!app.zen);
            if i == app.active_group || g.crumbs.label().is_empty() {
                g.crumbs.set_label(&super::sidebar::breadcrumbs(&app, i));
            }
        }
    }

    /// One `AdwTabView` per split, with its pages matching the app's tabs.
    fn sync_groups(&self, shell: &Rc<Shell>, app: &App) -> bool {
        let mut changed = false;
        let mut groups = self.groups.borrow_mut();
        while groups.len() < app.groups.len() {
            let g = GroupView::build(shell, groups.len(), self);
            groups.push(g);
            changed = true;
        }
        while groups.len() > app.groups.len() {
            let g = groups.pop().expect("a group");
            self.groups_box.remove(&g.root);
            changed = true;
        }
        if changed {
            while let Some(c) = self.groups_box.first_child() {
                self.groups_box.remove(&c);
            }
            for (i, g) in groups.iter().enumerate() {
                if i > 0 {
                    self.groups_box.append(&gtk::Separator::new(gtk::Orientation::Vertical));
                }
                self.groups_box.append(&g.root);
            }
        }
        let any_tab = app.groups.iter().any(|g| !g.model.is_empty());
        if !any_tab && groups.len() <= 1 {
            if self.welcome.parent().is_none() {
                for g in groups.iter() {
                    g.root.set_visible(false);
                }
                self.groups_box.append(&self.welcome);
            }
        } else if self.welcome.parent().is_some() {
            self.groups_box.remove(&self.welcome);
            for g in groups.iter() {
                g.root.set_visible(true);
            }
        }
        for (i, g) in groups.iter().enumerate() {
            g.sync(shell, app, i);
        }
        changed
    }
}

fn icon_button(icon: &str, cmd: Command, shell: &Rc<Shell>) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(cmd.title(crate::i18n::is_portuguese())));
    let s = Rc::downgrade(shell);
    b.connect_clicked(move |_| {
        if let Some(s) = s.upgrade() {
            s.dispatch(Message::Cmd(cmd));
        }
    });
    b
}

impl GroupView {
    fn build(shell: &Rc<Shell>, index: usize, win: &Window) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_hexpand(true);
        let view = adw::TabView::new();
        view.set_vexpand(true);
        let bar = adw::TabBar::new();
        bar.set_view(Some(&view));
        bar.set_autohide(false);
        root.append(&bar);
        let crumbs = gtk::Label::new(None);
        crumbs.set_xalign(0.0);
        crumbs.add_css_class("tinta-breadcrumbs");
        crumbs.add_css_class("dim-label");
        crumbs.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        root.append(&crumbs);
        let find = super::findbar::FindBar::build(shell, win, index);
        root.append(&find.root);
        root.append(&view);
        let g = Rc::new(GroupView { root, view, pages: RefCell::new(Vec::new()), syncing: Cell::new(false), find, crumbs });

        let s = Rc::downgrade(shell);
        let me = Rc::downgrade(&g);
        g.view.connect_selected_page_notify(move |v| {
            let (Some(s), Some(me)) = (s.upgrade(), me.upgrade()) else { return };
            if me.syncing.get() {
                return;
            }
            let Some(page) = v.selected_page() else { return };
            let id = me.pages.borrow().iter().find(|(_, p, ..)| *p == page).map(|(id, ..)| *id);
            if let Some(id) = id {
                s.dispatch(Message::TabActivate(index, id));
            }
        });
        let s = Rc::downgrade(shell);
        let me = Rc::downgrade(&g);
        g.view.connect_close_page(move |v, page| {
            let (Some(s), Some(me)) = (s.upgrade(), me.upgrade()) else { return glib::Propagation::Proceed };
            if me.syncing.get() {
                v.close_page_finish(page, true);
                return glib::Propagation::Stop;
            }
            // The app decides: unsaved changes ask first.
            v.close_page_finish(page, false);
            let id = me.pages.borrow().iter().find(|(_, p, ..)| p == page).map(|(id, ..)| *id);
            if let Some(id) = id {
                s.dispatch(Message::TabClose(index, id));
            }
            glib::Propagation::Stop
        });
        let s = Rc::downgrade(shell);
        let me = Rc::downgrade(&g);
        g.view.connect_page_reordered(move |_, page, pos| {
            let (Some(s), Some(me)) = (s.upgrade(), me.upgrade()) else { return };
            if me.syncing.get() {
                return;
            }
            let id = me.pages.borrow().iter().find(|(_, p, ..)| p == page).map(|(id, ..)| *id);
            if let Some(id) = id {
                s.dispatch(Message::TabMoved(index, id, pos as usize));
            }
        });
        // A tab's menu: the usual close actions.
        let menu = gio::Menu::new();
        for c in [Command::CloseOtherTabs, Command::CloseAllTabs, Command::PinTab] {
            menu.append_item(&super::menu::item(c, None));
        }
        g.view.set_menu_model(Some(&menu));
        let s = Rc::downgrade(shell);
        let me = Rc::downgrade(&g);
        g.view.connect_setup_menu(move |_, page| {
            let (Some(s), Some(me), Some(page)) = (s.upgrade(), me.upgrade(), page) else { return };
            let id = me.pages.borrow().iter().find(|(_, p, ..)| p == page).map(|(id, ..)| *id);
            if let Some(id) = id {
                s.dispatch(Message::TabActivate(index, id));
            }
        });
        g
    }

    fn sync(&self, shell: &Rc<Shell>, app: &App, index: usize) {
        let Some(group) = app.groups.get(index) else { return };
        self.syncing.set(true);
        let wanted = group.tabs();
        // Pages of tabs that are gone.
        let stale: Vec<adw::TabPage> = self.pages.borrow().iter().filter(|(id, ..)| !wanted.contains(id)).map(|(_, p, ..)| p.clone()).collect();
        for p in stale {
            self.view.close_page(&p);
            self.pages.borrow_mut().retain(|(_, x, ..)| *x != p);
        }
        for (pos, id) in wanted.iter().enumerate() {
            let existing = self.pages.borrow().iter().find(|(t, ..)| t == id).map(|(_, p, ..)| p.clone());
            let page = match existing {
                Some(p) => p,
                None => {
                    let editor = Editor::new(shell, *id);
                    let overlay = gtk::Overlay::new();
                    let child: gtk::Widget = match app.tabs.get(id).map(|t| t.kind == TabKind::Preview) {
                        Some(true) => super::markdown::view(shell, *id).upcast(),
                        _ => editor.clone().upcast(),
                    };
                    overlay.set_child(Some(&child));
                    let page = self.view.insert(&overlay, pos as i32);
                    self.pages.borrow_mut().push((*id, page.clone(), overlay, editor));
                    page
                }
            };
            if self.view.page_position(&page) != pos as i32 {
                self.view.reorder_page(&page, pos as i32);
            }
            let title = group.model.text(*id).unwrap_or("").to_string();
            let dirty = title.starts_with("● ");
            let clean = title.trim_start_matches("● ").to_string();
            if page.title() != clean {
                page.set_title(&clean);
            }
            if page.needs_attention() != dirty {
                page.set_needs_attention(dirty);
            }
            let tip = app.tabs.get(id).and_then(|t| app.docs.get(&t.doc)).and_then(|e| e.doc.path.as_ref()).map(|p| p.display().to_string()).unwrap_or_default();
            if page.tooltip().map(|t| t.to_string()).unwrap_or_default() != tip {
                page.set_tooltip(&tip);
            }
            let pinned = app.tabs.get(id).is_some_and(|t| t.pinned);
            if page.is_pinned() != pinned {
                self.view.set_page_pinned(&page, pinned);
            }
            if let Some(t) = app.tabs.get(id) && t.kind == TabKind::Preview {
                super::markdown::refresh(&page, shell, *id);
            }
        }
        if let Some(active) = group.active_tab()
            && let Some(page) = self.pages.borrow().iter().find(|(t, ..)| *t == active).map(|(_, p, ..)| p.clone())
            && self.view.selected_page().as_ref() != Some(&page)
        {
            self.view.set_selected_page(&page);
        }
        // Completion, hover and signature help float over the active editor.
        if index == app.active_group
            && let Some(active) = group.active_tab()
            && let Some((_, _, overlay, _)) = self.pages.borrow().iter().find(|(t, ..)| *t == active)
        {
            if let Some(win) = shell.window.get() {
                win.popups.attach(overlay);
            }
        }
        self.syncing.set(false);
    }
}
