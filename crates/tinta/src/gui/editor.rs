//! The text area as a GTK widget: GTK's gestures go into the editor's own mouse handling, and
//! drawing goes through GSK. The same widget, with another painter, shows side-by-side diffs.

use super::Shell;
use crate::app::{App, Focus, Message};
use crate::gfx::{self, Color, Event, Modifiers, Painter, Point, Rectangle, Size, TextCache, mouse};
use tinta_core::commands;
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
        /// What the screen reader was last told: the text's revision and length, the caret,
        /// and the selection.
        pub a11y: Cell<Option<(u64, usize, usize, (usize, usize))>>,
        /// The text as the screen reader last knew it. While it's told of a removal, this is what
        /// it reads, since it asks for the text that went away.
        pub a11y_text: RefCell<Option<ropey::Rope>>,
        pub a11y_removing: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Editor {
        const NAME: &'static str = "TintaEditor";
        type Type = super::Editor;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::AccessibleText,);

        fn class_init(klass: &mut Self::Class) {
            klass.set_accessible_role(gtk::AccessibleRole::TextBox);
        }
    }

    impl ObjectImpl for Editor {}

    /// What a screen reader reads: the document, by character offsets, which are the rope's.
    impl AccessibleTextImpl for Editor {
        fn contents(&self, start: u32, end: u32) -> Option<glib::Bytes> {
            if self.a11y_removing.get()
                && let Some(old) = self.a11y_text.borrow().as_ref()
            {
                let len = old.len_chars();
                let (a, b) = ((start as usize).min(len), (end as usize).min(len));
                return Some(glib::Bytes::from_owned(old.slice(a..b.max(a)).to_string().into_bytes()));
            }
            self.obj().with_text(|_, _, doc| {
                let len = doc.len_chars();
                let (a, b) = ((start as usize).min(len), (end as usize).min(len));
                glib::Bytes::from_owned(doc.slice_string(a, b.max(a)).into_bytes())
            })
        }

        fn contents_at(&self, offset: u32, granularity: gtk::AccessibleTextGranularity) -> Option<(u32, u32, glib::Bytes)> {
            self.obj().with_text(|_, _, doc| {
                let len = doc.len_chars();
                let pos = (offset as usize).min(len);
                let (a, b) = match granularity {
                    gtk::AccessibleTextGranularity::Character => (pos, (pos + 1).min(len)),
                    gtk::AccessibleTextGranularity::Word => {
                        let (a, b) = commands::word_at(doc, pos);
                        if a < b { (a, b) } else { (pos, (pos + 1).min(len)) }
                    }
                    // A line of code is its sentence and its paragraph.
                    _ => {
                        let line = doc.char_to_line(pos);
                        let a = doc.rope.line_to_char(line);
                        let b = if line + 1 < doc.len_lines() { doc.rope.line_to_char(line + 1) } else { len };
                        (a, b)
                    }
                };
                (a as u32, b as u32, glib::Bytes::from_owned(doc.slice_string(a, b).into_bytes()))
            })
        }

        fn caret_position(&self) -> u32 {
            self.obj().with_text(|_, t, _| t.cursors.head() as u32).unwrap_or(0)
        }

        fn selection(&self) -> Vec<gtk::AccessibleTextRange> {
            self.obj()
                .with_text(|_, t, _| {
                    t.cursors.iter().filter(|s| !s.is_empty()).map(|s| gtk::AccessibleTextRange::new(s.start(), s.end() - s.start())).collect()
                })
                .unwrap_or_default()
        }

        fn attributes(&self, _offset: u32) -> Vec<(gtk::AccessibleTextRange, glib::GString, glib::GString)> {
            Vec::new()
        }

        fn default_attributes(&self) -> Vec<(glib::GString, glib::GString)> {
            Vec::new()
        }

        /// Where a range is on screen, for magnifiers and for following the caret.
        fn extents(&self, start: u32, end: u32) -> Option<gtk::graphene::Rect> {
            let obj = self.obj();
            obj.with_text(|app, t, doc| {
                let g = editor_view::Geometry::new(doc.len_lines(), t.view_w, &app.metrics, &app.settings);
                let m = &app.metrics;
                let at = |pos: usize| {
                    let (row, vcol) = t.map.pos_to_row(doc, pos.min(doc.len_chars()));
                    (g.text_x + vcol as f32 * m.cell_w - t.scroll_x, row as f32 * m.line_h - t.scroll_y)
                };
                let (x0, y0) = at(start as usize);
                let (x1, y1) = at((end as usize).max(start as usize));
                if y1 > y0 {
                    // Over several lines: from the first line's start to the last's end.
                    gtk::graphene::Rect::new(g.text_x, y0, (t.view_w - g.text_x).max(1.0), y1 - y0 + m.line_h)
                } else {
                    gtk::graphene::Rect::new(x0, y0, (x1 - x0).max(m.cell_w), m.line_h)
                }
            })
        }

        fn offset(&self, point: &gtk::graphene::Point) -> Option<u32> {
            self.obj().with_text(|app, t, doc| {
                let g = editor_view::Geometry::new(doc.len_lines(), t.view_w, &app.metrics, &app.settings);
                let m = &app.metrics;
                let row = ((point.y() + t.scroll_y) / m.line_h).max(0.0) as usize;
                let row = row.min(t.map.row_count(doc).saturating_sub(1));
                let vcol = ((point.x() - g.text_x + t.scroll_x) / m.cell_w).max(0.0) as usize;
                t.map.row_to_pos(doc, row, vcol) as u32
            })
        }
    }

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
        @implements gtk::Accessible, gtk::AccessibleText, gtk::Buildable, gtk::ConstraintTarget;
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
    /// The tab and its text, for the screen reader. `None` for a diff, or while the app is busy.
    fn with_text<R>(&self, f: impl FnOnce(&App, &crate::state::Tab, &tinta_core::Document) -> R) -> Option<R> {
        let shell = self.shell()?;
        let app = shell.app.try_borrow().ok()?;
        let t = app.tabs.get(&self.tab())?;
        if t.kind == TabKind::Diff {
            return None;
        }
        let e = app.docs.get(&t.doc)?;
        Some(f(&app, t, &e.doc))
    }

    /// Tells the screen reader what changed since it last heard: text typed or removed, the
    /// caret moving, the selection. Called after each update of the app.
    pub fn sync_accessible(&self) {
        let Some((now, name, readonly, rope)) = self.with_text(|_, t, doc| {
            let p = t.cursors.primary();
            ((doc.revision, doc.len_chars(), t.cursors.head(), (p.start(), p.end())), doc.file_name(), doc.readonly, doc.rope.clone())
        }) else {
            return;
        };
        let imp = self.imp();
        let before = imp.a11y.replace(Some(now));
        let old_text = imp.a11y_text.replace(Some(rope));
        let Some((rev, len, head, sel)) = before else {
            self.update_property(&[gtk::accessible::Property::Label(&name), gtk::accessible::Property::MultiLine(true), gtk::accessible::Property::ReadOnly(readonly)]);
            return;
        };
        if rev != now.0 {
            self.update_property(&[gtk::accessible::Property::Label(&name), gtk::accessible::Property::ReadOnly(readonly)]);
            // Typing leaves the caret after what came in; deleting, where it went from.
            let (new_len, caret) = (now.1, now.2);
            if new_len > len {
                let n = new_len - len;
                let start = caret.saturating_sub(n);
                self.update_contents(gtk::AccessibleTextContentChange::Insert, start as u32, (start + n) as u32);
            } else if new_len < len {
                // It reads what was removed from the text as it was.
                imp.a11y_text.replace(old_text);
                imp.a11y_removing.set(true);
                self.update_contents(gtk::AccessibleTextContentChange::Remove, caret as u32, (caret + len - new_len) as u32);
                imp.a11y_removing.set(false);
                let rope = self.with_text(|_, _, doc| doc.rope.clone());
                imp.a11y_text.replace(rope);
            }
        }
        if head != now.2 {
            self.update_caret_position();
        }
        if sel != now.3 {
            self.update_selection_bound();
        }
    }

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
            let button = g.current_button();
            if w.deliver(Event::Mouse(mouse::Event::ButtonPressed(button_of(button)))) {
                g.set_state(gtk::EventSequenceState::Claimed);
            }
            // The text's own menu, where the click was.
            if button == 3 && !w.is_diff() {
                w.context_menu(x, y);
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

    fn context_menu(&self, x: f64, y: f64) {
        let Some(shell) = self.shell() else { return };
        let menu = super::menu::editor_menu(&shell.app.borrow());
        let pop = gtk::PopoverMenu::from_model(Some(&menu));
        pop.set_parent(self);
        pop.set_has_arrow(false);
        pop.set_halign(gtk::Align::Start);
        pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        pop.popup();
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
