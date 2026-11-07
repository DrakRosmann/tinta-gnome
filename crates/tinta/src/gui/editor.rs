//! The text area as a GTK widget: GTK's gestures go into the editor's own mouse handling, and
//! drawing goes through GSK. The same widget, with another painter, shows side-by-side diffs.

use super::Shell;
use crate::app::{App, Focus, Message};
use crate::gfx::{self, Color, Event, Modifiers, Painter, Point, Rectangle, Size, TextCache, mouse};
use crate::state::{TabId, TabKind};
use crate::ui::diff_view::{self, DiffEvent, DiffView};
use crate::ui::editor_view::{self, EditorEvent, EditorView, Palette};
use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Instant;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Editor {
        pub shell: RefCell<Weak<Shell>>,
        pub tab: Cell<TabId>,
        pub state: RefCell<editor_view::State>,
        pub diff_state: RefCell<diff_view::State>,
        pub cache: RefCell<TextCache>,
        pub pointer: Cell<Option<(f32, f32)>>,
        pub timer: RefCell<Option<(Instant, glib::SourceId)>>,
        pub im: RefCell<Option<gtk::IMMulticontext>>,
        /// GTK's own composing, for when the desktop's input method won't take a dead key, as
        /// happens on compositors without one: ´ and a still make á.
        pub compose: RefCell<Option<gtk::IMContextSimple>>,
        pub composing: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Editor {
        const NAME: &'static str = "TintaEditor";
        type Type = super::Editor;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Editor {}

    impl WidgetImpl for Editor {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().paint(snapshot);
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            // The app needs the size to scroll and wrap; it hears about it on the next frame.
            self.obj().later();
        }
    }
}

glib::wrapper! {
    pub struct Editor(ObjectSubclass<imp::Editor>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// The colors of the text area, from libadwaita: the view's background, the accent the user
/// picked, and its status colors, for dark and for light.
pub fn palette(widget: &impl IsA<gtk::Widget>) -> Palette {
    let sm = adw::StyleManager::default();
    let dark = sm.is_dark();
    let fg: Color = widget.as_ref().color().into();
    let bg = if dark { Color::from_rgb8(0x1d, 0x1d, 0x20) } else { Color::from_rgb8(0xff, 0xff, 0xff) };
    let accent: Color = sm.accent_color_rgba().into();
    let (success, warning, error) = if dark {
        (Color::from_rgb8(0x26, 0xa2, 0x69), Color::from_rgb8(0xcd, 0x93, 0x09), Color::from_rgb8(0xe6, 0x2d, 0x42))
    } else {
        (Color::from_rgb8(0x2e, 0xc2, 0x7e), Color::from_rgb8(0xe5, 0xa5, 0x0a), Color::from_rgb8(0xe0, 0x1b, 0x24))
    };
    Palette::new(fg, bg, accent, success, warning, error, dark)
}

fn button_of(n: u32) -> mouse::Button {
    match n {
        1 => mouse::Button::Left,
        2 => mouse::Button::Middle,
        3 => mouse::Button::Right,
        _ => mouse::Button::Other,
    }
}

impl Editor {
    pub fn new(shell: &Rc<Shell>, tab: TabId) -> Self {
        let w: Self = glib::Object::builder().build();
        let imp = w.imp();
        *imp.shell.borrow_mut() = Rc::downgrade(shell);
        imp.tab.set(tab);
        w.set_focusable(true);
        w.set_hexpand(true);
        w.set_vexpand(true);
        w.set_overflow(gtk::Overflow::Hidden);
        w.add_css_class("view");
        w.set_cursor_from_name(Some("text"));

        let click = gtk::GestureClick::new();
        click.set_button(0);
        let me = w.downgrade();
        click.connect_pressed(move |g, _n, x, y| {
            let Some(w) = me.upgrade() else { return };
            if !w.is_diff() {
                w.grab_focus();
            }
            w.imp().pointer.set(Some((x as f32, y as f32)));
            w.set_mods(g.current_event_state());
            if w.deliver(Event::Mouse(mouse::Event::ButtonPressed(button_of(g.current_button())))) {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        let me = w.downgrade();
        click.connect_released(move |g, _n, x, y| {
            let Some(w) = me.upgrade() else { return };
            w.imp().pointer.set(Some((x as f32, y as f32)));
            w.deliver(Event::Mouse(mouse::Event::ButtonReleased(button_of(g.current_button()))));
        });
        w.add_controller(click);

        let motion = gtk::EventControllerMotion::new();
        let me = w.downgrade();
        motion.connect_motion(move |c, x, y| {
            let Some(w) = me.upgrade() else { return };
            w.imp().pointer.set(Some((x as f32, y as f32)));
            w.set_mods(c.current_event_state());
            w.deliver(Event::Mouse(mouse::Event::CursorMoved { position: Point::new(x as f32, y as f32) }));
            w.update_cursor();
        });
        let me = w.downgrade();
        motion.connect_enter(move |_, x, y| {
            if let Some(w) = me.upgrade() {
                w.imp().pointer.set(Some((x as f32, y as f32)));
                w.deliver(Event::Mouse(mouse::Event::CursorEntered));
            }
        });
        let me = w.downgrade();
        motion.connect_leave(move |_| {
            if let Some(w) = me.upgrade() {
                w.deliver(Event::Mouse(mouse::Event::CursorLeft));
            }
        });
        w.add_controller(motion);

        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let me = w.downgrade();
        scroll.connect_scroll(move |c, dx, dy| {
            let Some(w) = me.upgrade() else { return glib::Propagation::Proceed };
            w.set_mods(c.current_event_state());
            let (x, y) = (-dx as f32, -dy as f32);
            let delta = if c.unit() == gdk::ScrollUnit::Wheel { mouse::ScrollDelta::Lines { x, y } } else { mouse::ScrollDelta::Pixels { x, y } };
            if w.deliver(Event::Mouse(mouse::Event::WheelScrolled { delta })) { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        });
        w.add_controller(scroll);

        // Accents, compose and input methods for other scripts.
        let im = gtk::IMMulticontext::new();
        im.set_client_widget(Some(&w));
        let me = w.downgrade();
        im.connect_commit(move |_, text| {
            if std::env::var_os("TINTA_DEBUG_IM").is_some() {
                eprintln!("im: commit {text:?}");
            }
            if let Some(w) = me.upgrade() {
                w.deliver(Event::InputMethod(gfx::input_method::Event::Commit(text.to_string())));
            }
        });
        let me = w.downgrade();
        im.connect_preedit_changed(move |im| {
            if let Some(w) = me.upgrade() {
                let (text, _, _) = im.preedit_string();
                w.deliver(Event::InputMethod(gfx::input_method::Event::Preedit(text.to_string(), None)));
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let me = w.downgrade();
        focus.connect_enter(move |_| {
            if let Some(w) = me.upgrade() {
                if let Some(im) = w.imp().im.borrow().as_ref() {
                    im.focus_in();
                }
                w.queue_draw();
            }
        });
        let me = w.downgrade();
        focus.connect_leave(move |_| {
            if let Some(w) = me.upgrade() {
                if let Some(im) = w.imp().im.borrow().as_ref() {
                    im.focus_out();
                }
                w.queue_draw();
            }
        });
        w.add_controller(focus);
        *imp.im.borrow_mut() = Some(im);
        let simple = gtk::IMContextSimple::new();
        simple.set_client_widget(Some(&w));
        let me = w.downgrade();
        simple.connect_commit(move |_, text| {
            if let Some(w) = me.upgrade() {
                w.imp().composing.set(false);
                w.deliver(Event::InputMethod(gfx::input_method::Event::Commit(text.to_string())));
            }
        });
        let me = w.downgrade();
        simple.connect_preedit_changed(move |im| {
            if let Some(w) = me.upgrade() {
                let (text, _, _) = im.preedit_string();
                w.deliver(Event::InputMethod(gfx::input_method::Event::Preedit(text.to_string(), None)));
            }
        });
        simple.focus_in();
        *imp.compose.borrow_mut() = Some(simple);
        w
    }

    pub fn tab(&self) -> TabId {
        self.imp().tab.get()
    }

    fn shell(&self) -> Option<Rc<Shell>> {
        self.imp().shell.borrow().upgrade()
    }

    fn is_diff(&self) -> bool {
        self.shell().is_some_and(|s| s.app.borrow().tabs.get(&self.tab()).is_some_and(|t| t.kind == TabKind::Diff))
    }

    /// Lets the input method look at a key first. True if it took it.
    pub fn filter_key(&self, ev: &gdk::Event) -> bool {
        let imp = self.imp();
        let simple = imp.compose.borrow().clone();
        if imp.composing.get()
            && let Some(s) = &simple
        {
            let took = s.filter_keypress(ev);
            // Escape and the like end it without a char.
            let (preedit, _, _) = s.preedit_string();
            if preedit.is_empty() {
                imp.composing.set(false);
            }
            return took;
        }
        if imp.im.borrow().as_ref().is_some_and(|im| im.filter_keypress(ev)) {
            return true;
        }
        let dead = ev.downcast_ref::<gdk::KeyEvent>().is_some_and(|k| super::keys::is_dead(k.keyval()));
        if dead && let Some(s) = &simple && s.filter_keypress(ev) {
            imp.composing.set(true);
            return true;
        }
        false
    }

    fn set_mods(&self, state: gdk::ModifierType) {
        let m = Modifiers::from_gdk(state);
        let mut st = self.imp().state.borrow_mut();
        let _ = &mut st;
        drop(st);
        self.deliver(Event::Keyboard(gfx::keyboard::Event::ModifiersChanged(m)));
    }

    fn bounds(&self) -> Rectangle {
        Rectangle::with_size(Size::new(self.width() as f32, self.height() as f32))
    }

    fn cursor(&self) -> mouse::Cursor {
        match self.imp().pointer.get() {
            Some((x, y)) => mouse::Cursor::Available(Point::new(x, y)),
            None => mouse::Cursor::Unavailable,
        }
    }

    /// Runs one event through the editor's own handling. True if it used it.
    pub fn deliver(&self, event: Event) -> bool {
        let Some(shell) = self.shell() else { return false };
        let tab = self.tab();
        let bounds = self.bounds();
        let cursor = self.cursor();
        let mut out = gfx::Shell::<Message>::default();
        {
            let app = shell.app.borrow();
            let Some(t) = app.tabs.get(&tab) else { return false };
            if t.kind == TabKind::Diff {
                let Some(data) = t.diff.as_deref() else { return false };
                let view = DiffView { data, metrics: app.metrics, on_event: Box::new(move |e: DiffEvent| Message::Diff(tab, e)) };
                view.update(&mut self.imp().diff_state.borrow_mut(), &event, bounds, cursor, &mut out);
            } else {
                let Some(view) = editor_view_for(&app, tab, self.has_focus()) else { return false };
                view.update(&mut self.imp().state.borrow_mut(), &event, bounds, cursor, &mut out);
            }
        }
        if let (Some(r), Some(im)) = (out.ime_cursor, self.imp().im.borrow().as_ref()) {
            im.set_cursor_location(&gdk::Rectangle::new(r.x as i32, r.y as i32, r.width as i32, r.height as i32));
        }
        if out.redraw {
            self.queue_draw();
        }
        if let Some(at) = out.redraw_at {
            self.wake_at(at);
        }
        let captured = out.captured;
        for m in out.messages {
            shell.dispatch(m);
        }
        captured
    }

    /// Another frame soon: for the caret's blink, hovering, and scrolling while dragging.
    fn wake_at(&self, at: Instant) {
        let imp = self.imp();
        if let Some((when, _)) = imp.timer.borrow().as_ref()
            && *when <= at
        {
            return;
        }
        if let Some((_, id)) = imp.timer.borrow_mut().take() {
            id.remove();
        }
        let delay = at.saturating_duration_since(Instant::now());
        let me = self.downgrade();
        let id = glib::timeout_add_local_once(delay, move || {
            if let Some(w) = me.upgrade() {
                w.imp().timer.borrow_mut().take();
                w.frame();
            }
        });
        *imp.timer.borrow_mut() = Some((at, id));
    }

    /// What iced called a redraw: the editor checks the time, reports its size, and repaints.
    pub fn frame(&self) {
        self.deliver(Event::Window(gfx::window::Event::RedrawRequested(Instant::now())));
        self.queue_draw();
    }

    fn later(&self) {
        let me = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(w) = me.upgrade() {
                w.frame();
            }
        });
    }

    fn update_cursor(&self) {
        let Some(shell) = self.shell() else { return };
        let app = shell.app.borrow();
        let Some(view) = editor_view_for(&app, self.tab(), self.has_focus()) else { return };
        let i = view.mouse_interaction(&self.imp().state.borrow(), self.bounds(), self.cursor());
        self.set_cursor_from_name(Some(i.css_name()));
    }

    fn paint(&self, snapshot: &gtk::Snapshot) {
        let Some(shell) = self.shell() else { return };
        let Ok(app) = shell.app.try_borrow() else { return };
        let imp = self.imp();
        let pal = palette(self);
        let pango = self.pango_context();
        // The character grid: a long run measured once per font, so rounding doesn't add up.
        {
            let key = (app.metrics.font, app.metrics.size);
            let mut st = imp.state.borrow_mut();
            if st.measured != Some(key) {
                let sample = gfx::Text { content: "M".repeat(100), size: app.metrics.size, line_h: app.metrics.line_h, font: app.metrics.font };
                let w = imp.cache.borrow_mut().measure(&pango, &sample).width;
                st.cell_w = (w / 100.0).max(1.0);
                st.measured = Some(key);
                imp.cache.borrow_mut().clear();
                let mut ds = imp.diff_state.borrow_mut();
                ds.cell_w = st.cell_w;
                ds.measured = Some(key);
                self.later();
            }
        }
        let mut painter = Painter { snapshot, pango, cache: &imp.cache };
        let bounds = self.bounds();
        let tab = self.tab();
        let Some(t) = app.tabs.get(&tab) else { return };
        if t.kind == TabKind::Diff {
            if let Some(data) = t.diff.as_deref() {
                let view = DiffView { data, metrics: app.metrics, on_event: Box::new(move |e: DiffEvent| Message::Diff(tab, e)) };
                let ui_font = gfx::Font { family: "Sans", weight: gfx::Weight::Normal, style: gfx::FontStyle::Normal };
                view.draw(&imp.diff_state.borrow(), &mut painter, &pal, bounds, ui_font);
            }
            return;
        }
        if let Some(view) = editor_view_for(&app, tab, self.has_focus()) {
            view.draw(&imp.state.borrow(), &mut painter, &pal, bounds, self.cursor());
        }
    }
}

/// The text area as the app's state describes it, for one tab.
pub fn editor_view_for(app: &App, tab_id: TabId, has_focus: bool) -> Option<EditorView<'_, Message>> {
    let tab = app.tabs.get(&tab_id)?;
    let entry = app.docs.get(&tab.doc)?;
    let active_group = app.groups.get(app.active_group).and_then(|g| g.active_tab()) == Some(tab_id);
    let blame = entry.blame.as_ref().filter(|_| app.settings.inline_blame && active_group).map(|(_, s)| s.as_str()).filter(|s| !s.is_empty());
    Some(EditorView {
        entry,
        tab,
        metrics: app.metrics,
        settings: &app.settings,
        focused: has_focus && active_group && app.focus == Focus::Editor && app.palette.is_none() && app.dialog.is_none(),
        current_match: if active_group { app.find.current } else { None },
        inline_blame: blame,
        breakpoints: entry.doc.path.as_ref().and_then(|p| app.debug.breakpoints.get(p)).map_or(&[], Vec::as_slice),
        exec_line: app.debug.session.as_ref().and_then(|s| s.exec_location()).filter(|(p, _, _)| entry.doc.path.as_deref() == Some(*p)).map(|(_, l, top)| (l, top)),
        cursor_style: match (app.settings.vim_mode, app.vim.mode) {
            (false, _) | (true, tinta_core::vim::Mode::Insert) => app.settings.cursor_style,
            (true, tinta_core::vim::Mode::Normal) => tinta_core::settings::CursorStyle::Block,
            (true, tinta_core::vim::Mode::Replace) => tinta_core::settings::CursorStyle::Underline,
            (true, _) => tinta_core::settings::CursorStyle::Bar,
        },
        on_event: Box::new(move |ev: EditorEvent| Message::Editor(tab_id, ev)),
    })
}
