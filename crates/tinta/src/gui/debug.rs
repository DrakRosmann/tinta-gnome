//! The debugger's side panel and console.

use super::Shell;
use super::rows;
use crate::app::{App, DEBUG_INPUT, DebugMsg, Message, WATCH_INPUT};
use crate::i18n::tr;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use tinta_core::dap::{BpState, RunState, VarRow};
use tinta_core::keymap::Command;

pub struct DebugSidebar {
    pub root: gtk::Box,
    tools: gtk::Box,
    state: gtk::Label,
    start: gtk::Box,
    list: gtk::ListBox,
    watch: gtk::Entry,
    last: RefCell<u64>,
}

fn tool(shell: &Rc<Shell>, icon: &str, tip: &str, cmd: Command, on: bool) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.add_css_class("flat");
    b.set_tooltip_text(Some(tip));
    b.set_sensitive(on);
    let s = Rc::downgrade(shell);
    b.connect_clicked(move |_| {
        if let Some(s) = s.upgrade() {
            s.dispatch(Message::Cmd(cmd));
        }
    });
    b
}

fn section(title: &'static str) -> gtk::ListBoxRow {
    let l = rows::label(&tr(title).to_uppercase());
    l.add_css_class("caption-heading");
    l.add_css_class("dim-label");
    l.set_margin_top(10);
    l.set_margin_start(6);
    gtk::ListBoxRow::builder().child(&l).activatable(false).selectable(false).build()
}

fn hint(text: &'static str) -> gtk::ListBoxRow {
    let l = rows::dim(tr(text));
    l.set_margin_start(10);
    gtk::ListBoxRow::builder().child(&l).activatable(false).selectable(false).build()
}

impl DebugSidebar {
    pub fn fields(&self) -> Vec<(&'static str, gtk::Widget)> {
        vec![(WATCH_INPUT, self.watch.clone().upcast())]
    }

    pub fn build(shell: &Rc<Shell>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = rows::label(tr("Run and Debug"));
        title.add_css_class("heading");
        title.set_margin_start(12);
        title.set_margin_top(6);
        root.append(&title);
        let tools = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        tools.set_margin_start(8);
        root.append(&tools);
        let state = rows::dim("");
        state.set_margin_start(12);
        root.append(&state);
        let start = gtk::Box::new(gtk::Orientation::Vertical, 6);
        start.set_margin_start(12);
        start.set_margin_end(12);
        let go = gtk::Button::with_label(tr("Start Debugging"));
        go.add_css_class("suggested-action");
        go.add_css_class("pill");
        let s = Rc::downgrade(shell);
        go.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Cmd(Command::DebugStart));
            }
        });
        start.append(&go);
        let more = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for (label, cmd) in [("Choose configuration", Command::DebugSelectConfig), ("Edit launch.json", Command::DebugConfigure)] {
            let b = gtk::Button::with_label(tr(label));
            b.add_css_class("flat");
            let s = Rc::downgrade(shell);
            b.connect_clicked(move |_| {
                if let Some(s) = s.upgrade() {
                    s.dispatch(Message::Cmd(cmd));
                }
            });
            more.append(&b);
        }
        start.append(&more);
        root.append(&start);
        let list = gtk::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_selection_mode(gtk::SelectionMode::None);
        let watch = rows::entry(shell, tr("Add an expression"), |t| Message::Debug(DebugMsg::WatchInput(t)));
        let s = Rc::downgrade(shell);
        watch.connect_activate(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Debug(DebugMsg::WatchSubmit));
            }
        });
        root.append(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build());
        DebugSidebar { root, tools, state, start, list, watch, last: RefCell::new(u64::MAX) }
    }

    fn var_row(&self, shell: &Rc<Shell>, r: VarRow, remove: Option<String>) -> gtk::ListBoxRow {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        b.append(&rows::spacer(r.depth as i32 * 12));
        let chevron = gtk::Image::from_icon_name(if r.expanded { "pan-down-symbolic" } else { "pan-end-symbolic" });
        chevron.set_opacity(if r.expandable { 0.8 } else { 0.0 });
        b.append(&chevron);
        if r.is_scope {
            let n = rows::label(&r.name);
            n.add_css_class("heading");
            b.append(&n);
        } else {
            let n = rows::label(&format!("{}:", r.name));
            n.add_css_class("monospace");
            b.append(&n);
            let v = rows::label(&r.value);
            v.add_css_class("monospace");
            v.add_css_class(if r.changed { "accent" } else { "dim-label" });
            v.set_hexpand(true);
            v.set_tooltip_text(Some(&r.value));
            b.append(&v);
        }
        if let Some(expr) = remove {
            let x = gtk::Button::from_icon_name("window-close-symbolic");
            x.add_css_class("flat");
            let s = Rc::downgrade(shell);
            x.connect_clicked(move |_| {
                if let Some(s) = s.upgrade() {
                    s.dispatch(Message::Debug(DebugMsg::WatchRemove(expr.clone())));
                }
            });
            b.append(&x);
        }
        let row = gtk::ListBoxRow::builder().child(&b).activatable(r.expandable).build();
        if r.expandable {
            let s = Rc::downgrade(shell);
            let click = gtk::GestureClick::new();
            click.connect_released(move |_, _, _, _| {
                if let Some(s) = s.upgrade() {
                    s.dispatch(Message::Debug(DebugMsg::ToggleRow(r.clone())));
                }
            });
            row.add_controller(click);
        }
        row
    }

    pub fn sync(&self, shell: &Rc<Shell>, app: &App) {
        let d = &app.debug;
        let live = d.session.as_ref().filter(|s| s.is_live());
        let stopped = d.is_stopped();
        rows::set_text(&self.watch, &d.watch_input);
        let key = rows::hash_of((
            live.map(|s| (format!("{:?}", s.state), s.variable_rows(), s.watch_rows(), s.frames.iter().map(|f| (f.name.clone(), f.line)).collect::<Vec<_>>(), s.frame, s.threads.len())),
            d.breakpoints.iter().map(|(p, l)| (p.clone(), l.iter().map(|b| (b.line, b.enabled, b.condition.clone(), b.state == BpState::Pending)).collect::<Vec<_>>())).collect::<Vec<_>>(),
            d.watches.clone(),
            d.after_build.is_some(),
        ));
        if *self.last.borrow() == key {
            return;
        }
        *self.last.borrow_mut() = key;
        while let Some(c) = self.tools.first_child() {
            self.tools.remove(&c);
        }
        self.start.set_visible(live.is_none());
        match live {
            Some(s) => {
                if s.state == RunState::Running {
                    self.tools.append(&tool(shell, "media-playback-pause-symbolic", tr("Pause"), Command::DebugPause, true));
                } else {
                    self.tools.append(&tool(shell, "media-playback-start-symbolic", tr("Continue (F5)"), Command::DebugStart, stopped));
                }
                self.tools.append(&tool(shell, "tinta-step-over-symbolic", tr("Step Over (F10)"), Command::StepOver, stopped));
                self.tools.append(&tool(shell, "tinta-step-in-symbolic", tr("Step Into (F11)"), Command::StepInto, stopped));
                self.tools.append(&tool(shell, "tinta-step-out-symbolic", tr("Step Out (Shift+F11)"), Command::StepOut, stopped));
                self.tools.append(&tool(shell, "view-refresh-symbolic", tr("Restart"), Command::DebugRestart, true));
                self.tools.append(&tool(shell, "media-playback-stop-symbolic", tr("Stop (Shift+F5)"), Command::DebugStop, true));
                let state = match &s.state {
                    RunState::Starting => tr("Starting").to_string(),
                    RunState::Running => tr("Running").to_string(),
                    RunState::Stopped { reason, description } => format!("{}: {}", tr("Paused"), description.clone().unwrap_or_else(|| reason.clone())),
                    RunState::Ended => tr("Debug session ended").to_string(),
                };
                self.state.set_label(&format!("{} · {state}", s.name));
            }
            None => self.state.set_label(""),
        }
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.list.append(&section("Variables"));
        match live {
            Some(s) if stopped => {
                for r in s.variable_rows() {
                    self.list.append(&self.var_row(shell, r, None));
                }
            }
            _ => self.list.append(&hint("Shown when the program pauses")),
        }
        self.list.append(&section("Watch"));
        let w = gtk::ListBoxRow::builder().activatable(false).selectable(false).build();
        if self.watch.parent().is_some() {
            self.watch.unparent();
        }
        w.set_child(Some(&self.watch));
        self.list.append(&w);
        match live {
            Some(s) if stopped => {
                for (i, r) in s.watch_rows() {
                    let expr = (r.depth == 0).then(|| s.watches[i].expression.clone());
                    self.list.append(&self.var_row(shell, r, expr));
                }
            }
            _ => {
                for w in &d.watches {
                    let r = VarRow { depth: 0, path: String::new(), name: w.clone(), value: String::new(), reference: 0, expandable: false, expanded: false, changed: false, is_scope: false };
                    self.list.append(&self.var_row(shell, r, Some(w.clone())));
                }
            }
        }
        self.list.append(&section("Call Stack"));
        match live {
            Some(s) if stopped => {
                for (i, f) in s.frames.iter().enumerate() {
                    let place = match &f.path {
                        Some(p) => format!("{}:{}", p.file_name().unwrap_or_default().to_string_lossy(), f.line + 1),
                        None => tr("no source").to_string(),
                    };
                    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                    let n = rows::label(&f.name);
                    n.add_css_class("monospace");
                    if i == s.frame {
                        n.add_css_class("heading");
                    }
                    b.append(&n);
                    b.append(&rows::dim(&place));
                    let row = gtk::ListBoxRow::builder().child(&b).build();
                    let sh = Rc::downgrade(shell);
                    let click = gtk::GestureClick::new();
                    click.connect_released(move |_, _, _, _| {
                        if let Some(sh) = sh.upgrade() {
                            sh.dispatch(Message::Debug(DebugMsg::SelectFrame(i)));
                        }
                    });
                    row.add_controller(click);
                    self.list.append(&row);
                }
            }
            _ => self.list.append(&hint("Shown when the program pauses")),
        }
        self.list.append(&section("Breakpoints"));
        if d.breakpoints.is_empty() {
            self.list.append(&hint("F9 or a click left of the line numbers"));
        }
        for (path, list) in &d.breakpoints {
            for b in list {
                let bx = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let check = gtk::CheckButton::new();
                check.set_active(b.enabled);
                let sh = Rc::downgrade(shell);
                let (p, l) = (path.clone(), b.line);
                check.connect_toggled(move |_| {
                    if let Some(sh) = sh.upgrade() {
                        sh.dispatch(Message::Debug(DebugMsg::BreakpointEnable(p.clone(), l)));
                    }
                });
                bx.append(&check);
                let name = rows::label(&format!("{}:{}", path.file_name().unwrap_or_default().to_string_lossy(), b.line + 1));
                if b.state == BpState::Pending {
                    name.add_css_class("dim-label");
                }
                bx.append(&name);
                if let Some(c) = &b.condition {
                    let cl = rows::dim(c);
                    cl.add_css_class("monospace");
                    bx.append(&cl);
                }
                let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                fill.set_hexpand(true);
                bx.append(&fill);
                let x = gtk::Button::from_icon_name("window-close-symbolic");
                x.add_css_class("flat");
                let sh = Rc::downgrade(shell);
                let (p, l) = (path.clone(), b.line);
                x.connect_clicked(move |_| {
                    if let Some(sh) = sh.upgrade() {
                        sh.dispatch(Message::Debug(DebugMsg::BreakpointRemove(p.clone(), l)));
                    }
                });
                bx.append(&x);
                let row = gtk::ListBoxRow::builder().child(&bx).build();
                let sh = Rc::downgrade(shell);
                let (p, l) = (path.clone(), b.line);
                let click = gtk::GestureClick::new();
                click.connect_released(move |_, _, _, _| {
                    if let Some(sh) = sh.upgrade() {
                        sh.dispatch(Message::Debug(DebugMsg::BreakpointJump(p.clone(), l)));
                    }
                });
                name.add_controller(click);
                self.list.append(&row);
            }
        }
    }
}

/// The debug console: the program's output, and expressions to evaluate.
pub struct Console {
    pub root: gtk::Box,
    view: gtk::TextView,
    input: gtk::Entry,
    shown: RefCell<(usize, String)>,
}

impl Console {
    pub fn fields(&self) -> Vec<(&'static str, gtk::Widget)> {
        vec![(DEBUG_INPUT, self.input.clone().upcast())]
    }

    pub fn build(shell: &Rc<Shell>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let view = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).left_margin(12).top_margin(4).build();
        let buf = view.buffer();
        buf.create_tag(Some("stderr"), &[("foreground", &"#e62d42")]);
        buf.create_tag(Some("dim"), &[("foreground-rgba", &gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0))]);
        buf.create_tag(Some("result"), &[("weight", &700)]);
        root.append(&gtk::ScrolledWindow::builder().child(&view).vexpand(true).build());
        let input = rows::entry(shell, tr("Expression to evaluate; -exec for a debugger command"), |t| Message::Debug(DebugMsg::Input(t)));
        input.set_margin_start(10);
        input.set_margin_end(10);
        input.set_margin_bottom(6);
        input.add_css_class("monospace");
        let s = Rc::downgrade(shell);
        input.connect_activate(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Debug(DebugMsg::Submit));
            }
        });
        root.append(&input);
        Console { root, view, input, shown: RefCell::new((0, String::new())) }
    }

    pub fn sync(&self, app: &App) {
        let d = &app.debug;
        rows::set_text(&self.input, &d.input);
        let first = d.console.first().map(|l| l.text.clone()).unwrap_or_default();
        let last_text = d.console.last().map(|l| l.text.clone()).unwrap_or_default();
        let key = (d.console.len(), format!("{first}\u{1}{last_text}"));
        if *self.shown.borrow() == key {
            return;
        }
        *self.shown.borrow_mut() = key;
        let buf = self.view.buffer();
        buf.set_text("");
        for l in d.console.iter().rev().take(800).collect::<Vec<_>>().into_iter().rev() {
            let mut end = buf.end_iter();
            let tag = match l.category.as_str() {
                "stderr" => Some("stderr"),
                "input" | "console" => Some("dim"),
                "result" => Some("result"),
                _ => None,
            };
            match tag {
                Some(t) => buf.insert_with_tags_by_name(&mut end, &format!("{}\n", l.text), &[t]),
                None => buf.insert(&mut end, &format!("{}\n", l.text)),
            }
        }
        let mark = buf.create_mark(None, &buf.end_iter(), false);
        self.view.scroll_mark_onscreen(&mark);
        buf.delete_mark(&mark);
    }
}
