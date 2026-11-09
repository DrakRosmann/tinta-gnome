//! The strip of icons at the window's left edge: one per sidebar view, and settings at the bottom.
//! It stays when the sidebar is closed, so a click on a view's icon opens it, and on the open
//! view's icon closes it.

use super::Shell;
use crate::app::{App, Message, SidebarView};
use crate::i18n::tr;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use tinta_core::keymap::Command;

pub const CSS: &str = r#"
.tinta-activity { padding: 4px 0; border-right: 1px solid alpha(currentColor, 0.10); }
.tinta-activity button { min-width: 36px; min-height: 36px; margin: 2px 6px; padding: 0; }
.tinta-activity button:checked { background: alpha(currentColor, 0.12); }
.tinta-activity button:checked image { color: @accent_color; }
.tinta-badge { background: @accent_bg_color; color: @accent_fg_color; border-radius: 999px; font-size: 0.7em; font-weight: bold; min-width: 15px; min-height: 15px; padding: 0 3px; margin: 3px 4px; }
"#;

pub struct ActivityBar {
    pub root: gtk::Box,
    buttons: Vec<(SidebarView, gtk::ToggleButton)>,
    git_badge: gtk::Label,
    last: RefCell<Option<(Option<SidebarView>, usize)>>,
}

pub const VIEWS: [(SidebarView, &str, &str); 5] = [
    (SidebarView::Explorer, "folder-symbolic", "Explorer"),
    (SidebarView::Search, "system-search-symbolic", "Search"),
    (SidebarView::Git, "tinta-git-symbolic", "Source Control"),
    (SidebarView::Outline, "view-list-bullet-symbolic", "Outline"),
    (SidebarView::Debug, "tinta-debug-symbolic", "Run and Debug"),
];

impl ActivityBar {
    pub fn build(shell: &Rc<Shell>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("tinta-activity");
        let git_badge = badge();
        let mut buttons = Vec::new();
        for (view, icon, tip) in VIEWS {
            let b = gtk::ToggleButton::new();
            b.set_icon_name(icon);
            b.add_css_class("flat");
            b.set_tooltip_text(Some(tr(tip)));
            b.update_property(&[gtk::accessible::Property::Label(tr(tip))]);
            let s = Rc::downgrade(shell);
            b.connect_clicked(move |b| {
                if let Some(s) = s.upgrade()
                    && !b.has_css_class("syncing")
                {
                    s.dispatch(Message::SidebarShow(view));
                }
            });
            if view == SidebarView::Git {
                root.append(&with_badge(&b, &git_badge));
            } else {
                root.append(&b);
            }
            buttons.push((view, b));
        }
        let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        spacer.set_vexpand(true);
        root.append(&spacer);
        let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings.add_css_class("flat");
        settings.set_tooltip_text(Some(tr("Settings")));
        settings.update_property(&[gtk::accessible::Property::Label(tr("Settings"))]);
        let s = Rc::downgrade(shell);
        settings.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Cmd(Command::OpenSettings));
            }
        });
        root.append(&settings);
        ActivityBar { root, buttons, git_badge, last: RefCell::new(None) }
    }

    pub fn sync(&self, app: &App) {
        let shown = if app.zen { None } else { app.sidebar };
        let changes = app.git.entries.len();
        let now = (shown, changes);
        if *self.last.borrow() == Some(now) {
            return;
        }
        for (view, b) in &self.buttons {
            let on = shown == Some(*view);
            if b.is_active() != on {
                b.add_css_class("syncing");
                b.set_active(on);
                b.remove_css_class("syncing");
            }
        }
        self.git_badge.set_visible(changes > 0);
        self.git_badge.set_label(&if changes > 99 { "99+".to_string() } else { changes.to_string() });
        *self.last.borrow_mut() = Some(now);
    }
}

fn badge() -> gtk::Label {
    let l = gtk::Label::new(None);
    l.add_css_class("tinta-badge");
    l.add_css_class("numeric");
    l.set_halign(gtk::Align::End);
    l.set_valign(gtk::Align::End);
    l.set_can_target(false);
    l.set_visible(false);
    l
}

fn with_badge(b: &impl IsA<gtk::Widget>, badge: &gtk::Label) -> gtk::Overlay {
    let o = gtk::Overlay::new();
    o.set_child(Some(b));
    o.add_overlay(badge);
    o
}
