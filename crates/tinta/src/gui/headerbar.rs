//! The header bar's buttons, which the user picks: any command of the table, on either side, in
//! any order. The main menu stays first on the left, since that's where the editor for the rest
//! lives.

use super::Shell;
use crate::app::{App, DialogMsg, Message, SettingMsg};
use crate::i18n::{is_portuguese, tr};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use tinta_core::keymap::Command;
use tinta_core::settings::{default_header_end, default_header_start};

/// A symbolic icon for the commands that have an obvious one; the rest show their name.
pub fn icon_for(c: Command) -> Option<&'static str> {
    use Command as C;
    Some(match c {
        C::NewFile => "document-new-symbolic",
        C::OpenFile => "document-open-symbolic",
        C::OpenFolder => "folder-open-symbolic",
        C::Save => "document-save-symbolic",
        C::SaveAs => "document-save-as-symbolic",
        C::CloseTab => "window-close-symbolic",
        C::Undo => "edit-undo-symbolic",
        C::Redo => "edit-redo-symbolic",
        C::Cut => "edit-cut-symbolic",
        C::Copy => "edit-copy-symbolic",
        C::Paste => "edit-paste-symbolic",
        C::Find => "edit-find-symbolic",
        C::Replace => "edit-find-replace-symbolic",
        C::FindInFiles | C::ShowSearch => "system-search-symbolic",
        C::CommandPalette => "view-grid-symbolic",
        C::QuickOpen => "document-open-recent-symbolic",
        C::ToggleSidebar => "sidebar-show-symbolic",
        C::ShowExplorer => "folder-symbolic",
        C::ShowGit => "vcs-branch-symbolic",
        C::ShowOutline => "view-list-symbolic",
        C::ShowDebug => "bug-symbolic",
        C::TogglePanel => "view-dual-symbolic",
        C::ShowProblems => "dialog-warning-symbolic",
        C::SplitRight => "view-dual-symbolic",
        C::ToggleWordWrap => "format-justify-left-symbolic",
        C::MarkdownPreview => "view-reveal-symbolic",
        C::ZoomIn => "zoom-in-symbolic",
        C::ZoomOut => "zoom-out-symbolic",
        C::ZoomReset => "zoom-original-symbolic",
        C::ToggleFullscreen => "view-fullscreen-symbolic",
        C::ToggleZen => "weather-clear-night-symbolic",
        C::GoBack => "go-previous-symbolic",
        C::GoForward => "go-next-symbolic",
        C::FormatDocument => "format-indent-more-symbolic",
        C::GitCommit => "object-select-symbolic",
        C::GitPush => "go-up-symbolic",
        C::GitPull => "go-down-symbolic",
        C::ToggleTerminal | C::NewTerminal => "utilities-terminal-symbolic",
        C::RunBuild => "system-run-symbolic",
        C::RunFile => "media-playback-start-symbolic",
        C::RunStop | C::StopTask => "media-playback-stop-symbolic",
        C::DebugStart => "media-seek-forward-symbolic",
        C::DebugStop => "media-playback-stop-symbolic",
        C::DebugPause => "media-playback-pause-symbolic",
        C::DebugRestart => "view-refresh-symbolic",
        C::StepOver => "go-jump-symbolic",
        C::StepInto => "go-bottom-symbolic",
        C::StepOut => "go-top-symbolic",
        C::OpenSettings => "emblem-system-symbolic",
        C::EditHeaderBar => "document-edit-symbolic",
        C::ShowShortcuts => "preferences-desktop-keyboard-shortcuts-symbolic",
        C::About => "help-about-symbolic",
        _ => return None,
    })
}

fn title_of(c: Command) -> &'static str {
    c.title(is_portuguese())
}

/// A header button that runs `cmd`. Its tooltip names the shortcut too, read when shown, so a
/// remapped key shows up.
pub fn command_button(cmd: Command, shell: &Rc<Shell>) -> gtk::Button {
    let b = match icon_for(cmd) {
        Some(icon) => gtk::Button::from_icon_name(icon),
        None => gtk::Button::with_label(title_of(cmd).trim_end_matches('…')),
    };
    let title = title_of(cmd);
    b.set_has_tooltip(true);
    let s = Rc::downgrade(shell);
    b.connect_query_tooltip(move |_, _, _, _, tip| {
        let keys = s.upgrade().and_then(|s| s.app.try_borrow().ok().and_then(|a| a.keymap.label(cmd).map(str::to_string)));
        match keys.filter(|k| !k.is_empty()) {
            Some(k) => tip.set_text(Some(&format!("{title} ({k})"))),
            None => tip.set_text(Some(title)),
        }
        true
    });
    let s = Rc::downgrade(shell);
    b.connect_clicked(move |_| {
        if let Some(s) = s.upgrade() {
            s.dispatch(Message::Cmd(cmd));
        }
    });
    b
}

fn commands(ids: &[String]) -> Vec<Command> {
    ids.iter().filter_map(|id| Command::from_id(id)).collect()
}

/// The user's buttons on the header bar, rebuilt when the settings change.
pub struct HeaderButtons {
    header: adw::HeaderBar,
    shown: RefCell<Option<(Vec<String>, Vec<String>)>>,
    widgets: RefCell<Vec<gtk::Widget>>,
}

impl HeaderButtons {
    pub fn new(header: &adw::HeaderBar) -> Self {
        Self { header: header.clone(), shown: RefCell::new(None), widgets: RefCell::new(Vec::new()) }
    }

    pub fn sync(&self, shell: &Rc<Shell>, app: &App) {
        let wanted = (app.settings.header_start.clone(), app.settings.header_end.clone());
        if self.shown.borrow().as_ref() == Some(&wanted) {
            return;
        }
        for w in self.widgets.borrow_mut().drain(..) {
            self.header.remove(&w);
        }
        let mut widgets = Vec::new();
        for c in commands(&wanted.0) {
            let b = command_button(c, shell);
            self.header.pack_start(&b);
            widgets.push(b.upcast());
        }
        // pack_end goes from the window's controls inwards.
        for c in commands(&wanted.1).into_iter().rev() {
            let b = command_button(c, shell);
            self.header.pack_end(&b);
            widgets.push(b.upcast());
        }
        *self.widgets.borrow_mut() = widgets;
        *self.shown.borrow_mut() = Some(wanted);
    }
}

/// A right click on the header bar offers its editor.
pub fn connect_context_menu(header: &adw::HeaderBar) {
    let model = gtk::gio::Menu::new();
    model.append_item(&super::menu::item(Command::EditHeaderBar, None));
    let pop = gtk::PopoverMenu::from_model(Some(&model));
    pop.set_parent(header);
    pop.set_has_arrow(false);
    let click = gtk::GestureClick::builder().button(3).build();
    click.connect_pressed(move |g, _, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.popup();
    });
    header.add_controller(click);
}

/// The dialog that edits the header bar: two lists, left and right, whose rows move up, down and
/// across, or go; and a searchable list of every command to add.
pub fn editor_dialog(shell: &Rc<Shell>, app: &App) -> adw::Dialog {
    let d = adw::Dialog::builder().title(tr("Customize Header Bar")).content_width(460).content_height(620).build();
    let page = adw::PreferencesPage::new();
    let reset = gtk::Button::builder().label(tr("Restore Defaults")).halign(gtk::Align::Center).margin_bottom(18).build();
    reset.add_css_class("pill");
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    view.add_bottom_bar(&reset);
    d.set_child(Some(&view));

    let ed = Rc::new(Editor {
        shell: Rc::downgrade(shell),
        page,
        groups: RefCell::new(Vec::new()),
        sides: RefCell::new([app.settings.header_start.clone(), app.settings.header_end.clone()]),
    });
    ed.rebuild();
    let e = ed.clone();
    reset.connect_clicked(move |_| {
        *e.sides.borrow_mut() = [default_header_start(), default_header_end()];
        e.changed();
    });
    let s = Rc::downgrade(shell);
    d.connect_closed(move |_| {
        if let Some(s) = s.upgrade() {
            s.dispatch(Message::Dialog(DialogMsg::Cancel));
        }
    });
    d
}

struct Editor {
    shell: Weak<Shell>,
    page: adw::PreferencesPage,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    /// Left and right, as command ids, left to right.
    sides: RefCell<[Vec<String>; 2]>,
}

impl Editor {
    fn changed(self: &Rc<Self>) {
        let [start, end] = self.sides.borrow().clone();
        if let Some(s) = self.shell.upgrade() {
            s.dispatch(Message::Setting(SettingMsg::HeaderBar(start, end)));
        }
        self.rebuild();
    }

    fn edit(self: &Rc<Self>, f: impl FnOnce(&mut [Vec<String>; 2])) {
        f(&mut self.sides.borrow_mut());
        self.changed();
    }

    fn rebuild(self: &Rc<Self>) {
        for g in self.groups.borrow_mut().drain(..) {
            self.page.remove(&g);
        }
        let sides = self.sides.borrow().clone();
        let titles = [(tr("Left"), tr("Beside the main menu")), (tr("Right"), tr("Beside the window controls"))];
        let mut groups = Vec::new();
        for (side, (title, desc)) in titles.into_iter().enumerate() {
            let g = adw::PreferencesGroup::builder().title(title).description(desc).build();
            g.set_header_suffix(Some(&self.add_button(side, &sides)));
            let list = &sides[side];
            if list.is_empty() {
                let r = adw::ActionRow::builder().title(tr("No buttons")).build();
                r.add_css_class("dim-label");
                g.add(&r);
            }
            for (i, id) in list.iter().enumerate() {
                let Some(c) = Command::from_id(id) else { continue };
                g.add(&self.row(c, side, i, list.len()));
            }
            self.page.add(&g);
            groups.push(g);
        }
        *self.groups.borrow_mut() = groups;
    }

    fn row(self: &Rc<Self>, c: Command, side: usize, i: usize, len: usize) -> adw::ActionRow {
        let r = adw::ActionRow::builder().title(title_of(c)).build();
        if let Some(keys) = self.shell.upgrade().and_then(|s| s.app.try_borrow().ok().and_then(|a| a.keymap.label(c).map(str::to_string))) {
            r.set_subtitle(&keys);
        }
        let icon = gtk::Image::from_icon_name(icon_for(c).unwrap_or("text-x-generic-symbolic"));
        r.add_prefix(&icon);
        let tool = |icon: &str, tip: &str, sensitive: bool| {
            let b = gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).sensitive(sensitive).build();
            b.add_css_class("flat");
            r.add_suffix(&b);
            b
        };
        let up = tool("go-up-symbolic", tr("Move Left"), i > 0);
        let down = tool("go-down-symbolic", tr("Move Right"), i + 1 < len);
        let across = tool(
            if side == 0 { "go-next-symbolic" } else { "go-previous-symbolic" },
            if side == 0 { tr("Move to the Right Side") } else { tr("Move to the Left Side") },
            true,
        );
        let remove = tool("user-trash-symbolic", tr("Remove"), true);
        let e = self.clone();
        up.connect_clicked(move |_| e.edit(|s| s[side].swap(i, i - 1)));
        let e = self.clone();
        down.connect_clicked(move |_| e.edit(|s| s[side].swap(i, i + 1)));
        let e = self.clone();
        across.connect_clicked(move |_| {
            e.edit(|s| {
                let id = s[side].remove(i);
                // Toward the middle of the bar: the end of the left side, the start of the right.
                if side == 0 { s[1].insert(0, id) } else { s[0].push(id) }
            })
        });
        let e = self.clone();
        remove.connect_clicked(move |_| {
            e.edit(|s| {
                s[side].remove(i);
            })
        });
        r
    }

    /// "+" with a searchable list of the commands not on the bar yet.
    fn add_button(self: &Rc<Self>, side: usize, sides: &[Vec<String>; 2]) -> gtk::MenuButton {
        let search = gtk::SearchEntry::builder().placeholder_text(tr("Search commands")).build();
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let mut entries: Vec<(Command, String)> = Command::ALL
            .iter()
            .copied()
            .filter(|c| !sides.iter().flatten().any(|id| id == c.id()))
            .map(|c| (c, title_of(c).to_string()))
            .collect();
        entries.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
        let ids: Rc<Vec<(Command, String)>> = Rc::new(entries);
        for (c, title) in ids.iter() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.set_margin_top(4);
            row.set_margin_bottom(4);
            row.append(&gtk::Image::from_icon_name(icon_for(*c).unwrap_or("text-x-generic-symbolic")));
            let l = gtk::Label::builder().label(title.as_str()).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
            row.append(&l);
            list.append(&row);
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_height(320)
            .max_content_height(320)
            .min_content_width(300)
            .child(&list)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        body.append(&search);
        body.append(&scroll);
        let pop = gtk::Popover::builder().child(&body).build();
        let btn = gtk::MenuButton::builder().icon_name("list-add-symbolic").tooltip_text(tr("Add Button")).popover(&pop).valign(gtk::Align::Center).build();
        btn.add_css_class("flat");

        let (filter_ids, s) = (ids.clone(), search.clone());
        list.set_filter_func(move |row| {
            let q = s.text().to_lowercase();
            let Some((c, title)) = filter_ids.get(row.index() as usize) else { return false };
            q.is_empty() || title.to_lowercase().contains(&q) || c.id().to_lowercase().contains(&q)
        });
        let l = list.clone();
        search.connect_search_changed(move |_| l.invalidate_filter());
        let (l, sr) = (list.clone(), search.clone());
        pop.connect_show(move |_| {
            sr.set_text("");
            sr.grab_focus();
            l.invalidate_filter();
        });
        let l = list.clone();
        search.connect_activate(move |_| {
            let mut i = 0;
            while let Some(r) = l.row_at_index(i) {
                if r.is_child_visible() {
                    r.activate();
                    return;
                }
                i += 1;
            }
        });
        let (e, p) = (self.clone(), pop.clone());
        list.connect_row_activated(move |_, row| {
            let Some((c, _)) = ids.get(row.index() as usize) else { return };
            let id = c.id().to_string();
            p.popdown();
            e.edit(|s| s[side].push(id));
        });
        btn
    }
}
