//! Owns the app's state, feeds it messages one at a time, runs the effects it asks for, and has
//! the window catch up with the state after each batch.

use super::window::Window;
use crate::app::{App, Flags, Message};
use crate::fx::{Effect, Task};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

pub struct Shell {
    pub app: RefCell<App>,
    queue: RefCell<VecDeque<Message>>,
    busy: Cell<bool>,
    pub window: OnceCell<Window>,
    frame_timer: RefCell<Option<glib::SourceId>>,
}

/// Icons the desktop's theme doesn't have, and the app's own, put where GTK looks for them.
fn install_icons() {
    const ICONS: &[(&str, &str)] = &[
        ("actions/tinta-git-symbolic.svg", include_str!("../../icons/tinta-git-symbolic.svg")),
        ("actions/tinta-debug-symbolic.svg", include_str!("../../icons/tinta-debug-symbolic.svg")),
        ("actions/tinta-step-over-symbolic.svg", include_str!("../../icons/tinta-step-over-symbolic.svg")),
        ("actions/tinta-step-in-symbolic.svg", include_str!("../../icons/tinta-step-in-symbolic.svg")),
        ("actions/tinta-step-out-symbolic.svg", include_str!("../../icons/tinta-step-out-symbolic.svg")),
        ("actions/tinta-plot-symbolic.svg", include_str!("../../icons/tinta-plot-symbolic.svg")),
        ("apps/io.github.DrakRosmann.TintaGnome.svg", include_str!("../../../../data/icons/scalable/apps/io.github.DrakRosmann.TintaGnome.svg")),
    ];
    let root = glib::user_cache_dir().join("tinta-gnome").join("icons");
    let dir = root.join("hicolor").join("scalable");
    for (name, svg) in ICONS {
        let p = dir.join(name);
        if std::fs::read_to_string(&p).ok().as_deref() != Some(*svg) {
            let _ = p.parent().map(std::fs::create_dir_all);
            let _ = std::fs::write(&p, svg);
        }
    }
    if let Some(display) = gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(&root);
    }
}

/// The desktop's monospace font, if libadwaita knows it.
pub fn system_mono() -> Option<String> {
    let name = adw::StyleManager::default().monospace_font_name();
    tinta_core::desktop::Font::parse(&name).map(|f| f.family)
}

fn mono_families() -> Vec<String> {
    let label = gtk::Label::new(None);
    let mut names: Vec<String> = label.pango_context().list_families().iter().filter(|f| f.is_monospace()).map(|f| f.name().to_string()).collect();
    names.sort();
    names.dedup();
    names
}

impl Shell {
    pub fn start(gapp: &adw::Application, targets: Vec<crate::Target>, new_session: bool) -> Rc<Self> {
        let style = adw::StyleManager::default();
        let flags = Flags { targets, new_session, dark: style.is_dark(), system_mono: system_mono(), font_names: mono_families() };
        let (app, first) = App::new(flags);
        let shell = Rc::new(Shell {
            app: RefCell::new(app),
            queue: RefCell::new(VecDeque::new()),
            busy: Cell::new(false),
            window: OnceCell::new(),
            frame_timer: RefCell::new(None),
        });
        install_icons();
        let window = Window::build(gapp, &shell);
        let _ = shell.window.set(window);

        // Background threads: language servers, git, builds, the debugger.
        let rx = crate::bus::receiver();
        let weak = Rc::downgrade(&shell);
        glib::spawn_future_local(async move {
            while let Ok(m) = rx.recv().await {
                let Some(s) = weak.upgrade() else { break };
                s.dispatch(m);
            }
        });
        let weak = Rc::downgrade(&shell);
        glib::timeout_add_local(Duration::from_millis(1000), move || match weak.upgrade() {
            Some(s) => {
                s.dispatch(Message::TickSlow);
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        // Dark or light and the monospace font, as the desktop changes them.
        for prop in ["dark", "monospace-font-name"] {
            let weak = Rc::downgrade(&shell);
            style.connect_notify_local(Some(prop), move |sm, _| {
                if let Some(s) = weak.upgrade() {
                    s.dispatch(Message::StyleChanged { dark: sm.is_dark(), mono: system_mono() });
                }
            });
        }
        shell.run(first);
        shell.flush();
        shell.present();
        shell
    }

    pub fn present(&self) {
        if let Some(w) = self.window.get() {
            w.window.present();
        }
    }

    /// One message, and whatever it sets off, then the window catches up.
    pub fn dispatch(self: &Rc<Self>, m: Message) {
        self.queue.borrow_mut().push_back(m);
        self.flush();
    }

    fn flush(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        self.busy.set(true);
        // Syncing widgets can make them emit signals, and those dispatch too: they wait their
        // turn in the queue, and the loop goes round again.
        loop {
            loop {
                let next = self.queue.borrow_mut().pop_front();
                let Some(m) = next else { break };
                let task = self.app.borrow_mut().update(m);
                self.run(task);
            }
            if let Some(w) = self.window.get() {
                w.sync(self);
            }
            if self.queue.borrow().is_empty() {
                break;
            }
        }
        self.busy.set(false);
        self.schedule_frames();
    }

    /// Highlighting a big file goes on a bit per frame until it's done.
    fn schedule_frames(self: &Rc<Self>) {
        let wanted = self.app.borrow().needs_frame_tick();
        if !wanted || self.frame_timer.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local(Duration::from_millis(16), move || {
            let Some(s) = weak.upgrade() else { return glib::ControlFlow::Break };
            if !s.app.borrow().needs_frame_tick() {
                s.frame_timer.borrow_mut().take();
                return glib::ControlFlow::Break;
            }
            s.dispatch(Message::TickFrame);
            glib::ControlFlow::Continue
        });
        *self.frame_timer.borrow_mut() = Some(id);
    }

    /// Effects run with the app's state free, so they can call back into it.
    fn run(self: &Rc<Self>, task: Task) {
        for e in task.0 {
            self.effect(e);
        }
    }

    fn effect(self: &Rc<Self>, e: Effect) {
        let Some(w) = self.window.get() else {
            if let Effect::Message(m) = e {
                self.queue.borrow_mut().push_back(m);
            }
            return;
        };
        let display = gdk::Display::default().expect("a display");
        match e {
            Effect::Message(m) => self.queue.borrow_mut().push_back(m),
            Effect::Focus(id) => w.focus_field(id, false),
            Effect::SelectAll(id) => w.focus_field(id, true),
            Effect::FocusEditor => w.focus_editor(),
            Effect::ClipboardWrite(text) => display.clipboard().set_text(&text),
            Effect::ClipboardRead(then) => {
                let weak = Rc::downgrade(self);
                display.clipboard().read_text_async(gio::Cancellable::NONE, move |r| {
                    let text = r.ok().flatten().map(|t| t.to_string());
                    if let Some(s) = weak.upgrade() {
                        s.dispatch(then(text));
                    }
                });
            }
            Effect::OpenFiles(then) => {
                let weak = Rc::downgrade(self);
                gtk::FileDialog::new().open_multiple(Some(&w.window), gio::Cancellable::NONE, move |r| {
                    let Ok(list) = r else { return };
                    let paths: Vec<_> = (0..list.n_items()).filter_map(|i| list.item(i).and_downcast::<gio::File>()).filter_map(|f| f.path()).collect();
                    if let Some(s) = weak.upgrade() {
                        s.dispatch(then(paths));
                    }
                });
            }
            Effect::OpenFolder(then) => {
                let weak = Rc::downgrade(self);
                gtk::FileDialog::new().select_folder(Some(&w.window), gio::Cancellable::NONE, move |r| {
                    if let (Ok(f), Some(s)) = (r, weak.upgrade())
                        && let Some(p) = f.path()
                    {
                        s.dispatch(then(p));
                    }
                });
            }
            Effect::SaveAs { name, folder, then } => {
                let d = gtk::FileDialog::builder().initial_name(name).build();
                if let Some(f) = folder {
                    d.set_initial_folder(Some(&gio::File::for_path(f)));
                }
                let weak = Rc::downgrade(self);
                d.save(Some(&w.window), gio::Cancellable::NONE, move |r| {
                    if let (Ok(f), Some(s)) = (r, weak.upgrade())
                        && let Some(p) = f.path()
                    {
                        s.dispatch(then(p));
                    }
                });
            }
            Effect::Fullscreen(on) => {
                if on {
                    w.window.fullscreen()
                } else {
                    w.window.unfullscreen()
                }
            }
            Effect::Quit => w.quit(),
            Effect::Screenshot(path) => w.screenshot(&path),
            Effect::TermSpawn { id, cwd } => w.panel.spawn_terminal(self, id, &cwd),
            Effect::TermAttach { id, master } => w.panel.attach_terminal(self, id, master),
            Effect::TermRun { id, cwd, argv } => w.panel.spawn_argv(self, id, &cwd, argv),
            Effect::TermInput { id, text } => w.panel.paste_text(id, &text),
            Effect::TermFeed { id, text } => w.panel.feed(id, &text),
            Effect::TermCopy(id) => w.panel.copy(id),
            Effect::TermPaste(id) => w.panel.paste(id),
            Effect::TermKill(id) => w.panel.kill(id),
            Effect::TermFocus(id) => w.panel.focus_terminal(id),
        }
    }
}
