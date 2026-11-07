//! The bottom panel: terminals (VTE), build tasks, problems, output and the debug console.

use super::Shell;
use crate::app::{App, Message, PanelView, TaskMsg};
use crate::i18n::tr;
use adw::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use vte::prelude::*;

pub struct Panel {
    pub root: gtk::Box,
    switcher: gtk::Box,
    buttons: HashMap<&'static str, gtk::ToggleButton>,
    stack: gtk::Stack,
    term_stack: gtk::Stack,
    term_tabs: gtk::Box,
    terms: RefCell<HashMap<u64, vte::Terminal>>,
    tasks: gtk::TextView,
    output: gtk::TextView,
    problems: gtk::ListBox,
    pub debug: super::debug::Console,
    last: RefCell<Synced>,
    syncing: Cell<bool>,
    dark: Cell<Option<bool>>,
}

#[derive(Default, PartialEq)]
struct Synced {
    tasks_len: usize,
    tasks_first: String,
    output_len: usize,
    problems: Vec<String>,
    term_titles: Vec<(u64, String)>,
    active_term: usize,
    view: Option<&'static str>,
}

fn view_name(v: PanelView) -> &'static str {
    match v {
        PanelView::Terminal => "terminal",
        PanelView::Tasks => "tasks",
        PanelView::Problems => "problems",
        PanelView::Output => "output",
        PanelView::Debug => "debug",
    }
}

fn mono_view() -> gtk::TextView {
    let v = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).wrap_mode(gtk::WrapMode::None).build();
    v.add_css_class("panel-text");
    v.set_left_margin(12);
    v.set_top_margin(6);
    v
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder().child(child).vexpand(true).hexpand(true).build()
}

impl Panel {
    pub fn build(shell: &Rc<Shell>) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("tinta-panel");
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        header.set_margin_start(8);
        header.set_margin_end(6);
        header.set_margin_top(4);
        header.set_margin_bottom(2);
        let switcher = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        switcher.add_css_class("linked");
        header.append(&switcher);
        let term_tabs = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        term_tabs.set_margin_start(12);
        header.append(&term_tabs);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);
        let new_term = gtk::Button::from_icon_name("list-add-symbolic");
        new_term.set_tooltip_text(Some(tr("New Terminal")));
        new_term.add_css_class("flat");
        let s = Rc::downgrade(shell);
        new_term.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Cmd(tinta_core::keymap::Command::NewTerminal));
            }
        });
        header.append(&new_term);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some(tr("Close")));
        close.add_css_class("flat");
        let s = Rc::downgrade(shell);
        close.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::PanelClose);
            }
        });
        header.append(&close);
        root.append(&header);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        let term_stack = gtk::Stack::new();
        stack.add_named(&term_stack, Some("terminal"));
        let tasks = mono_view();
        stack.add_named(&scrolled(&tasks), Some("tasks"));
        let problems = gtk::ListBox::new();
        problems.add_css_class("navigation-sidebar");
        stack.add_named(&scrolled(&problems), Some("problems"));
        let output = mono_view();
        stack.add_named(&scrolled(&output), Some("output"));
        let debug = super::debug::Console::build(shell);
        stack.add_named(&debug.root, Some("debug"));
        root.append(&stack);

        let mut buttons = HashMap::new();
        for (name, label, view) in [
            ("terminal", "Terminal", PanelView::Terminal),
            ("tasks", "Tasks", PanelView::Tasks),
            ("problems", "Problems", PanelView::Problems),
            ("output", "Output", PanelView::Output),
            ("debug", "Debug Console", PanelView::Debug),
        ] {
            let b = gtk::ToggleButton::with_label(tr(label));
            b.add_css_class("flat");
            let s = Rc::downgrade(shell);
            b.connect_clicked(move |_| {
                if let Some(s) = s.upgrade() {
                    s.dispatch(Message::PanelShow(view));
                }
            });
            switcher.append(&b);
            buttons.insert(name, b);
        }

        let panel = Rc::new(Panel {
            root,
            switcher,
            buttons,
            stack,
            term_stack,
            term_tabs,
            terms: RefCell::new(HashMap::new()),
            tasks,
            output,
            problems,
            debug,
            last: RefCell::new(Synced::default()),
            syncing: Cell::new(false),
            dark: Cell::new(None),
        });
        let s = Rc::downgrade(shell);
        panel.problems.connect_row_activated(move |_, row| {
            let Some(s) = s.upgrade() else { return };
            let i = row.index();
            let target = s.app.borrow().problems().into_iter().nth(i as usize).map(|(p, l, c, _, _)| (p, l, c));
            if let Some((p, l, c)) = target {
                s.dispatch(Message::ProblemJump(p, l, c));
            }
        });
        let s = Rc::downgrade(shell);
        let tv = panel.tasks.clone();
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, x, y| {
            let Some(s) = s.upgrade() else { return };
            let (bx, by) = tv.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);
            if let Some(iter) = tv.iter_at_location(bx, by) {
                let line = iter.line() as usize;
                let first = s.app.borrow().tasks.lines.len().saturating_sub(5000);
                let i = first + line.saturating_sub(1);
                if s.app.borrow().tasks.lines.get(i).is_some_and(|l| l.location.is_some()) {
                    s.dispatch(Message::Task(TaskMsg::Jump(i)));
                }
            }
        });
        panel.tasks.add_controller(click);
        panel
    }

    fn mono_font() -> pango::FontDescription {
        let family = super::shell::system_mono().unwrap_or_else(|| "Monospace".into());
        pango::FontDescription::from_string(&format!("{family} 11"))
    }

    /// libadwaita's view colors, and the GNOME palette for the 16 terminal colors.
    fn paint(term: &vte::Terminal, dark: bool) {
        let c = |hex: &str| gdk::RGBA::parse(hex).unwrap_or(gdk::RGBA::BLACK);
        let (fg, bg) = if dark { (c("#ffffff"), c("#1d1d20")) } else { (c("#1e1e20"), c("#ffffff")) };
        let palette: Vec<gdk::RGBA> = [
            "#241f31", "#c01c28", "#2ec27e", "#f5c211", "#1e78e4", "#9841bb", "#0ab9dc", "#c0bfbc",
            "#5e5c64", "#ed333b", "#57e389", "#f8e45c", "#51a1ff", "#c061cb", "#4fd2fd", "#f6f5f4",
        ]
        .iter()
        .map(|h| c(h))
        .collect();
        let refs: Vec<&gdk::RGBA> = palette.iter().collect();
        term.set_colors(Some(&fg), Some(&bg), &refs);
    }

    pub fn spawn_terminal(&self, shell: &Rc<Shell>, id: u64, cwd: &Path) {
        let term = vte::Terminal::new();
        term.set_font(Some(&Self::mono_font()));
        Self::paint(&term, adw::StyleManager::default().is_dark());
        term.set_scrollback_lines(10_000);
        term.set_vexpand(true);
        term.set_hexpand(true);
        // file:line and links: Ctrl+click opens them.
        if let Ok(re) = vte::Regex::for_match(r"(https?://[^\s'\x22<>]+)|([~./\w-]*[\w-]+\.[A-Za-z0-9]+:\d+(:\d+)?)", 0x0040_0000 | 0x0000_0400) {
            term.match_add_regex(&re, 0);
        }
        let click = gtk::GestureClick::new();
        let s = Rc::downgrade(shell);
        let t = term.downgrade();
        click.connect_pressed(move |g, _, x, y| {
            let (Some(s), Some(t)) = (s.upgrade(), t.upgrade()) else { return };
            if !g.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                return;
            }
            if let (Some(text), _) = t.check_match_at(x, y) {
                s.dispatch(Message::TermLink(text.to_string(), 0));
                g.set_state(gtk::EventSequenceState::Claimed);
            }
        });
        term.add_controller(click);
        let s = Rc::downgrade(shell);
        term.connect_child_exited(move |_, _| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::TermExited(id));
            }
        });
        let s = Rc::downgrade(shell);
        term.connect_window_title_changed(move |t| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::TermTitle(id, t.window_title().map(|x| x.to_string())));
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let s = Rc::downgrade(shell);
        focus.connect_enter(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::TermFocus);
            }
        });
        term.add_controller(focus);
        let shell_cmd = {
            let configured = shell.app.borrow().settings.terminal_shell.clone();
            if configured.trim().is_empty() { std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()) } else { configured }
        };
        let env = ["TERM_PROGRAM=tinta", "COLORTERM=truecolor"];
        term.spawn_async(
            vte::PtyFlags::DEFAULT,
            cwd.to_str(),
            &[shell_cmd.as_str()],
            &env,
            glib::SpawnFlags::SEARCH_PATH,
            || {},
            -1,
            gio::Cancellable::NONE,
            |r| {
                if let Err(e) = r {
                    log::warn!("terminal: {e}");
                }
            },
        );
        self.term_stack.add_named(&term, Some(&format!("t{id}")));
        self.term_stack.set_visible_child(&term);
        self.terms.borrow_mut().insert(id, term);
    }

    pub fn paste_text(&self, id: u64, text: &str) {
        if let Some(t) = self.terms.borrow().get(&id) {
            t.paste_text(text);
        }
    }

    pub fn copy(&self, id: u64) {
        if let Some(t) = self.terms.borrow().get(&id) {
            t.copy_clipboard_format(vte::Format::Text);
        }
    }

    pub fn paste(&self, id: u64) {
        if let Some(t) = self.terms.borrow().get(&id) {
            t.paste_clipboard();
        }
    }

    pub fn kill(&self, id: u64) {
        if let Some(t) = self.terms.borrow_mut().remove(&id) {
            self.term_stack.remove(&t);
        }
    }

    pub fn focus_terminal(&self, id: u64) {
        if let Some(t) = self.terms.borrow().get(&id) {
            let t = t.clone();
            glib::idle_add_local_once(move || {
                t.grab_focus();
            });
        }
    }

    pub fn sync(&self, shell: &Rc<Shell>, app: &App) {
        if self.syncing.replace(true) {
            return;
        }
        let view = app.panel.map(view_name);
        let dark = adw::StyleManager::default().is_dark();
        if self.dark.get() != Some(dark) {
            for t in self.terms.borrow().values() {
                Self::paint(t, dark);
            }
            self.dark.set(Some(dark));
        }
        let mut last = self.last.borrow_mut();
        if last.view != view {
            if let Some(v) = view {
                self.stack.set_visible_child_name(v);
            }
            for (name, b) in &self.buttons {
                b.set_active(Some(*name) == view);
            }
            last.view = view;
        }
        // Terminals that ended on their own.
        let alive: Vec<u64> = app.terms.iter().map(|t| t.id).collect();
        let gone: Vec<u64> = self.terms.borrow().keys().copied().filter(|id| !alive.contains(id)).collect();
        for id in gone {
            self.kill(id);
        }
        let titles: Vec<(u64, String)> = app.terms.iter().map(|t| (t.id, t.title.clone())).collect();
        if last.term_titles != titles || last.active_term != app.active_term {
            while let Some(c) = self.term_tabs.first_child() {
                self.term_tabs.remove(&c);
            }
            for (i, (id, title)) in titles.iter().enumerate() {
                let label = if title.is_empty() { format!("{}", i + 1) } else { format!("{}: {}", i + 1, truncate(title, 24)) };
                let b = gtk::ToggleButton::with_label(&label);
                b.add_css_class("flat");
                b.set_active(i == app.active_term);
                let s = Rc::downgrade(shell);
                b.connect_clicked(move |_| {
                    if let Some(s) = s.upgrade() {
                        s.dispatch(Message::TermSelect(i));
                    }
                });
                self.term_tabs.append(&b);
                if i == app.active_term
                    && let Some(t) = self.terms.borrow().get(id)
                {
                    self.term_stack.set_visible_child(t);
                }
            }
            last.term_titles = titles;
            last.active_term = app.active_term;
        }
        self.term_tabs.set_visible(app.panel == Some(PanelView::Terminal));

        // Build output: appended as it comes, so a long build doesn't redraw everything.
        let lines = &app.tasks.lines;
        let first = lines.first().map(|l| l.text.clone()).unwrap_or_default();
        if lines.len() != last.tasks_len || first != last.tasks_first {
            let buf = self.tasks.buffer();
            let start = if lines.len() >= last.tasks_len && first == last.tasks_first { last.tasks_len } else {
                buf.set_text("");
                0
            };
            let mut text = String::new();
            if start == 0 {
                text.push_str(&task_status(app));
                text.push('\n');
            }
            for l in &lines[start.max(lines.len().saturating_sub(5000))..] {
                text.push_str(&l.text);
                text.push('\n');
            }
            let mut end = buf.end_iter();
            buf.insert(&mut end, &text);
            last.tasks_len = lines.len();
            last.tasks_first = first;
            scroll_to_end(&self.tasks);
        }
        if app.output.len() != last.output_len {
            let buf = self.output.buffer();
            if app.output.len() < last.output_len {
                buf.set_text("");
                last.output_len = 0;
            }
            let mut end = buf.end_iter();
            let text: String = app.output[last.output_len..].iter().map(|l| format!("{l}\n")).collect();
            buf.insert(&mut end, &text);
            last.output_len = app.output.len();
            scroll_to_end(&self.output);
        }
        let problems: Vec<String> = app
            .problems()
            .into_iter()
            .take(500)
            .map(|(p, l, c, sev, msg)| {
                let rel = app.folder.as_ref().and_then(|f| p.strip_prefix(f).ok()).unwrap_or(&p).display().to_string();
                format!("{sev:?}\u{1}{}\u{1}{rel}:{}:{}", msg.lines().next().unwrap_or(""), l + 1, c + 1)
            })
            .collect();
        if problems != last.problems {
            while let Some(c) = self.problems.first_child() {
                self.problems.remove(&c);
            }
            for p in &problems {
                let mut parts = p.split('\u{1}');
                let (sev, msg, place) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let icon = gtk::Image::from_icon_name(match sev {
                    "Error" => "dialog-error-symbolic",
                    "Warning" => "dialog-warning-symbolic",
                    _ => "dialog-information-symbolic",
                });
                icon.add_css_class(match sev {
                    "Error" => "error",
                    "Warning" => "warning",
                    _ => "accent",
                });
                row.append(&icon);
                let m = gtk::Label::new(Some(msg));
                m.set_ellipsize(pango::EllipsizeMode::End);
                m.set_xalign(0.0);
                row.append(&m);
                let where_ = gtk::Label::new(Some(place));
                where_.add_css_class("dim-label");
                where_.add_css_class("caption");
                row.append(&where_);
                self.problems.append(&row);
            }
            last.problems = problems;
        }
        let count = last.problems.len();
        if let Some(b) = self.buttons.get("problems") {
            b.set_label(&if count > 0 { format!("{} ({count})", tr("Problems")) } else { tr("Problems").to_string() });
        }
        if let Some(b) = self.buttons.get("tasks") {
            b.set_label(&if app.tasks.running.is_some() { format!("{} …", tr("Tasks")) } else { tr("Tasks").to_string() });
        }
        drop(last);
        self.debug.sync(app);
        let _ = &self.switcher;
        self.syncing.set(false);
    }
}

fn task_status(app: &App) -> String {
    let t = &app.tasks;
    let Some(task) = &t.last else { return tr("No task has run yet").to_string() };
    match (&t.exit, t.running) {
        (_, Some(_)) => format!("{} {}", tr("Running"), task.command),
        (Some(Ok(())), _) => format!("{}: {}", task.command, tr("finished")),
        (Some(Err(Some(code))), _) => format!("{}: {} {code}", task.command, tr("Exit code")),
        _ => task.command.clone(),
    }
}

fn scroll_to_end(v: &gtk::TextView) {
    let buf = v.buffer();
    let mark = buf.create_mark(None, &buf.end_iter(), false);
    v.scroll_mark_onscreen(&mark);
    buf.delete_mark(&mark);
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let t: String = s.chars().take(n).collect();
    format!("{t}…")
}
