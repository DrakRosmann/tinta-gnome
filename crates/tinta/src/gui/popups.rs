//! What floats over the text: suggestions, hover docs and parameter hints, placed at the caret;
//! and the command palette, over the window.

use super::Shell;
use super::rows;
use super::window::Window;
use crate::app::{App, Message, PALETTE_INPUT, PaletteMode, PaletteMsg};
use crate::i18n::tr;
use crate::ui::editor_view::Geometry;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

const COMPLETION_ROWS: usize = 10;
const COMPLETION_W: i32 = 460;

#[derive(Default)]
pub struct Popups {
    inner: RefCell<Option<Inner>>,
}

struct Inner {
    completion: gtk::Box,
    completion_list: gtk::ListBox,
    hover: gtk::Box,
    hover_body: gtk::Box,
    signature: gtk::Label,
    palette: gtk::Box,
    palette_entry: gtk::Entry,
    palette_list: gtk::ListBox,
    palette_scroll: gtk::ScrolledWindow,
    host: Option<gtk::Overlay>,
    last: RefCell<Shown>,
}

#[derive(Default, PartialEq)]
struct Shown {
    completion: Option<(usize, usize, Vec<usize>, u64)>,
    hover: Option<String>,
    palette: Option<(String, usize, usize, String)>,
}

fn popup_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    b.add_css_class("tinta-popup");
    b.set_halign(gtk::Align::Start);
    b.set_valign(gtk::Align::Start);
    b.set_visible(false);
    b.set_can_target(true);
    b
}

/// Markdown from a language server, as Pango markup: headings, code blocks, rules.
fn hover_markup(md: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut code = false;
    let mut para = String::new();
    let flush = |out: &mut Vec<(bool, String)>, para: &mut String| {
        if !para.trim().is_empty() {
            out.push((false, para.trim_end().to_string()));
        }
        para.clear();
    };
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            flush(&mut out, &mut para);
            code = !code;
            continue;
        }
        if code {
            out.push((true, line.to_string()));
            continue;
        }
        if line.trim() == "---" {
            flush(&mut out, &mut para);
            out.push((false, "\u{1}rule".into()));
            continue;
        }
        let clean = line.replace("**", "").replace('`', "").replace("\\_", "_").replace('\\', "");
        let clean = if clean.starts_with('#') { clean.trim_start_matches('#').trim_start().to_string() } else { clean };
        if clean.trim().is_empty() {
            flush(&mut out, &mut para);
        } else {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(&clean);
        }
    }
    flush(&mut out, &mut para);
    out
}

impl Popups {
    fn ensure(&self, win: &Window, shell: &Rc<Shell>) {
        if self.inner.borrow().is_some() {
            return;
        }
        let completion = popup_box();
        completion.set_size_request(COMPLETION_W, -1);
        let completion_list = gtk::ListBox::new();
        completion_list.add_css_class("navigation-sidebar");
        completion_list.set_selection_mode(gtk::SelectionMode::Single);
        let s = Rc::downgrade(shell);
        completion_list.connect_row_activated(move |_, row| {
            if let Some(s) = s.upgrade() {
                let first: usize = row.widget_name().parse().unwrap_or(0);
                s.dispatch(Message::CompletionPick(first));
            }
        });
        completion_list.set_activate_on_single_click(true);
        completion.append(&completion_list);
        let hover = popup_box();
        hover.set_size_request(-1, -1);
        let hover_body = gtk::Box::new(gtk::Orientation::Vertical, 6);
        hover_body.set_margin_start(12);
        hover_body.set_margin_end(12);
        hover_body.set_margin_top(8);
        hover_body.set_margin_bottom(8);
        let hs = gtk::ScrolledWindow::builder().child(&hover_body).propagate_natural_height(true).propagate_natural_width(true).max_content_height(260).max_content_width(560).build();
        hover.append(&hs);
        let signature = gtk::Label::new(None);
        signature.add_css_class("tinta-popup");
        signature.add_css_class("monospace");
        signature.set_halign(gtk::Align::Start);
        signature.set_valign(gtk::Align::Start);
        signature.set_margin_top(0);
        signature.set_wrap(true);
        signature.set_max_width_chars(80);
        signature.set_visible(false);
        {
            let (w, m) = (&signature, 6);
            w.set_margin_start(m);
        }

        // The palette: a box at the top of the window, over everything.
        let palette = popup_box();
        palette.set_halign(gtk::Align::Fill);
        palette.set_margin_top(52);
        let palette_entry = rows::entry(shell, "", |t| Message::Palette(PaletteMsg::Input(t)));
        palette_entry.set_margin_start(8);
        palette_entry.set_margin_end(8);
        palette_entry.set_margin_top(8);
        let s = Rc::downgrade(shell);
        palette_entry.connect_activate(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Palette(PaletteMsg::Submit));
            }
        });
        palette.append(&palette_entry);
        let palette_list = gtk::ListBox::new();
        palette_list.add_css_class("navigation-sidebar");
        palette_list.set_activate_on_single_click(true);
        let s = Rc::downgrade(shell);
        palette_list.connect_row_activated(move |_, row| {
            if let Some(s) = s.upgrade() {
                let i: usize = row.widget_name().parse().unwrap_or(0);
                s.dispatch(Message::Palette(PaletteMsg::Click(i)));
            }
        });
        let palette_scroll = gtk::ScrolledWindow::builder().child(&palette_list).propagate_natural_height(true).max_content_height(420).hscrollbar_policy(gtk::PolicyType::Never).build();
        palette.append(&palette_scroll);
        // 620 px where there's room, the window's width less a margin where there isn't.
        let clamp = adw::Clamp::builder().maximum_size(620).tightening_threshold(620).child(&palette).valign(gtk::Align::Start).margin_start(12).margin_end(12).build();
        win.overlay.add_overlay(&clamp);
        win.register_field(PALETTE_INPUT, &palette_entry);
        *self.inner.borrow_mut() = Some(Inner {
            completion,
            completion_list,
            hover,
            hover_body,
            signature,
            palette,
            palette_entry,
            palette_list,
            palette_scroll,
            host: None,
            last: RefCell::new(Shown::default()),
        });
    }

    /// The active editor's overlay holds the popups that follow the caret.
    pub fn attach(&self, overlay: &gtk::Overlay) {
        let mut inner = self.inner.borrow_mut();
        let Some(i) = inner.as_mut() else { return };
        if i.host.as_ref() == Some(overlay) {
            return;
        }
        for w in [i.completion.upcast_ref::<gtk::Widget>(), i.hover.upcast_ref(), i.signature.upcast_ref()] {
            if let Some(old) = &i.host {
                old.remove_overlay(w);
            }
            overlay.add_overlay(w);
        }
        i.host = Some(overlay.clone());
    }

    pub fn sync(&self, win: &Window, shell: &Rc<Shell>, app: &App) {
        self.ensure(win, shell);
        let inner = self.inner.borrow();
        let Some(i) = inner.as_ref() else { return };
        let mut last = i.last.borrow_mut();
        self.sync_palette(i, &mut last, app);
        let Some((tab, entry)) = app.active() else {
            i.completion.set_visible(false);
            i.hover.set_visible(false);
            i.signature.set_visible(false);
            return;
        };
        let geo = Geometry::new(entry.doc.len_lines(), tab.view_w, &app.metrics, &app.settings);
        let at = |pos: usize| -> (f32, f32) {
            let (row, v) = tab.map.pos_to_row(&entry.doc, pos);
            let pad = tab.map.row(&entry.doc, row).pad as f32;
            (geo.text_x + (v as f32 + pad) * app.metrics.cell_w - tab.scroll_x, row as f32 * app.metrics.line_h - tab.scroll_y)
        };
        let line_h = app.metrics.line_h;
        let tab_id = app.active_tab_id();
        // Suggestions.
        match &app.completion {
            Some(c) if Some(c.tab) == tab_id && !c.visible.is_empty() => {
                let (x, y) = at(c.anchor);
                let below = (tab.view_h - y - line_h - 16.0).max(0.0);
                let above = (y - 16.0).max(0.0);
                let row_h = 30.0;
                let rows = ((below.max(above) / row_h) as usize).clamp(1, COMPLETION_ROWS.min(c.visible.len()).max(1));
                let start = if c.selected >= rows / 2 { (c.selected - rows / 2).min(c.visible.len().saturating_sub(rows)) } else { 0 };
                let key = Some((start, c.selected, c.visible.clone(), c.generation));
                if last.completion != key {
                    while let Some(ch) = i.completion_list.first_child() {
                        i.completion_list.remove(&ch);
                    }
                    for (n, idx) in c.visible.iter().enumerate().skip(start).take(rows) {
                        let e = &c.entries[*idx];
                        let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                        let kind = rows::dim(e.kind);
                        kind.set_width_chars(5);
                        kind.add_css_class("monospace");
                        b.append(&kind);
                        let chars: Vec<char> = e.label.chars().collect();
                        let mut markup = String::new();
                        for (k, ch) in chars.iter().enumerate() {
                            let esc = glib::markup_escape_text(&ch.to_string()).to_string();
                            if e.positions.contains(&(k as u32)) {
                                markup.push_str(&format!("<b>{esc}</b>"));
                            } else {
                                markup.push_str(&esc);
                            }
                        }
                        let l = gtk::Label::new(None);
                        l.set_markup(&markup);
                        l.add_css_class("monospace");
                        l.set_xalign(0.0);
                        b.append(&l);
                        let d = rows::dim(&e.detail);
                        d.set_hexpand(true);
                        d.set_xalign(1.0);
                        b.append(&d);
                        let row = gtk::ListBoxRow::builder().child(&b).build();
                        row.set_widget_name(&n.to_string());
                        i.completion_list.append(&row);
                        if n == c.selected {
                            i.completion_list.select_row(Some(&row));
                        }
                    }
                    last.completion = key;
                }
                let h = rows as f32 * row_h + 12.0;
                let py = if h <= below || below >= above { y + line_h + 2.0 } else { y - h - 2.0 };
                let px = x.min((tab.view_w - COMPLETION_W as f32 - 8.0).max(0.0)).max(0.0);
                i.completion.set_margin_start(px as i32);
                i.completion.set_margin_top(py.max(0.0) as i32);
                i.completion.set_visible(true);
            }
            _ => {
                i.completion.set_visible(false);
                last.completion = None;
            }
        }
        // Hover docs, when no list is open.
        match &app.hover {
            Some(h) if !i.completion.is_visible() => {
                if last.hover.as_deref() != Some(h.text.as_str()) {
                    while let Some(ch) = i.hover_body.first_child() {
                        i.hover_body.remove(&ch);
                    }
                    for (code, text) in hover_markup(&h.text) {
                        if text == "\u{1}rule" {
                            i.hover_body.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                            continue;
                        }
                        let l = gtk::Label::new(Some(&text));
                        l.set_xalign(0.0);
                        l.set_wrap(!code);
                        l.set_max_width_chars(80);
                        l.set_selectable(true);
                        if code {
                            l.add_css_class("monospace");
                        }
                        i.hover_body.append(&l);
                    }
                    last.hover = Some(h.text.clone());
                }
                let (x, y) = at(h.pos);
                let below = y + line_h + 2.0;
                let py = if below + 220.0 > tab.view_h && y - 222.0 > 0.0 { y - 222.0 } else { below };
                let px = x.min((tab.view_w - 560.0 - 8.0).max(0.0)).max(0.0);
                i.hover.set_margin_start(px as i32);
                i.hover.set_margin_top(py as i32);
                i.hover.set_visible(true);
            }
            _ => {
                i.hover.set_visible(false);
                last.hover = None;
            }
        }
        // Parameter hints, above the line being typed.
        match &app.signature {
            Some(s) => {
                let chars: Vec<char> = s.label.chars().collect();
                let (a, b) = s.active.unwrap_or((chars.len(), chars.len()));
                let (a, b) = (a.min(chars.len()), b.min(chars.len()));
                let esc = |x: usize, y: usize| glib::markup_escape_text(&chars[x..y].iter().collect::<String>()).to_string();
                i.signature.set_markup(&format!("{}<b>{}</b>{}", esc(0, a), esc(a, b), esc(b, chars.len())));
                let (x, y) = at(s.pos);
                i.signature.set_margin_start(x.min((tab.view_w - 500.0).max(0.0)).max(0.0) as i32);
                i.signature.set_margin_top((y - 40.0).max(0.0) as i32);
                i.signature.set_visible(true);
            }
            None => i.signature.set_visible(false),
        }
    }

    fn sync_palette(&self, i: &Inner, last: &mut Shown, app: &App) {
        let Some(p) = &app.palette else {
            i.palette.set_visible(false);
            last.palette = None;
            return;
        };
        let placeholder = match p.mode {
            PaletteMode::Commands => tr("Type a command…"),
            PaletteMode::Files => tr("Search files by name… (> commands, : line, @ symbol)"),
            PaletteMode::Line => tr("Go to line…"),
            PaletteMode::Symbols => tr("Go to symbol…"),
            _ => tr("Select…"),
        };
        i.palette_entry.set_placeholder_text(Some(placeholder));
        rows::set_text(&i.palette_entry, &p.query);
        i.palette.set_visible(true);
        let key = Some((p.query.clone(), p.items.len(), p.selected, p.items.iter().take(60).map(|x| x.label.as_str()).collect::<Vec<_>>().join("\u{1}")));
        if last.palette == key {
            return;
        }
        let items_changed = last.palette.as_ref().is_none_or(|(q, n, _, labels)| *q != p.query || *n != p.items.len() || Some(labels) != key.as_ref().map(|k| &k.3));
        if items_changed {
            while let Some(ch) = i.palette_list.first_child() {
                i.palette_list.remove(&ch);
            }
            for (n, it) in p.items.iter().enumerate().take(200) {
                let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let chars: Vec<char> = it.label.chars().collect();
                let mut markup = String::new();
                for (k, ch) in chars.iter().enumerate() {
                    let esc = glib::markup_escape_text(&ch.to_string()).to_string();
                    if it.positions.contains(&(k as u32)) {
                        markup.push_str(&format!("<b>{esc}</b>"));
                    } else {
                        markup.push_str(&esc);
                    }
                }
                let l = gtk::Label::new(None);
                l.set_markup(&markup);
                l.set_xalign(0.0);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                b.append(&l);
                let d = rows::dim(&it.detail);
                d.set_hexpand(true);
                b.append(&d);
                if !it.hint.is_empty() {
                    let h = rows::dim(&it.hint);
                    h.add_css_class("monospace");
                    b.append(&h);
                }
                let row = gtk::ListBoxRow::builder().child(&b).build();
                row.set_widget_name(&n.to_string());
                i.palette_list.append(&row);
            }
            if p.items.is_empty() {
                let l = rows::dim(tr("No results"));
                l.set_margin_start(10);
                l.set_margin_top(6);
                l.set_margin_bottom(6);
                i.palette_list.append(&l);
            }
        }
        if let Some(row) = i.palette_list.row_at_index(p.selected as i32) {
            i.palette_list.select_row(Some(&row));
            // Keep the selected row in view as the arrows move through a long list.
            let adj = i.palette_scroll.vadjustment();
            if let Some(b) = row.compute_bounds(&i.palette_list) {
                let (top, bottom) = (b.y() as f64, (b.y() + b.height()) as f64);
                if top < adj.value() {
                    adj.set_value(top);
                } else if bottom > adj.value() + adj.page_size() {
                    adj.set_value(bottom - adj.page_size());
                }
            }
        }
        *last = Shown { palette: key, ..std::mem::take(last) };
    }
}
