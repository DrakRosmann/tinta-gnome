//! The serial monitor's panel: the connection and its port and speed, what the board writes, a
//! plot of its numbers, and a field to send it lines.

use super::{Shell, rows};
use crate::app::{App, LineKind, Message, SERIAL_INPUT, SerialMsg, SerialState};
use crate::i18n::{is_portuguese, tr};
use adw::prelude::*;
use gtk::{cairo, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tinta_core::embedded::{BAUDS, serial::LineEnding};
use tinta_core::keymap::Command;

/// Colors for the plot's series, from the GNOME palette.
const SERIES: &[(f64, f64, f64)] = &[
    (0.21, 0.52, 0.89),
    (0.90, 0.38, 0.00),
    (0.18, 0.76, 0.49),
    (0.75, 0.38, 0.80),
    (0.96, 0.76, 0.07),
    (0.88, 0.11, 0.14),
    (0.60, 0.42, 0.27),
    (0.38, 0.21, 0.51),
];

#[derive(Default, PartialEq, Clone)]
struct Shown {
    epoch: u64,
    total: u64,
    timestamps: bool,
    partial: String,
}

pub struct SerialView {
    pub root: gtk::Box,
    status: gtk::Label,
    connect: gtk::Button,
    port: gtk::Button,
    baud: gtk::DropDown,
    upload: gtk::Button,
    timestamps: gtk::ToggleButton,
    plot_toggle: gtk::ToggleButton,
    plot: gtk::DrawingArea,
    view: gtk::TextView,
    scroller: gtk::ScrolledWindow,
    input: gtk::Entry,
    ending: gtk::DropDown,
    shown: RefCell<Shown>,
    plot_version: Cell<u64>,
    /// Set while the widgets are being put in step with the app, so they don't answer back.
    syncing: Rc<Cell<bool>>,
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tip));
    b.add_css_class("flat");
    b
}

impl SerialView {
    pub fn fields(&self) -> Vec<(&'static str, gtk::Widget)> {
        vec![(SERIAL_INPUT, self.input.clone().upcast())]
    }

    pub fn build(shell: &Rc<Shell>) -> Self {
        let pt = is_portuguese();
        let syncing = Rc::new(Cell::new(false));
        let send = |m: fn() -> Message| {
            let s = Rc::downgrade(shell);
            move || {
                if let Some(s) = s.upgrade() {
                    s.dispatch(m());
                }
            }
        };
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // The connection, the port and the speed, then uploading, then how it's shown.
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.set_margin_start(10);
        bar.set_margin_end(10);
        bar.set_margin_top(4);
        bar.set_margin_bottom(4);
        let connect = gtk::Button::with_label(tr("Connect"));
        let f = send(|| Message::Serial(SerialMsg::Toggle));
        connect.connect_clicked(move |_| f());
        bar.append(&connect);
        let port = gtk::Button::new();
        port.add_css_class("flat");
        port.set_tooltip_text(Some(Command::EmbeddedPort.title(pt)));
        let f = send(|| Message::Cmd(Command::EmbeddedPort));
        port.connect_clicked(move |_| f());
        bar.append(&port);
        let labels: Vec<String> = BAUDS.iter().map(|b| format!("{b} baud")).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let baud = gtk::DropDown::from_strings(&labels);
        baud.set_tooltip_text(Some(tr("Baud rate")));
        {
            let s = Rc::downgrade(shell);
            let syncing = syncing.clone();
            baud.connect_selected_notify(move |d| {
                if syncing.get() {
                    return;
                }
                if let (Some(s), Some(b)) = (s.upgrade(), BAUDS.get(d.selected() as usize)) {
                    s.dispatch(Message::Serial(SerialMsg::Baud(*b)));
                }
            });
        }
        bar.append(&baud);
        let status = gtk::Label::new(None);
        status.add_css_class("dim-label");
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_margin_start(6);
        bar.append(&status);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        bar.append(&spacer);
        let upload = gtk::Button::new();
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        content.append(&gtk::Image::from_icon_name("go-next-symbolic"));
        content.append(&gtk::Label::new(Some(tr("Upload"))));
        upload.set_child(Some(&content));
        upload.add_css_class("suggested-action");
        upload.set_tooltip_text(Some(Command::EmbeddedFlash.title(pt)));
        let f = send(|| Message::Cmd(Command::EmbeddedFlash));
        upload.connect_clicked(move |_| f());
        bar.append(&upload);
        let timestamps = gtk::ToggleButton::new();
        timestamps.set_icon_name("preferences-system-time-symbolic");
        timestamps.set_tooltip_text(Some(tr("Show the time of each line")));
        timestamps.add_css_class("flat");
        {
            let s = Rc::downgrade(shell);
            let syncing = syncing.clone();
            timestamps.connect_toggled(move |b| {
                if let (false, Some(s)) = (syncing.get(), s.upgrade()) {
                    s.dispatch(Message::Serial(SerialMsg::Timestamps(b.is_active())));
                }
            });
        }
        bar.append(&timestamps);
        let plot_toggle = gtk::ToggleButton::new();
        plot_toggle.set_icon_name("tinta-plot-symbolic");
        plot_toggle.set_tooltip_text(Some(tr("Plot the numbers the board writes")));
        plot_toggle.add_css_class("flat");
        {
            let s = Rc::downgrade(shell);
            let syncing = syncing.clone();
            plot_toggle.connect_toggled(move |b| {
                if let (false, Some(s)) = (syncing.get(), s.upgrade()) {
                    s.dispatch(Message::Serial(SerialMsg::Plot(b.is_active())));
                }
            });
        }
        bar.append(&plot_toggle);
        let clear = icon_button("edit-clear-all-symbolic", tr("Clear"));
        let f = send(|| Message::Serial(SerialMsg::Clear));
        clear.connect_clicked(move |_| f());
        bar.append(&clear);
        root.append(&bar);

        // The plot, above the text when it's on.
        let plot = gtk::DrawingArea::builder().height_request(60).hexpand(true).build();
        plot.set_margin_start(10);
        plot.set_margin_end(10);
        {
            let s = Rc::downgrade(shell);
            plot.set_draw_func(move |area, cr, w, h| {
                let Some(s) = s.upgrade() else { return };
                let Ok(app) = s.app.try_borrow() else { return };
                draw_plot(area, cr, w as f64, h as f64, &app);
            });
        }

        let view = gtk::TextView::builder().editable(false).cursor_visible(false).monospace(true).wrap_mode(gtk::WrapMode::WordChar).left_margin(12).top_margin(4).bottom_margin(4).build();
        view.add_css_class("panel-text");
        let buf = view.buffer();
        let dim = gtk::gdk::RGBA::new(0.5, 0.5, 0.5, 1.0);
        buf.create_tag(Some("time"), &[("foreground-rgba", &dim)]);
        buf.create_tag(Some("info"), &[("foreground-rgba", &dim), ("style", &gtk::pango::Style::Italic)]);
        buf.create_tag(Some("sent"), &[("foreground", &"#3584e4")]);
        buf.create_tag(Some("error"), &[("foreground", &"#e62d42")]);
        let scroller = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();
        scroller.set_size_request(-1, 40);
        // The plot over the text, the line between them dragged to share the room.
        let split = gtk::Paned::builder().orientation(gtk::Orientation::Vertical).start_child(&plot).end_child(&scroller).position(170).shrink_start_child(false).shrink_end_child(false).vexpand(true).build();
        root.append(&split);

        // Sending: the line, and what goes after it.
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        line.set_margin_start(10);
        line.set_margin_end(10);
        line.set_margin_bottom(6);
        line.set_margin_top(4);
        let input = rows::entry(shell, tr("Message to send to the board (Enter)"), |t| Message::Serial(SerialMsg::Input(t)));
        input.add_css_class("monospace");
        let f = send(|| Message::Serial(SerialMsg::Send));
        input.connect_activate(move |_| f());
        line.append(&input);
        let endings: Vec<&str> = LineEnding::ALL.iter().map(|e| e.label(pt)).collect();
        let ending = gtk::DropDown::from_strings(&endings);
        ending.set_tooltip_text(Some(tr("What goes after each line sent")));
        {
            let s = Rc::downgrade(shell);
            let syncing = syncing.clone();
            ending.connect_selected_notify(move |d| {
                if syncing.get() {
                    return;
                }
                if let (Some(s), Some(e)) = (s.upgrade(), LineEnding::ALL.get(d.selected() as usize)) {
                    s.dispatch(Message::Serial(SerialMsg::Ending(*e)));
                }
            });
        }
        line.append(&ending);
        let send_b = gtk::Button::with_label(tr("Send"));
        let f = send(|| Message::Serial(SerialMsg::Send));
        send_b.connect_clicked(move |_| f());
        line.append(&send_b);
        root.append(&line);

        SerialView {
            root,
            status,
            connect,
            port,
            baud,
            upload,
            timestamps,
            plot_toggle,
            plot,
            view,
            scroller,
            input,
            ending,
            shown: RefCell::new(Shown::default()),
            plot_version: Cell::new(u64::MAX),
            syncing,
        }
    }

    pub fn sync(&self, app: &App) {
        let s = &app.serial;
        self.syncing.set(true);
        let pt = is_portuguese();
        let (label, status) = match &s.state {
            SerialState::Off => (tr("Connect"), tr("Disconnected").to_string()),
            SerialState::Waiting => (tr("Disconnect"), tr("Waiting for the board…").to_string()),
            SerialState::Open(p) => (tr("Disconnect"), format!("● {}", p.rsplit('/').next().unwrap_or(p))),
        };
        self.connect.set_label(label);
        if matches!(s.state, SerialState::Off) {
            self.connect.add_css_class("suggested-action");
        } else {
            self.connect.remove_css_class("suggested-action");
        }
        self.status.set_label(&status);
        // What the monitor would connect to now: the choices, even before connecting.
        let (port, baud) = if s.is_on() { s.target.clone() } else { app.serial_target() };
        let port_label = port.as_deref().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).unwrap_or_else(|| tr("Automatic port").to_string());
        self.port.set_label(&format!("{port_label} ▾"));
        if let Some(i) = BAUDS.iter().position(|b| *b == baud)
            && self.baud.selected() != i as u32
        {
            self.baud.set_selected(i as u32);
        }
        if let Some(i) = LineEnding::ALL.iter().position(|e| *e == s.ending)
            && self.ending.selected() != i as u32
        {
            self.ending.set_selected(i as u32);
        }
        self.upload.set_visible(app.embedded.current.is_some());
        self.timestamps.set_active(s.timestamps);
        self.plot_toggle.set_active(s.show_plot);
        self.plot.set_visible(s.show_plot);
        self.input.set_placeholder_text(Some(if pt { "Mensagem para a placa (Enter envia)" } else { "Message for the board (Enter sends)" }));
        rows::set_text(&self.input, &s.input);
        self.syncing.set(false);

        if s.plot.version != self.plot_version.get() {
            self.plot_version.set(s.plot.version);
            self.plot.queue_draw();
        }
        self.sync_text(app);
    }

    fn sync_text(&self, app: &App) {
        let s = &app.serial;
        let now = Shown { epoch: s.epoch, total: s.total, timestamps: s.timestamps, partial: s.partial.clone() };
        let last = self.shown.borrow().clone();
        if last == now {
            return;
        }
        let buf = self.view.buffer();
        // Following the end only when the end is where the reader is: scrolled up, it stays put.
        let adj = self.scroller.vadjustment();
        let at_end = adj.value() + adj.page_size() >= adj.upper() - 24.0;
        let new = (now.total - last.total.min(now.total)) as usize;
        let full = last.epoch != now.epoch || last.timestamps != now.timestamps || new > s.lines.len() || last.total > now.total;
        if full {
            buf.set_text("");
        } else if let Some(mark) = buf.mark("partial") {
            // The line that was still coming in is replaced by what it became.
            let mut start = buf.iter_at_mark(&mark);
            let mut end = buf.end_iter();
            buf.delete(&mut start, &mut end);
        }
        let from = if full { s.lines.len().saturating_sub(2000) } else { s.lines.len() - new };
        for l in &s.lines[from..] {
            let mut end = buf.end_iter();
            if now.timestamps {
                buf.insert_with_tags_by_name(&mut end, &format!("{}  ", l.time), &["time"]);
            }
            let (prefix, tag) = match l.kind {
                LineKind::Received => ("", None),
                LineKind::Sent => ("→ ", Some("sent")),
                LineKind::Info => ("— ", Some("info")),
                LineKind::Error => ("! ", Some("error")),
            };
            let text = format!("{prefix}{}\n", l.text);
            match tag {
                Some(t) => buf.insert_with_tags_by_name(&mut end, &text, &[t]),
                None => buf.insert(&mut end, &text),
            }
        }
        // Too long a buffer slows the panel down: the oldest lines go.
        let extra = buf.line_count() - 3000;
        if extra > 0 {
            let mut start = buf.start_iter();
            let mut cut = buf.iter_at_line(extra).unwrap_or(buf.start_iter());
            buf.delete(&mut start, &mut cut);
        }
        match buf.mark("partial") {
            Some(m) => buf.move_mark(&m, &buf.end_iter()),
            None => {
                buf.create_mark(Some("partial"), &buf.end_iter(), true);
            }
        }
        if !now.partial.is_empty() {
            let mut end = buf.end_iter();
            buf.insert(&mut end, &now.partial);
        }
        *self.shown.borrow_mut() = now;
        if at_end || full {
            let mark = buf.create_mark(None, &buf.end_iter(), false);
            self.view.scroll_mark_onscreen(&mark);
            buf.delete_mark(&mark);
            let adj = adj.clone();
            glib::idle_add_local_once(move || adj.set_value(adj.upper() - adj.page_size()));
        }
    }
}

/// The plot: each series a line, scaled together, with the range on the left and a legend with
/// the latest values.
fn draw_plot(area: &gtk::DrawingArea, cr: &cairo::Context, w: f64, h: f64, app: &App) {
    let plot = &app.serial.plot;
    let fg = area.color();
    let (r, g, b) = (fg.red() as f64, fg.green() as f64, fg.blue() as f64);
    cr.select_font_face("monospace", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    cr.set_font_size(11.0);
    let Some((lo, hi)) = plot.range() else {
        cr.set_source_rgba(r, g, b, 0.5);
        let text = if is_portuguese() { "Nenhum número ainda: a placa pode escrever \"1 2 3\" ou \"temp:21.5,umid:40\" por linha" } else { "No numbers yet: the board can write \"1 2 3\" or \"temp:21.5,hum:40\" on each line" };
        cr.move_to(12.0, h / 2.0);
        let _ = cr.show_text(text);
        return;
    };
    let left = 64.0;
    let (top, bottom) = (8.0, h - 8.0);
    let span = hi - lo;
    let y_of = |v: f64| bottom - (v - lo) / span * (bottom - top);
    // The grid and the scale.
    cr.set_line_width(1.0);
    for i in 0..=4 {
        let v = lo + span * i as f64 / 4.0;
        let y = y_of(v).round() + 0.5;
        cr.set_source_rgba(r, g, b, 0.12);
        cr.move_to(left, y);
        cr.line_to(w, y);
        let _ = cr.stroke();
        cr.set_source_rgba(r, g, b, 0.6);
        let label = if span.abs() >= 100.0 || v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.2}") };
        cr.move_to(4.0, y + 4.0);
        let _ = cr.show_text(&label);
    }
    let width = plot.width.max(2) as f64;
    cr.set_line_width(1.6);
    for (i, (_, points)) in plot.series.iter().enumerate() {
        let (sr, sg, sb) = SERIES[i % SERIES.len()];
        cr.set_source_rgb(sr, sg, sb);
        // The newest point at the right edge.
        let offset = width - points.len() as f64;
        for (j, v) in points.iter().enumerate() {
            let x = left + (offset + j as f64) / (width - 1.0) * (w - left - 4.0);
            if j == 0 { cr.move_to(x, y_of(*v)) } else { cr.line_to(x, y_of(*v)) }
        }
        let _ = cr.stroke();
    }
    // The legend: each name, its color and its latest value.
    let mut x = left + 8.0;
    for (i, (name, points)) in plot.series.iter().enumerate() {
        let (sr, sg, sb) = SERIES[i % SERIES.len()];
        cr.set_source_rgb(sr, sg, sb);
        cr.rectangle(x, top + 4.0, 10.0, 10.0);
        let _ = cr.fill();
        cr.set_source_rgba(r, g, b, 0.9);
        let text = format!("{name}: {}", points.back().map(|v| if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v:.3}") }).unwrap_or_default());
        cr.move_to(x + 14.0, top + 13.0);
        let _ = cr.show_text(&text);
        x += 24.0 + cr.text_extents(&text).map(|e| e.x_advance()).unwrap_or(60.0);
    }
}
