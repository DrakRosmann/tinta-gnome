//! The header bar's buttons, which the user picks: any command of the table, on either side, in
//! any order. The main menu stays first on the left, since that's where the editor for the rest
//! lives.

use super::Shell;
use crate::app::{App, DialogMsg, Message, SettingMsg};
use crate::i18n::{is_portuguese, tr};
use adw::prelude::*;
use gtk::glib;
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
        C::ShowGit => "tinta-git-symbolic",
        C::ShowOutline => "view-list-symbolic",
        C::ShowDebug => "tinta-debug-symbolic",
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
        C::StepOver => "tinta-step-over-symbolic",
        C::StepInto => "tinta-step-in-symbolic",
        C::StepOut => "tinta-step-out-symbolic",
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

/// The two sides of the bar, left and right, each as command ids from left to right.
pub type Sides = [Vec<String>; 2];

/// What the editor's buttons and drags do to the two lists. Kept apart from the widgets so the
/// tests can go through every one of them.
pub mod edit {
    use super::Sides;

    /// One place to the left (`-1`) or to the right (`1`) on the same side.
    pub fn shift(s: &mut Sides, side: usize, i: usize, by: isize) {
        let j = i as isize + by;
        if i < s[side].len() && j >= 0 && (j as usize) < s[side].len() {
            s[side].swap(i, j as usize);
        }
    }

    /// To the other side, toward the middle of the bar: the end of the left, the start of the right.
    pub fn across(s: &mut Sides, side: usize, i: usize) {
        if i >= s[side].len() {
            return;
        }
        let id = s[side].remove(i);
        if side == 0 { s[1].insert(0, id) } else { s[0].push(id) }
    }

    pub fn remove(s: &mut Sides, side: usize, i: usize) {
        if i < s[side].len() {
            s[side].remove(i);
        }
    }

    /// At the end of a side, if it isn't on the bar already.
    pub fn add(s: &mut Sides, side: usize, id: &str) {
        if !s.iter().flatten().any(|x| x == id) {
            s[side].push(id.to_string());
        }
    }

    /// A drag from one row onto another: the dragged button takes the place of the one it's
    /// dropped on (so after it, moving down a side, and before it otherwise). Without a row, it
    /// goes to the end of that side.
    pub fn drop(s: &mut Sides, from: (usize, usize), to: (usize, Option<usize>)) {
        let (fs, fi) = from;
        if fi >= s[fs].len() {
            return;
        }
        let id = s[fs].remove(fi);
        let (ts, ti) = to;
        let at = ti.unwrap_or(usize::MAX).min(s[ts].len());
        s[ts].insert(at, id);
    }
}

struct Editor {
    shell: Weak<Shell>,
    page: adw::PreferencesPage,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    sides: RefCell<Sides>,
}

/// What a drag carries: the side and the place of the row it started from.
fn drag_payload(side: usize, i: usize) -> String {
    format!("tinta-header:{side}:{i}")
}

fn parse_payload(s: &str) -> Option<(usize, usize)> {
    let rest = s.strip_prefix("tinta-header:")?;
    let (a, b) = rest.split_once(':')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

impl Editor {
    fn changed(self: &Rc<Self>) {
        let [start, end] = self.sides.borrow().clone();
        if let Some(s) = self.shell.upgrade() {
            s.dispatch(Message::Setting(SettingMsg::HeaderBar(start, end)));
        }
        // Not now: this may run inside a handler of a widget that the rebuild takes away.
        let e = self.clone();
        glib::idle_add_local_once(move || e.rebuild());
    }

    fn edit(self: &Rc<Self>, f: impl FnOnce(&mut Sides)) {
        f(&mut self.sides.borrow_mut());
        self.changed();
    }

    /// Takes drops of rows: onto a row, or onto a side's title, for its end.
    fn drop_target(self: &Rc<Self>, side: usize, at: Option<usize>) -> gtk::DropTarget {
        let t = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let e = self.clone();
        t.connect_drop(move |_, value, _, _| {
            let Some(from) = value.get::<String>().ok().as_deref().and_then(parse_payload) else { return false };
            e.edit(|s| edit::drop(s, from, (side, at)));
            true
        });
        t
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
            g.add_controller(self.drop_target(side, None));
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
        let handle = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        handle.add_css_class("dim-label");
        handle.set_tooltip_text(Some(tr("Drag to Move")));
        // add_prefix puts each one before the last: the handle comes out first.
        r.add_prefix(&gtk::Image::from_icon_name(icon_for(c).unwrap_or("text-x-generic-symbolic")));
        r.add_prefix(&handle);

        // The whole row drags, and takes drops.
        let drag = gtk::DragSource::new();
        drag.set_actions(gtk::gdk::DragAction::MOVE);
        drag.set_content(Some(&gtk::gdk::ContentProvider::for_value(&drag_payload(side, i).to_value())));
        let row = r.clone();
        drag.connect_drag_begin(move |src, _| {
            src.set_icon(Some(&gtk::WidgetPaintable::new(Some(&row))), 20, 20);
        });
        r.add_controller(drag);
        r.add_controller(self.drop_target(side, Some(i)));

        let tool = |icon: &str, tip: &str, sensitive: bool| {
            let b = gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).sensitive(sensitive).build();
            b.add_css_class("flat");
            // A name for screen readers (and the tests), which the icon alone doesn't give.
            b.update_property(&[gtk::accessible::Property::Label(&format!("{tip}: {}", title_of(c)))]);
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
        up.connect_clicked(move |_| e.edit(|s| edit::shift(s, side, i, -1)));
        let e = self.clone();
        down.connect_clicked(move |_| e.edit(|s| edit::shift(s, side, i, 1)));
        let e = self.clone();
        across.connect_clicked(move |_| e.edit(|s| edit::across(s, side, i)));
        let e = self.clone();
        remove.connect_clicked(move |_| e.edit(|s| edit::remove(s, side, i)));
        r
    }

    /// "+" with a searchable list of the commands not on the bar yet.
    fn add_button(self: &Rc<Self>, side: usize, sides: &Sides) -> gtk::MenuButton {
        let search = gtk::SearchEntry::builder().placeholder_text(tr("Search commands")).build();
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let mut entries: Vec<(Command, String)> = Command::ALL
            .iter()
            .copied()
            .filter(|c| !sides.iter().flatten().any(|id| id == c.id()))
            .map(|c| (c, title_of(c).to_string()))
            .collect();
        entries.sort_by_key(|a| a.1.to_lowercase());
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
        let tip = if side == 0 { tr("Add Button on the Left") } else { tr("Add Button on the Right") };
        let btn = gtk::MenuButton::builder().icon_name("list-add-symbolic").tooltip_text(tip).popover(&pop).valign(gtk::Align::Center).build();
        btn.update_property(&[gtk::accessible::Property::Label(tip)]);
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
            e.edit(|s| edit::add(s, side, &id));
        });
        btn
    }
}

#[cfg(test)]
mod tests {
    use super::edit;
    use super::Sides;

    fn sides(l: &[&str], r: &[&str]) -> Sides {
        [l.iter().map(|s| s.to_string()).collect(), r.iter().map(|s| s.to_string()).collect()]
    }

    #[test]
    fn arrows_move_within_a_side_and_stop_at_the_ends() {
        let mut s = sides(&[], &["a", "b", "c"]);
        edit::shift(&mut s, 1, 0, 1);
        assert_eq!(s, sides(&[], &["b", "a", "c"]));
        edit::shift(&mut s, 1, 2, -1);
        assert_eq!(s, sides(&[], &["b", "c", "a"]));
        edit::shift(&mut s, 1, 0, -1);
        edit::shift(&mut s, 1, 2, 1);
        assert_eq!(s, sides(&[], &["b", "c", "a"]), "past either end, nothing moves");
    }

    #[test]
    fn across_goes_toward_the_middle_of_the_bar() {
        let mut s = sides(&["x"], &["a", "b"]);
        edit::across(&mut s, 0, 0);
        assert_eq!(s, sides(&[], &["x", "a", "b"]));
        edit::across(&mut s, 1, 2);
        assert_eq!(s, sides(&["b"], &["x", "a"]));
    }

    #[test]
    fn remove_and_add_without_duplicates() {
        let mut s = sides(&["x"], &["a"]);
        edit::remove(&mut s, 0, 0);
        edit::remove(&mut s, 0, 5);
        assert_eq!(s, sides(&[], &["a"]));
        edit::add(&mut s, 0, "a");
        assert_eq!(s, sides(&[], &["a"]), "already on the bar");
        edit::add(&mut s, 0, "y");
        assert_eq!(s, sides(&["y"], &["a"]));
    }

    #[test]
    fn a_drop_takes_the_place_of_the_row_under_it() {
        // Down a side: after the row dropped on.
        let mut s = sides(&[], &["a", "b", "c", "d"]);
        edit::drop(&mut s, (1, 0), (1, Some(2)));
        assert_eq!(s, sides(&[], &["b", "c", "a", "d"]));
        // Up a side: before it.
        edit::drop(&mut s, (1, 3), (1, Some(0)));
        assert_eq!(s, sides(&[], &["d", "b", "c", "a"]));
        // To the other side, onto a row, and onto the title (the end).
        edit::drop(&mut s, (1, 1), (0, Some(0)));
        assert_eq!(s, sides(&["b"], &["d", "c", "a"]));
        edit::drop(&mut s, (1, 0), (0, None));
        assert_eq!(s, sides(&["b", "d"], &["c", "a"]));
        // Onto itself: stays.
        edit::drop(&mut s, (0, 1), (0, Some(1)));
        assert_eq!(s, sides(&["b", "d"], &["c", "a"]));
    }

    #[test]
    fn drag_payloads_round_trip_and_reject_strangers() {
        assert_eq!(super::parse_payload(&super::drag_payload(1, 7)), Some((1, 7)));
        assert_eq!(super::parse_payload("some text"), None);
    }
}
