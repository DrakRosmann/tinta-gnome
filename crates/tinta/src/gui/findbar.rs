//! Find and replace in the file, above the text of each split; and Vim's command line.

use super::Shell;
use super::rows;
use super::window::Window;
use crate::app::{App, FIND_INPUT, FindMsg, MAX_SHOWN_MATCHES, Message, REPLACE_INPUT, VIM_LINE, VimMsg};
use crate::i18n::tr;
use adw::prelude::*;
use std::rc::Rc;

pub struct FindBar {
    pub root: gtk::Box,
    query: gtk::Entry,
    replace: gtk::Entry,
    replace_row: gtk::Box,
    toggles: [gtk::ToggleButton; 5],
    count: gtk::Label,
    error: gtk::Label,
}

fn send(shell: &Rc<Shell>, m: FindMsg) -> impl Fn() + 'static {
    let s = Rc::downgrade(shell);
    move || {
        if let Some(s) = s.upgrade() {
            s.dispatch(Message::Find(m.clone()));
        }
    }
}

fn icon(shell: &Rc<Shell>, name: &str, tip: &str, m: FindMsg) -> gtk::Button {
    let b = gtk::Button::from_icon_name(name);
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tip));
    let f = send(shell, m);
    b.connect_clicked(move |_| f());
    b
}

impl FindBar {
    pub fn build(shell: &Rc<Shell>, _win: &Window, _group: usize) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        root.add_css_class("tinta-findbar");
        root.set_visible(false);
        let line1 = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        let mk = |label: &str, tip: &str, m: FindMsg| {
            let b = gtk::ToggleButton::with_label(label);
            b.add_css_class("flat");
            b.set_tooltip_text(Some(tip));
            let f = send(shell, m);
            b.connect_clicked(move |b| {
                if !b.has_css_class("syncing") {
                    f();
                }
            });
            b
        };
        let expand = mk("⇄", tr("Toggle replace"), FindMsg::ToggleReplace);
        line1.append(&expand);
        let query = rows::entry(shell, tr("Find"), |t| Message::Find(FindMsg::Query(t)));
        query.set_width_chars(28);
        let f = send(shell, FindMsg::Next);
        query.connect_activate(move |_| f());
        line1.append(&query);
        let case = mk("Aa", tr("Match case"), FindMsg::ToggleCase);
        let word = mk("ab", tr("Whole word"), FindMsg::ToggleWord);
        let regex = mk(".*", tr("Regular expression"), FindMsg::ToggleRegex);
        let sel = mk("≡", tr("Find in selection"), FindMsg::ToggleInSelection);
        for b in [&case, &word, &regex, &sel] {
            line1.append(b);
        }
        let count = rows::dim("");
        count.set_margin_start(6);
        count.set_margin_end(6);
        line1.append(&count);
        line1.append(&icon(shell, "go-up-symbolic", tr("Previous match"), FindMsg::Prev));
        line1.append(&icon(shell, "go-down-symbolic", tr("Next match"), FindMsg::Next));
        line1.append(&icon(shell, "window-close-symbolic", tr("Close"), FindMsg::Close));
        root.append(&line1);
        let error = gtk::Label::new(None);
        error.add_css_class("error");
        error.add_css_class("caption");
        error.set_xalign(0.0);
        root.append(&error);
        let replace_row = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        replace_row.append(&rows::spacer(34));
        let replace = rows::entry(shell, tr("Replace"), |t| Message::Find(FindMsg::Replacement(t)));
        replace.set_width_chars(28);
        let f = send(shell, FindMsg::ReplaceOne);
        replace.connect_activate(move |_| f());
        replace_row.append(&replace);
        for (label, m) in [("Replace", FindMsg::ReplaceOne), ("Replace All", FindMsg::ReplaceAll)] {
            let b = gtk::Button::with_label(tr(label));
            b.add_css_class("flat");
            let f = send(shell, m);
            b.connect_clicked(move |_| f());
            replace_row.append(&b);
        }
        root.append(&replace_row);
        FindBar { root, query, replace, replace_row, toggles: [expand, case, word, regex, sel], count, error }
    }

    pub fn sync(&self, shell: &Rc<Shell>, app: &App, active: bool) {
        let f = &app.find;
        let open = active && f.open && app.active().is_some_and(|(t, _)| t.kind == crate::state::TabKind::Text);
        self.root.set_visible(open);
        if !open {
            return;
        }
        // The fields the app focuses by name are the active split's.
        if let Some(w) = shell.window.get() {
            w.register_field(FIND_INPUT, &self.query);
            w.register_field(REPLACE_INPUT, &self.replace);
        }
        rows::set_text(&self.query, &f.query.pattern);
        rows::set_text(&self.replace, &f.replacement);
        for (b, on) in self.toggles.iter().zip([f.replace_open, f.query.case_sensitive, f.query.whole_word, f.query.regex, f.in_selection.is_some()]) {
            if b.is_active() != on {
                b.add_css_class("syncing");
                b.set_active(on);
                b.remove_css_class("syncing");
            }
        }
        self.replace_row.set_visible(f.replace_open);
        let total = app.active().map_or(0, |(_, e)| e.matches.len());
        let count = if f.query.pattern.is_empty() {
            String::new()
        } else if total == 0 {
            tr("No results").to_string()
        } else {
            let plus = if total >= MAX_SHOWN_MATCHES { "+" } else { "" };
            format!("{} {} {total}{plus}", f.current.map_or(0, |c| c + 1), tr("of"))
        };
        self.count.set_label(&count);
        self.error.set_label(f.error.as_deref().unwrap_or(""));
        self.error.set_visible(f.error.is_some());
    }
}

/// Vim's `:`, `/` and `?` line, at the bottom of the window.
pub struct VimLine {
    pub root: gtk::Box,
    prefix: gtk::Label,
    entry: gtk::Entry,
}

impl VimLine {
    pub fn build(shell: &Rc<Shell>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        root.set_margin_start(10);
        root.set_margin_end(10);
        root.set_margin_top(2);
        root.set_margin_bottom(2);
        root.set_visible(false);
        let prefix = gtk::Label::new(Some(":"));
        prefix.add_css_class("monospace");
        root.append(&prefix);
        let entry = rows::entry(shell, "", |t| Message::Vim(VimMsg::Input(t)));
        entry.add_css_class("monospace");
        entry.add_css_class("flat");
        let s = Rc::downgrade(shell);
        entry.connect_activate(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Vim(VimMsg::Submit));
            }
        });
        root.append(&entry);
        VimLine { root, prefix, entry }
    }

    pub fn field(&self) -> (&'static str, gtk::Widget) {
        (VIM_LINE, self.entry.clone().upcast())
    }

    pub fn sync(&self, app: &App) {
        match &app.vim_line {
            Some((p, text)) => {
                self.root.set_visible(true);
                self.prefix.set_label(&p.to_string());
                rows::set_text(&self.entry, text);
            }
            None => self.root.set_visible(false),
        }
    }
}
