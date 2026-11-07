//! Long lists drawn on demand: a `GtkListView` over row numbers, each row built from the app's
//! state when it scrolls into view. The model is rebuilt only when the rows really change.

use adw::prelude::*;
use gtk::glib;
use std::cell::Cell;
use std::rc::Rc;

pub type Bind = Rc<dyn Fn(&gtk::Box, usize)>;

pub struct RowList {
    pub view: gtk::ListView,
    pub scroll: gtk::ScrolledWindow,
    model: gtk::StringList,
    key: Cell<u64>,
}

impl RowList {
    /// `bind` fills an empty row box for a row number; `activate` runs on a click; `secondary`
    /// on a right click, with where it happened.
    pub fn new(bind: Bind, activate: impl Fn(usize) + 'static, secondary: Option<Rc<dyn Fn(usize, &gtk::Widget, f64, f64)>>) -> Self {
        let model = gtk::StringList::new(&[]);
        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            b.set_margin_start(4);
            b.set_margin_end(6);
            if let Some(sec) = &secondary {
                let click = gtk::GestureClick::new();
                click.set_button(3);
                let sec = sec.clone();
                let weak = item.downgrade();
                let row = b.clone();
                click.connect_pressed(move |_, _, x, y| {
                    if let Some(item) = weak.upgrade() {
                        sec(item.position() as usize, row.upcast_ref(), x, y);
                    }
                });
                b.add_controller(click);
            }
            item.set_child(Some(&b));
        });
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("a list item");
            let Some(b) = item.child().and_downcast::<gtk::Box>() else { return };
            while let Some(c) = b.first_child() {
                b.remove(&c);
            }
            for cls in b.css_classes() {
                b.remove_css_class(&cls);
            }
            bind(&b, item.position() as usize);
        });
        let selection = gtk::NoSelection::new(Some(model.clone()));
        let view = gtk::ListView::new(Some(selection), Some(factory));
        view.set_single_click_activate(true);
        view.add_css_class("navigation-sidebar");
        view.connect_activate(move |_, pos| activate(pos as usize));
        let scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        RowList { view, scroll, model, key: Cell::new(u64::MAX) }
    }

    /// `n` rows; `key` says whether anything in them changed since last time.
    pub fn set(&self, n: usize, key: u64) {
        if self.key.get() == key && self.model.n_items() as usize == n {
            return;
        }
        self.key.set(key);
        let old = self.model.n_items();
        let names: Vec<String> = (0..n).map(|i| i.to_string()).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        self.model.splice(0, old, &refs);
    }
}

pub fn hash_of(h: impl std::hash::Hash) -> u64 {
    use std::hash::Hasher;
    let mut s = std::collections::hash_map::DefaultHasher::new();
    h.hash(&mut s);
    s.finish()
}

pub fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l
}

pub fn dim(text: &str) -> gtk::Label {
    let l = label(text);
    l.add_css_class("dim-label");
    l.add_css_class("caption");
    l
}

/// An icon for a file, as the desktop's file manager would show it.
pub fn file_icon(name: &str, is_dir: bool, open: bool) -> gtk::Image {
    if is_dir {
        return gtk::Image::from_icon_name(if open { "folder-open-symbolic" } else { "folder-symbolic" });
    }
    let (ty, _) = gtk::gio::content_type_guess(Some(name), None);
    gtk::Image::from_gicon(&gtk::gio::content_type_get_symbolic_icon(&ty))
}

pub fn spacer(w: i32) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.set_size_request(w, -1);
    b
}

/// An entry that tells the app when it has the keyboard, and what's typed in it.
pub fn entry(shell: &Rc<super::Shell>, placeholder: &str, on_change: impl Fn(String) -> crate::app::Message + 'static) -> gtk::Entry {
    let e = gtk::Entry::builder().placeholder_text(placeholder).hexpand(true).build();
    let s = Rc::downgrade(shell);
    e.connect_changed(move |e| {
        if let Some(s) = s.upgrade()
            && !e.has_css_class("syncing")
        {
            s.dispatch(on_change(e.text().to_string()));
        }
    });
    track_focus(shell, &e);
    e
}

pub fn track_focus(shell: &Rc<super::Shell>, w: &impl IsA<gtk::Widget>) {
    let focus = gtk::EventControllerFocus::new();
    let s = Rc::downgrade(shell);
    focus.connect_enter(move |_| {
        if let Some(s) = s.upgrade() {
            s.dispatch(crate::app::Message::InputFocus(true));
        }
    });
    let s = Rc::downgrade(shell);
    focus.connect_leave(move |_| {
        if let Some(s) = s.upgrade() {
            s.dispatch(crate::app::Message::InputFocus(false));
        }
    });
    w.add_controller(focus);
}

/// Changes an entry's text without it reporting the change as typing.
pub fn set_text(e: &(impl IsA<gtk::Editable> + IsA<gtk::Widget>), text: &str) {
    if e.text() == text {
        return;
    }
    let w = e.upcast_ref::<gtk::Widget>();
    w.add_css_class("syncing");
    let pos = e.position();
    e.set_text(text);
    e.set_position(pos.min(text.chars().count() as i32));
    w.remove_css_class("syncing");
}

pub fn idle(f: impl FnOnce() + 'static) {
    glib::idle_add_local_once(f);
}
