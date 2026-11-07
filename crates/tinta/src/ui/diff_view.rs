//! Two versions of a file, side by side, scrolling together.

use super::editor_view::{Palette, text_primitive};
use crate::gfx::{Border, Color, Event, Font, Point, Quad, Rectangle, Shell, Size, mouse};
use crate::state::{DiffData, Metrics};
use std::time::{Duration, Instant};

type Renderer<'a> = crate::gfx::Painter<'a>;
use tinta_core::Document;
use tinta_core::diff::RowKind;
use tinta_core::display::char_width;

#[derive(Debug, Clone)]
pub enum DiffEvent {
    Viewport { w: f32, h: f32, cell_w: f32 },
    Scroll { dx: f32, dy: f32 },
    ScrollTo { y: f32 },
    /// Double click on a line of the new version.
    OpenRight { line: usize },
}

const HEADER_H: f32 = 26.0;
const SCROLLBAR_W: f32 = 12.0;

pub struct DiffView<'a, Message> {
    pub data: &'a DiffData,
    pub metrics: Metrics,
    pub on_event: Box<dyn Fn(DiffEvent) -> Message + 'a>,
}

#[derive(Default)]
pub struct State {
    pub cell_w: f32,
    pub measured: Option<(Font, f32)>,
    reported: Option<(f32, f32, f32)>,
    last_click: Option<Instant>,
    drag_bar: bool,
}

fn quad(r: &mut Renderer, bounds: Rectangle, c: Color) {
    r.fill_quad(Quad { bounds, ..Quad::default() }, c);
}

fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

/// Digits for the larger of the two line counts, and a little air.
pub fn gutter_width(data: &DiffData, cell_w: f32) -> f32 {
    let lines = data.left.len_lines().max(data.right.len_lines()).max(1);
    ((lines.ilog10() as f32 + 1.0).max(3.0) + 1.5) * cell_w
}

fn draw_line(r: &mut Renderer, doc: &Document, line: usize, x0: f32, y: f32, clip: Rectangle, m: &Metrics, fg: Color) {
    let tab = doc.indent.width.max(1);
    let spans = doc.highlighter.as_ref().and_then(|h| h.line(line)).map(|l| &l.spans[..]).unwrap_or(&[]);
    let mut v = 0usize;
    let mut run = String::new();
    let mut run_v = 0usize;
    let mut run_color = fg;
    let mut span_i = 0usize;
    let flush = |r: &mut Renderer, run: &mut String, run_v: usize, color: Color| {
        if !run.is_empty() {
            let x = x0 + run_v as f32 * m.cell_w;
            r.fill_text(text_primitive(std::mem::take(run), m, m.font), Point::new(x, y), color, clip);
        }
    };
    for (col, ch) in doc.line_slice(line).chars().enumerate() {
        let w = char_width(ch, v, tab);
        let x = x0 + v as f32 * m.cell_w;
        if x > clip.x + clip.width {
            break;
        }
        while span_i < spans.len() && spans[span_i].end as usize <= col {
            span_i += 1;
        }
        let color = match spans.get(span_i).filter(|s| s.start as usize <= col) {
            Some(s) => Color::from_rgb8(s.style.fg[0], s.style.fg[1], s.style.fg[2]),
            None => fg,
        };
        if ch == '\t' || ch == ' ' || ch.is_control() || (ch as u32) > 0x24F {
            flush(r, &mut run, run_v, run_color);
            if (ch as u32) > 0x24F && !ch.is_control() {
                r.fill_text(text_primitive(ch.to_string(), m, m.font), Point::new(x, y), color, clip);
            }
        } else {
            if run.is_empty() || run_color != color {
                flush(r, &mut run, run_v, run_color);
                run_v = v;
                run_color = color;
            }
            run.push(ch);
        }
        v += w;
    }
    flush(r, &mut run, run_v, run_color);
}

fn col_to_vcol(doc: &Document, line: usize, col: usize) -> usize {
    tinta_core::display::char_to_vcol(doc.line_slice(line), col, doc.indent.width.max(1))
}

impl<'a, Message: Clone + 'a> DiffView<'a, Message> {
    pub fn update(&self, state: &mut State, event: &Event, b: Rectangle, cursor: mouse::Cursor, shell: &mut Shell<Message>) {
        let rep = (b.width, b.height, state.cell_w);
        if state.reported != Some(rep) && state.cell_w > 0.0 {
            state.reported = Some(rep);
            shell.publish((self.on_event)(DiffEvent::Viewport { w: rep.0, h: rep.1, cell_w: rep.2 }));
        }
        let m = self.metrics;
        let content = self.data.rows.len() as f32 * m.line_h + (b.height - HEADER_H);
        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if cursor.position_over(b).is_none() {
                    return;
                }
                let (dx, dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (-x * m.cell_w * 6.0, -y * m.line_h * 3.0),
                    mouse::ScrollDelta::Pixels { x, y } => (-x, -y),
                };
                shell.publish((self.on_event)(DiffEvent::Scroll { dx, dy }));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(p) = cursor.position_over(b) else { return };
                if p.x >= b.x + b.width - SCROLLBAR_W {
                    state.drag_bar = true;
                    let y = (p.y - b.y - HEADER_H).max(0.0) / (b.height - HEADER_H).max(1.0) * content;
                    shell.publish((self.on_event)(DiffEvent::ScrollTo { y }));
                    shell.capture_event();
                    return;
                }
                let double = state.last_click.is_some_and(|t| t.elapsed() < Duration::from_millis(400));
                state.last_click = Some(Instant::now());
                if double && p.x > b.x + (b.width - SCROLLBAR_W) / 2.0 {
                    let row = ((p.y - b.y - HEADER_H + self.data.scroll_y) / m.line_h).floor().max(0.0) as usize;
                    if let Some(line) = self.data.rows.get(row).and_then(|r| r.right) {
                        shell.publish((self.on_event)(DiffEvent::OpenRight { line: line as usize }));
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.drag_bar => {
                let y = (position.y - b.y - HEADER_H).max(0.0) / (b.height - HEADER_H).max(1.0) * content;
                shell.publish((self.on_event)(DiffEvent::ScrollTo { y }));
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left) | mouse::Event::CursorEntered) => {
                state.drag_bar = false;
            }
            _ => {}
        }
    }

    pub fn draw(&self, state: &State, r: &mut Renderer, pal: &Palette, b: Rectangle, ui_font: Font) {
        let pal = *pal;
        let m = Metrics { cell_w: if state.cell_w > 0.0 { state.cell_w } else { self.metrics.cell_w }, ..self.metrics };
        let data = self.data;
        let pane_w = (b.width - SCROLLBAR_W) / 2.0;
        let gutter = gutter_width(data, m.cell_w);
        let body_y = b.y + HEADER_H;
        let body_h = (b.height - HEADER_H).max(0.0);
        let panes = [(b.x, true), (b.x + pane_w, false)];

        // Titles.
        r.with_layer(Rectangle::new(b.position(), Size::new(b.width, HEADER_H)), |r| {
            quad(r, Rectangle::new(b.position(), Size::new(b.width, HEADER_H)), alpha(pal.fg, 0.04));
            quad(r, Rectangle::new(Point::new(b.x, body_y - 1.0), Size::new(b.width, 1.0)), alpha(pal.fg, 0.12));
            let small = Metrics { size: 12.0, line_h: HEADER_H, ..m };
            for ((x, left), title) in panes.iter().zip([&data.left_title, &data.right_title]) {
                let clip = Rectangle::new(Point::new(*x, b.y), Size::new(pane_w, HEADER_H));
                let f = ui_font;
                let color = if *left { pal.deleted } else { pal.added };
                r.fill_text(text_primitive(title.clone(), &small, f), Point::new(x + 10.0, b.y), color, clip);
            }
        });

        let first = (data.scroll_y / m.line_h).floor().max(0.0) as usize;
        let y_off = -(data.scroll_y - first as f32 * m.line_h);
        let count = (body_h / m.line_h).ceil() as usize + 1;
        for (x, left) in panes {
            let pane = Rectangle::new(Point::new(x, body_y), Size::new(pane_w, body_h));
            let doc = if left { &data.left } else { &data.right };
            let text_x = x + gutter - data.scroll_x;
            let text_clip = Rectangle::new(Point::new(x + gutter, body_y), Size::new((pane_w - gutter).max(0.0), body_h));
            r.with_layer(pane, |r| {
                for (i, row) in data.rows.iter().enumerate().skip(first).take(count) {
                    let y = body_y + y_off + (i - first) as f32 * m.line_h;
                    let line = if left { row.left } else { row.right };
                    let band = Rectangle::new(Point::new(x, y), Size::new(pane_w, m.line_h));
                    let base = if left { pal.deleted } else { pal.added };
                    match (line, row.kind) {
                        (None, _) => quad(r, band, alpha(pal.fg, 0.035)),
                        (_, RowKind::Equal) => {}
                        (Some(_), _) => quad(r, band, alpha(base, 0.15)),
                    }
                    let Some(line) = line else { continue };
                    let line = line as usize;
                    if line >= doc.len_lines() {
                        continue;
                    }
                    // What differs inside the line stands out more.
                    let spans = if left { &row.left_spans } else { &row.right_spans };
                    for (a, b2) in spans {
                        let xa = text_x + col_to_vcol(doc, line, *a as usize) as f32 * m.cell_w;
                        let xb = text_x + col_to_vcol(doc, line, *b2 as usize) as f32 * m.cell_w;
                        let rect = Rectangle::new(Point::new(xa.max(text_clip.x), y), Size::new((xb - xa.max(text_clip.x)).max(0.0), m.line_h));
                        r.fill_quad(
                            Quad { bounds: rect, border: Border { radius: 2.0, ..Border::default() }, ..Quad::default() },
                            alpha(base, 0.38),
                        );
                    }
                    let label = (line + 1).to_string();
                    let lx = x + gutter - (label.len() as f32 + 1.0) * m.cell_w;
                    r.fill_text(text_primitive(label, &m, m.font), Point::new(lx, y), pal.gutter, pane);
                    draw_line(r, doc, line, text_x, y, text_clip, &m, pal.fg);
                }
                quad(r, Rectangle::new(Point::new(x + pane_w - 1.0, body_y), Size::new(1.0, body_h)), alpha(pal.fg, 0.14));
            });
        }

        // Scrollbar, with where the changes are.
        let bar = Rectangle::new(Point::new(b.x + b.width - SCROLLBAR_W, body_y), Size::new(SCROLLBAR_W, body_h));
        r.with_layer(bar, |r| {
            let rows = data.rows.len().max(1) as f32;
            let content = rows * m.line_h + body_h;
            let thumb_h = (body_h / content * body_h).max(24.0);
            let thumb_y = data.scroll_y / content * body_h;
            if rows * m.line_h > body_h {
                quad(r, Rectangle::new(Point::new(bar.x + 3.0, bar.y + thumb_y), Size::new(SCROLLBAR_W - 6.0, thumb_h)), alpha(pal.fg, 0.22));
            }
            for (i, row) in data.rows.iter().enumerate() {
                let c = match row.kind {
                    RowKind::Equal => continue,
                    RowKind::Removed => pal.deleted,
                    RowKind::Added => pal.added,
                    RowKind::Changed => pal.modified,
                };
                let y = bar.y + i as f32 / rows * body_h;
                quad(r, Rectangle::new(Point::new(bar.x, y), Size::new(SCROLLBAR_W, 2.0)), alpha(c, 0.8));
            }
        });
    }
}
