//! The text area: gutter, text, cursors, minimap. It is a custom widget because everything in it
//! has to line up on a character grid and repaint every frame without re-laying out text.

use crate::gfx::{
    Border, Color, Event, Font, FontStyle, Modifiers, Point, Quad, Rectangle, Shell, Size, Text, Weight, input_method, keyboard,
    mouse, window,
};
use crate::state::{DocEntry, Metrics, Tab};
use std::time::{Duration, Instant};

type Renderer<'a> = crate::gfx::Painter<'a>;
use tinta_core::display::{self, char_width, enclosing_headers, is_foldable};
use tinta_core::git::LineMark;
use tinta_core::lsp::Severity;
use tinta_core::settings::{CursorStyle, LineNumbers, Settings, Whitespace};

const FOLD_W: f32 = 16.0;
/// Left of the line numbers: git's marks, and where breakpoints go.
const MARK_W: f32 = 22.0;
const MINIMAP_W: f32 = 84.0;
const SCROLLBAR_W: f32 = 12.0;
const MM_LINE_H: f32 = 3.0;
const MM_CHAR_W: f32 = 1.1;
const LEFT_PAD: f32 = 6.0;

/// What the widget tells the app. Positions are already grid coordinates.
#[derive(Debug, Clone)]
pub enum EditorEvent {
    Viewport { w: f32, h: f32, cell_w: f32 },
    Press { row: usize, vcol: usize, clicks: u8, mods: Modifiers, button: PressButton },
    Drag { row: usize, vcol: usize, mods: Modifiers },
    Release,
    Scroll { dx: f32, dy: f32 },
    ScrollTo { y: f32 },
    FoldToggle { row: usize },
    BreakpointToggle { row: usize },
    GutterSelect { row: usize, extend: bool },
    GutterDrag { row: usize },
    StickyJump { line: usize },
    Hover { row: usize, vcol: usize },
    HoverEnd,
    ZoomWheel { delta: f32 },
    Ime(String),
    Preedit(String),
    /// Mouse went down in the text area with the secondary button.
    SecondaryPress { row: usize, vcol: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressButton {
    Left,
    Middle,
}

/// Where everything sits inside the widget, in widget coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub gutter_w: f32,
    pub digits_w: f32,
    pub text_x: f32,
    pub text_w: f32,
    pub minimap_x: f32,
    pub minimap_w: f32,
    pub scrollbar_x: f32,
}

impl Geometry {
    pub fn new(line_count: usize, width: f32, m: &Metrics, s: &Settings) -> Self {
        let digits = (line_count.max(1).ilog10() as usize + 1).max(3);
        let digits_w = if s.line_numbers == LineNumbers::Off { 0.0 } else { digits as f32 * m.cell_w };
        let fold = if s.folding { FOLD_W } else { 4.0 };
        let gutter_w = MARK_W + digits_w + fold;
        let minimap_w = if s.minimap && width > 480.0 { MINIMAP_W } else { 0.0 };
        let scrollbar_x = width - SCROLLBAR_W;
        let minimap_x = scrollbar_x - minimap_w;
        Self {
            gutter_w,
            digits_w,
            text_x: gutter_w + LEFT_PAD,
            text_w: (minimap_x - gutter_w - LEFT_PAD).max(10.0),
            minimap_x,
            minimap_w,
            scrollbar_x,
        }
    }

    /// Columns available when wrapping.
    pub fn wrap_cols(&self, m: &Metrics) -> usize {
        ((self.text_w - m.cell_w) / m.cell_w).floor().max(8.0) as usize
    }
}

/// Colours derived from the desktop's style, so accent changes reach the editor.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub accent: Color,
    pub selection: Color,
    pub selection_dim: Color,
    pub current_line: Color,
    pub gutter: Color,
    pub gutter_active: Color,
    pub guide: Color,
    pub whitespace: Color,
    pub find: Color,
    pub find_current: Color,
    pub occurrence: Color,
    pub error: Color,
    pub warning: Color,
    pub info: Color,
    pub added: Color,
    pub modified: Color,
    pub deleted: Color,
    pub rainbow: [Color; 3],
}

fn alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}

impl Palette {
    /// From what the toolkit's style says: text, background, accent and the status colors.
    pub fn new(fg: Color, bg: Color, accent: Color, success: Color, warning: Color, error: Color, dark: bool) -> Self {
        let fg = Color { a: 1.0, ..fg };
        let bg = Color { a: 1.0, ..bg };
        Self {
            bg,
            fg,
            accent,
            selection: alpha(accent, if dark { 0.38 } else { 0.28 }),
            selection_dim: alpha(accent, 0.16),
            current_line: alpha(fg, 0.05),
            gutter: alpha(fg, 0.42),
            gutter_active: alpha(fg, 0.95),
            guide: alpha(fg, 0.10),
            whitespace: alpha(fg, 0.28),
            find: alpha(warning, 0.30),
            find_current: alpha(warning, 0.70),
            occurrence: alpha(fg, 0.11),
            error,
            warning,
            info: accent,
            added: success,
            modified: accent,
            deleted: error,
            rainbow: if dark {
                [
                    Color::from_rgb8(0xFF, 0xD7, 0x00),
                    Color::from_rgb8(0xDA, 0x70, 0xD6),
                    Color::from_rgb8(0x17, 0x9F, 0xFF),
                ]
            } else {
                [
                    Color::from_rgb8(0x95, 0x6F, 0x00),
                    Color::from_rgb8(0x9B, 0x2F, 0x9B),
                    Color::from_rgb8(0x0B, 0x62, 0xC4),
                ]
            },
        }
    }

    fn severity(&self, s: Severity) -> Color {
        match s {
            Severity::Error => self.error,
            Severity::Warning => self.warning,
            Severity::Info | Severity::Hint => self.info,
        }
    }
}

pub struct EditorView<'a, Message> {
    pub entry: &'a DocEntry,
    pub tab: &'a Tab,
    pub metrics: Metrics,
    pub settings: &'a Settings,
    pub focused: bool,
    pub current_match: Option<usize>,
    pub inline_blame: Option<&'a str>,
    pub breakpoints: &'a [tinta_core::dap::Breakpoint],
    /// Where the debugger stopped. The bool: the innermost frame.
    pub exec_line: Option<(usize, bool)>,
    pub cursor_style: CursorStyle,
    pub on_event: Box<dyn Fn(EditorEvent) -> Message + 'a>,
}

#[derive(Default, Clone, Copy, PartialEq)]
enum DragKind {
    #[default]
    None,
    Text,
    Column,
    Gutter,
    Scrollbar(f32),
    Minimap,
}

pub struct State {
    drag: DragKind,
    last_click: Option<(Instant, Point, u8)>,
    pub cell_w: f32,
    pub measured: Option<(Font, f32)>,
    reported: Option<(f32, f32, f32)>,
    blink_on: bool,
    next_blink: Instant,
    seen_stamp: u64,
    hover_at: Option<(Instant, usize, usize)>,
    hover_sent: bool,
    mods: Modifiers,
    last_cursor: Point,
    in_marks: bool,
    ime_open: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            drag: DragKind::None,
            last_click: None,
            cell_w: 0.0,
            measured: None,
            reported: None,
            blink_on: true,
            next_blink: Instant::now(),
            seen_stamp: u64::MAX,
            hover_at: None,
            hover_sent: false,
            mods: Modifiers::default(),
            last_cursor: Point::ORIGIN,
            in_marks: false,
            ime_open: false,
        }
    }
}

const BLINK: Duration = Duration::from_millis(530);
const HOVER_DELAY: Duration = Duration::from_millis(450);

impl<'a, Message: Clone + 'a> EditorView<'a, Message> {
    fn geometry(&self, width: f32) -> Geometry {
        Geometry::new(self.entry.doc.len_lines(), width, &self.metrics, self.settings)
    }

    fn sticky_lines(&self) -> Vec<usize> {
        if !self.settings.sticky_scroll || self.tab.scroll_y < 1.0 {
            return Vec::new();
        }
        let m = &self.metrics;
        let doc = &self.entry.doc;
        let first_row = (self.tab.scroll_y / m.line_h) as usize;
        let tab = doc.indent.width.max(1);
        // The row right below the sticky lines decides which blocks are open.
        let mut headers = Vec::new();
        for _ in 0..3 {
            let row = first_row + headers.len();
            let line = self.tab.map.row(doc, row).line as usize;
            let h = enclosing_headers(doc, line, tab, 4);
            if h.len() <= headers.len() {
                break;
            }
            headers = h;
        }
        // Sticky lines only hide things when they sit above the first visible row.
        let first_line = self.tab.map.row(doc, first_row).line as usize;
        let limit = first_line + headers.len();
        headers.retain(|l| *l < limit);
        headers
    }

    fn hit(&self, p: Point, bounds: Rectangle, g: &Geometry) -> (usize, usize) {
        let m = &self.metrics;
        let doc = &self.entry.doc;
        let y = p.y - bounds.y + self.tab.scroll_y;
        let row = ((y / m.line_h).floor().max(0.0) as usize).min(self.tab.map.row_count(doc).saturating_sub(1));
        let r = self.tab.map.row(doc, row);
        let x = p.x - bounds.x - g.text_x + self.tab.scroll_x - r.pad as f32 * m.cell_w;
        let vcol = (x / m.cell_w + 0.5).floor().max(0.0) as usize;
        (row, vcol)
    }

    fn minimap_offset(&self, height: f32) -> (f32, f32) {
        // Returns (first minimap row shown, scale from rows to document rows).
        let doc = &self.entry.doc;
        let rows = self.tab.map.row_count(doc) as f32;
        let visible = height / MM_LINE_H;
        if rows <= visible {
            return (0.0, 1.0);
        }
        let m = &self.metrics;
        let view_rows = height / m.line_h;
        let denom = (rows - view_rows).max(1.0);
        let t = (self.tab.scroll_y / m.line_h / denom).clamp(0.0, 1.0);
        ((rows - visible) * t, 1.0)
    }
}

#[derive(Clone, Copy)]
struct RowCtx {
    x0: f32,
    y: f32,
    clip: Rectangle,
}

pub(crate) fn text_primitive(content: String, m: &Metrics, font: Font) -> Text {
    Text { content, size: m.size, line_h: m.line_h, font }
}

fn quad(r: &mut Renderer, bounds: Rectangle, color: Color) {
    r.fill_quad(Quad { bounds, ..Quad::default() }, color);
}

fn rquad(r: &mut Renderer, bounds: Rectangle, color: Color, radius: f32) {
    r.fill_quad(
        Quad { bounds, border: Border { radius: radius, ..Border::default() }, ..Quad::default() },
        color,
    );
}

/// Chars the monospace grid can't trust to be exactly one cell wide get drawn on their own.
fn needs_own_cell(c: char) -> bool {
    (c as u32) > 0x24F && !matches!(c as u32, 0x2010..=0x2027 | 0x2190..=0x21FF | 0x2500..=0x259F)
}

impl<Message: Clone> EditorView<'_, Message> {
    fn draw_row_text(
        &self,
        r: &mut Renderer,
        row: tinta_core::display::Row,
        ctx: RowCtx,
        pal: &Palette,
        tab: usize,
    ) {
        let m = &self.metrics;
        let doc = &self.entry.doc;
        let line = row.line as usize;
        let info = doc.highlighter.as_ref().and_then(|h| h.line(line));
        let spans: &[tinta_core::syntax::Span] = info.map(|i| &i.spans[..]).unwrap_or(&[]);
        let noncode: &[(u32, u32)] = info.map(|i| &i.noncode[..]).unwrap_or(&[]);
        let mut depth = info.map(|i| i.depth).unwrap_or(0);
        let rainbow = self.settings.bracket_colors && info.is_some();

        let last_vis_x = ctx.clip.x + ctx.clip.width;
        let mut v = 0usize; // visual column in the line
        let mut v_row0: Option<usize> = None;
        let mut run = String::new();
        let mut run_v = 0usize;
        let mut run_color = pal.fg;
        let mut run_font = m.font;
        let mut span_i = 0usize;
        let flush = |r: &mut Renderer, run: &mut String, run_v: usize, v0: usize, color: Color, font: Font| {
            if run.is_empty() {
                return;
            }
            let x = ctx.x0 + (run_v - v0) as f32 * m.cell_w;
            let content = std::mem::take(run);
            r.fill_text(text_primitive(content, m, font), Point::new(x, ctx.y), color, ctx.clip);
        };

        let ws = self.settings.render_whitespace;
        let line_len = (row.end as usize).max(doc.line_len(line));
        let trailing_from = {
            let slice = doc.line_slice(line);
            let mut t = line_len;
            while t > 0 && matches!(slice.char(t - 1), ' ' | '\t') {
                t -= 1;
            }
            t
        };
        let mut seen_text = false;
        for (col, ch) in doc.line_slice(line).chars().enumerate() {
            if col as u32 >= row.end {
                break;
            }
            let w = char_width(ch, v, tab);
            let in_row = col as u32 >= row.start;
            if in_row && v_row0.is_none() {
                v_row0 = Some(v);
            }
            let is_bracket = matches!(ch, '(' | ')' | '[' | ']' | '{' | '}');
            let is_code = !noncode.iter().any(|&(a, b)| a as usize <= col && col < b as usize);
            let mut bracket_level = None;
            if is_bracket && is_code {
                if matches!(ch, ')' | ']' | '}') {
                    depth -= 1;
                }
                bracket_level = Some(depth);
                if matches!(ch, '(' | '[' | '{') {
                    depth += 1;
                }
            }
            if !ch.is_whitespace() {
                seen_text = true;
            }
            if in_row {
                let v0 = v_row0.unwrap_or(0);
                let x = ctx.x0 + (v - v0) as f32 * m.cell_w;
                if x > last_vis_x {
                    break;
                }
                let visible = x + w as f32 * m.cell_w >= ctx.clip.x;
                while span_i < spans.len() && spans[span_i].end as usize <= col {
                    span_i += 1;
                }
                let (mut color, font) = match spans.get(span_i).filter(|s| s.start as usize <= col) {
                    Some(s) => {
                        let f = Font {
                            weight: if s.style.bold { Weight::Bold } else { m.font.weight },
                            style: if s.style.italic { FontStyle::Italic } else { m.font.style },
                            ..m.font
                        };
                        (Color::from_rgb8(s.style.fg[0], s.style.fg[1], s.style.fg[2]), f)
                    }
                    None => (pal.fg, m.font),
                };
                if rainbow && let Some(level) = bracket_level {
                    color = pal.rainbow[level.rem_euclid(3) as usize];
                }
                if ch == ' ' || ch == '\t' {
                    flush(r, &mut run, run_v, v0, run_color, run_font);
                    if visible {
                        let show = match ws {
                            Whitespace::All => true,
                            Whitespace::Boundary => !seen_text || col >= trailing_from,
                            Whitespace::Selection => {
                                let pos = doc.line_start(line) + col;
                                self.tab.cursors.iter().any(|s| s.start() <= pos && pos < s.end())
                            }
                            Whitespace::None => false,
                        };
                        if show {
                            let cy = ctx.y + m.line_h / 2.0;
                            if ch == ' ' {
                                let cx = x + m.cell_w / 2.0;
                                rquad(r, Rectangle::new(Point::new(cx - 1.0, cy - 1.0), Size::new(2.0, 2.0)), pal.whitespace, 1.0);
                            } else {
                                let wpx = w as f32 * m.cell_w;
                                quad(r, Rectangle::new(Point::new(x + 2.0, cy - 0.5), Size::new(wpx - 4.0, 1.0)), pal.whitespace);
                                quad(r, Rectangle::new(Point::new(x + wpx - 4.0, cy - 2.0), Size::new(2.0, 4.0)), pal.whitespace);
                            }
                        }
                    }
                } else if ch.is_control() {
                    flush(r, &mut run, run_v, v0, run_color, run_font);
                } else if needs_own_cell(ch) && unicode_is_base(ch) {
                    flush(r, &mut run, run_v, v0, run_color, run_font);
                    if visible {
                        r.fill_text(text_primitive(ch.to_string(), m, font), Point::new(x, ctx.y), color, ctx.clip);
                    }
                } else if visible || !run.is_empty() {
                    if run.is_empty() {
                        run_v = v;
                        run_color = color;
                        run_font = font;
                    } else if run_color != color || run_font != font {
                        flush(r, &mut run, run_v, v0, run_color, run_font);
                        run_v = v;
                        run_color = color;
                        run_font = font;
                    }
                    run.push(ch);
                }
            }
            v += w;
        }
        flush(r, &mut run, run_v, v_row0.unwrap_or(0), run_color, run_font);
    }

    /// X of a char column in a row, relative to the row's origin.
    fn col_x(&self, row: tinta_core::display::Row, col: usize, tab: usize) -> f32 {
        let slice = self.entry.doc.line_slice(row.line as usize);
        let v0 = display::char_to_vcol(slice, row.start as usize, tab);
        let v = display::char_to_vcol(slice, col, tab);
        (v - v0) as f32 * self.metrics.cell_w
    }

    /// Fills the part of `[start, end)` (document offsets) on one row.
    #[allow(clippy::too_many_arguments)]
    fn range_rect(
        &self,
        row: tinta_core::display::Row,
        line_start: usize,
        start: usize,
        end: usize,
        x0: f32,
        y: f32,
        tab: usize,
        newline_tail: bool,
    ) -> Option<Rectangle> {
        let m = &self.metrics;
        let a = line_start + row.start as usize;
        let b = line_start + row.end as usize;
        let last_in_line = b == line_start + self.entry.doc.line_len(row.line as usize);
        let s = start.max(a);
        let e = end.min(b);
        if s > e || (s == e && !(newline_tail && end > e && last_in_line)) || end < a || start > b {
            return None;
        }
        let xs = self.col_x(row, s - line_start, tab);
        let mut xe = self.col_x(row, e - line_start, tab);
        // A selection running past the line's end shows the newline.
        if newline_tail && end > b && last_in_line {
            xe += m.cell_w * 0.6;
        }
        Some(Rectangle::new(Point::new(x0 + xs, y), Size::new((xe - xs).max(1.0), m.line_h)))
    }
}

fn unicode_is_base(c: char) -> bool {
    !matches!(c as u32, 0x300..=0x36F | 0x1AB0..=0x1AFF | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0x200B..=0x200F)
}

impl<'a, Message: Clone + 'a> EditorView<'a, Message> {
    pub fn update(&self, state: &mut State, event: &Event, bounds: Rectangle, cursor: mouse::Cursor, shell: &mut Shell<Message>) {
        // Tell the app the size whenever it changes; it needs it to scroll and wrap.
        let rep = (bounds.width, bounds.height, state.cell_w);
        if state.reported != Some(rep) && state.cell_w > 0.0 {
            state.reported = Some(rep);
            shell.publish((self.on_event)(EditorEvent::Viewport { w: rep.0, h: rep.1, cell_w: rep.2 }));
        }
        let g = self.geometry(bounds.width);
        let pos = cursor.position();
        let over = cursor.position_over(bounds);

        match event {
            Event::Window(window::Event::RedrawRequested(now)) => {
                if self.tab.caret_stamp != state.seen_stamp {
                    state.seen_stamp = self.tab.caret_stamp;
                    state.blink_on = true;
                    state.next_blink = *now + BLINK;
                }
                if self.focused && self.settings.cursor_blink {
                    if *now >= state.next_blink {
                        state.blink_on = !state.blink_on;
                        state.next_blink = *now + BLINK;
                    }
                    shell.request_redraw_at(state.next_blink);
                } else {
                    state.blink_on = true;
                }
                if let Some((at, row, vcol)) = state.hover_at
                    && !state.hover_sent
                {
                    if *now >= at + HOVER_DELAY {
                        state.hover_sent = true;
                        shell.publish((self.on_event)(EditorEvent::Hover { row, vcol }));
                    } else {
                        shell.request_redraw_at(at + HOVER_DELAY);
                    }
                }
                if self.focused {
                    let head = self.tab.cursors.head();
                    let (row, v) = self.tab.map.pos_to_row(&self.entry.doc, head);
                    let m = &self.metrics;
                    let x = bounds.x + g.text_x + v as f32 * m.cell_w - self.tab.scroll_x;
                    let y = bounds.y + row as f32 * m.line_h - self.tab.scroll_y;
                    shell.ime_cursor = Some(Rectangle::new(Point::new(x, y), Size::new(1.0, m.line_h)));
                }
                // Keep scrolling while a selection drag sits outside the text.
                if state.drag == DragKind::Text || state.drag == DragKind::Column {
                    let p = state.last_cursor;
                    let m = &self.metrics;
                    let mut dy = 0.0;
                    if p.y < bounds.y {
                        dy = -m.line_h;
                    } else if p.y > bounds.y + bounds.height {
                        dy = m.line_h;
                    }
                    let mut dx = 0.0;
                    if p.x < bounds.x + g.text_x {
                        dx = -m.cell_w * 2.0;
                    } else if p.x > bounds.x + g.text_x + g.text_w {
                        dx = m.cell_w * 2.0;
                    }
                    if dy != 0.0 || dx != 0.0 {
                        shell.publish((self.on_event)(EditorEvent::Scroll { dx, dy }));
                        let (row, vcol) = self.hit(p, bounds, &g);
                        shell.publish((self.on_event)(EditorEvent::Drag { row, vcol, mods: state.mods }));
                        shell.request_redraw_at(*now + Duration::from_millis(40));
                    }
                }
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(mods)) => {
                state.mods = *mods;
            }
            Event::InputMethod(ev) if self.focused => match ev {
                input_method::Event::Opened => state.ime_open = true,
                input_method::Event::Closed => state.ime_open = false,
                input_method::Event::Preedit(text, _) => {
                    shell.publish((self.on_event)(EditorEvent::Preedit(text.clone())));
                    shell.capture_event();
                }
                input_method::Event::Commit(text) => {
                    shell.publish((self.on_event)(EditorEvent::Ime(text.clone())));
                    shell.capture_event();
                }
            },
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let Some(p) = over else { return };
                let lx = p.x - bounds.x;
                let ly = p.y - bounds.y;
                state.hover_at = None;
                if state.hover_sent {
                    state.hover_sent = false;
                    shell.publish((self.on_event)(EditorEvent::HoverEnd));
                }
                let m = self.metrics;
                match button {
                    mouse::Button::Left | mouse::Button::Middle => {}
                    mouse::Button::Right if lx >= g.text_x && lx < g.minimap_x => {
                        let (row, vcol) = self.hit(p, bounds, &g);
                        shell.publish((self.on_event)(EditorEvent::SecondaryPress { row, vcol }));
                        return;
                    }
                    _ => return,
                }
                shell.capture_event();
                if *button == mouse::Button::Left {
                    if lx >= g.scrollbar_x {
                        let content = self.tab.map.row_count(&self.entry.doc) as f32 * m.line_h + bounds.height;
                        let thumb_h = (bounds.height / content * bounds.height).max(24.0);
                        let thumb_y = self.tab.scroll_y / content * bounds.height;
                        let grab = if ly >= thumb_y && ly <= thumb_y + thumb_h {
                            ly - thumb_y
                        } else {
                            thumb_h / 2.0
                        };
                        state.drag = DragKind::Scrollbar(grab);
                        let y = (ly - grab).max(0.0) / bounds.height * content;
                        shell.publish((self.on_event)(EditorEvent::ScrollTo { y }));
                        return;
                    }
                    if g.minimap_w > 0.0 && lx >= g.minimap_x {
                        state.drag = DragKind::Minimap;
                        let (first, _) = self.minimap_offset(bounds.height);
                        let row = first + ly / MM_LINE_H;
                        let y = (row * m.line_h - bounds.height / 2.0).max(0.0);
                        shell.publish((self.on_event)(EditorEvent::ScrollTo { y }));
                        return;
                    }
                }
                // Sticky header rows jump to their line.
                let sticky = self.sticky_lines();
                if !sticky.is_empty() && ly < sticky.len() as f32 * m.line_h {
                    let i = (ly / m.line_h) as usize;
                    if let Some(line) = sticky.get(i) {
                        shell.publish((self.on_event)(EditorEvent::StickyJump { line: *line }));
                    }
                    return;
                }
                let (row, vcol) = self.hit(p, bounds, &g);
                if lx < g.gutter_w {
                    if *button != mouse::Button::Left {
                        return;
                    }
                    if lx < MARK_W {
                        shell.publish((self.on_event)(EditorEvent::BreakpointToggle { row }));
                    } else if self.settings.folding && lx >= g.gutter_w - FOLD_W {
                        shell.publish((self.on_event)(EditorEvent::FoldToggle { row }));
                    } else {
                        state.drag = DragKind::Gutter;
                        shell.publish((self.on_event)(EditorEvent::GutterSelect { row, extend: state.mods.shift() }));
                    }
                    return;
                }
                let press = if *button == mouse::Button::Middle { PressButton::Middle } else { PressButton::Left };
                let clicks = match state.last_click {
                    Some((t, lp, n))
                        if t.elapsed() < Duration::from_millis(400)
                            && (lp.x - p.x).abs() < 4.0
                            && (lp.y - p.y).abs() < 4.0 =>
                    {
                        if n >= 3 { 1 } else { n + 1 }
                    }
                    _ => 1,
                };
                state.last_click = Some((Instant::now(), p, clicks));
                state.drag = if press == PressButton::Middle || (state.mods.alt() && state.mods.shift()) {
                    DragKind::Column
                } else {
                    DragKind::Text
                };
                shell.publish((self.on_event)(EditorEvent::Press { row, vcol, clicks, mods: state.mods, button: press }));
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                // The faint dot that says a click puts a breakpoint here.
                let in_marks = pos.is_some_and(|p| p.x - bounds.x < MARK_W);
                if in_marks || state.in_marks {
                    shell.request_redraw();
                }
                state.in_marks = in_marks;
                let Some(p) = pos else { return };
                state.last_cursor = p;
                let m = self.metrics;
                match state.drag {
                    DragKind::Text | DragKind::Column => {
                        let (row, vcol) = self.hit(p, bounds, &g);
                        let mods = if state.drag == DragKind::Column { state.mods | Modifiers::ALT | Modifiers::SHIFT } else { state.mods };
                        shell.publish((self.on_event)(EditorEvent::Drag { row, vcol, mods }));
                        if p.y < bounds.y || p.y > bounds.y + bounds.height || p.x < bounds.x + g.text_x || p.x > bounds.x + g.text_x + g.text_w {
                            shell.request_redraw();
                        }
                        shell.capture_event();
                    }
                    DragKind::Gutter => {
                        let (row, _) = self.hit(p, bounds, &g);
                        shell.publish((self.on_event)(EditorEvent::GutterDrag { row }));
                        shell.capture_event();
                    }
                    DragKind::Scrollbar(grab) => {
                        let content = self.tab.map.row_count(&self.entry.doc) as f32 * m.line_h + bounds.height;
                        let y = ((p.y - bounds.y - grab).max(0.0)) / bounds.height * content;
                        shell.publish((self.on_event)(EditorEvent::ScrollTo { y }));
                        shell.capture_event();
                    }
                    DragKind::Minimap => {
                        let rows = self.tab.map.row_count(&self.entry.doc) as f32;
                        let t = ((p.y - bounds.y) / bounds.height).clamp(0.0, 1.0);
                        let y = (t * rows * m.line_h - bounds.height / 2.0).max(0.0);
                        shell.publish((self.on_event)(EditorEvent::ScrollTo { y }));
                        shell.capture_event();
                    }
                    DragKind::None => {
                        // Hover timing: restart whenever the cell under the pointer changes.
                        match over {
                            Some(p) if p.x - bounds.x >= g.text_x && p.x - bounds.x < g.minimap_x => {
                                let (row, vcol) = self.hit(p, bounds, &g);
                                let same = state.hover_at.is_some_and(|(_, r, v)| r == row && v == vcol);
                                if !same {
                                    state.hover_at = Some((Instant::now(), row, vcol));
                                    if state.hover_sent {
                                        state.hover_sent = false;
                                        shell.publish((self.on_event)(EditorEvent::HoverEnd));
                                    }
                                    shell.request_redraw_at(Instant::now() + HOVER_DELAY);
                                }
                            }
                            _ => {
                                state.hover_at = None;
                                if state.hover_sent {
                                    state.hover_sent = false;
                                    shell.publish((self.on_event)(EditorEvent::HoverEnd));
                                }
                            }
                        }
                    }
                }
            }
            // During a real drag the pointer is grabbed, and never enters or leaves. If it does,
            // the release went elsewhere (a popup closing took it), and the drag is over.
            Event::Mouse(mouse::Event::CursorEntered | mouse::Event::CursorLeft) => {
                if state.drag != DragKind::None {
                    state.drag = DragKind::None;
                    shell.publish((self.on_event)(EditorEvent::Release));
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(_)) => {
                if state.drag != DragKind::None {
                    state.drag = DragKind::None;
                    shell.publish((self.on_event)(EditorEvent::Release));
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if over.is_none() {
                    return;
                }
                let m = &self.metrics;
                let speed = self.settings.scroll_speed;
                let (mut dx, mut dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (-x * m.cell_w * speed * 2.0, -y * m.line_h * speed),
                    mouse::ScrollDelta::Pixels { x, y } => (-x, -y),
                };
                if state.mods.control() {
                    let d = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => *y,
                        mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                    };
                    shell.publish((self.on_event)(EditorEvent::ZoomWheel { delta: d }));
                    shell.capture_event();
                    return;
                }
                if state.mods.shift() {
                    std::mem::swap(&mut dx, &mut dy);
                }
                shell.publish((self.on_event)(EditorEvent::Scroll { dx, dy }));
                shell.capture_event();
            }
            _ => {}
        }
    }

    pub fn mouse_interaction(&self, state: &State, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        if matches!(state.drag, DragKind::Text | DragKind::Column) {
            return mouse::Interaction::Text;
        }
        let Some(p) = cursor.position_over(bounds) else { return mouse::Interaction::default() };
        let g = self.geometry(bounds.width);
        let lx = p.x - bounds.x;
        if lx < g.gutter_w {
            mouse::Interaction::Pointer
        } else if lx < g.minimap_x {
            if state.mods.control() { mouse::Interaction::Pointer } else { mouse::Interaction::Text }
        } else {
            mouse::Interaction::default()
        }
    }

    pub fn draw(&self, state: &State, r: &mut Renderer, pal: &Palette, bounds: Rectangle, cursor: mouse::Cursor) {
        let hover = cursor.position_over(bounds);
        let pal = *pal;
        let m = self.metrics;
        let m = Metrics { cell_w: if state.cell_w > 0.0 { state.cell_w } else { m.cell_w }, ..m };
        let view = EditorView {
            entry: self.entry,
            tab: self.tab,
            metrics: m,
            settings: self.settings,
            focused: self.focused,
            current_match: self.current_match,
            inline_blame: self.inline_blame,
            breakpoints: self.breakpoints,
            exec_line: self.exec_line,
            cursor_style: self.cursor_style,
            on_event: Box::new(|_| unreachable!("drawing publishes nothing")),
        };
        view.paint(state, r, bounds, &pal, hover);
    }
}

impl<Message: Clone> EditorView<'_, Message> {
    fn paint(&self, state: &State, r: &mut Renderer, bounds: Rectangle, pal: &Palette, hover: Option<Point>) {
        let m = self.metrics;
        let doc = &self.entry.doc;
        let tabw = doc.indent.width.max(1);
        let g = self.geometry(bounds.width);
        let tab = self.tab;
        let s = self.settings;
        let rows_total = tab.map.row_count(doc);
        let first_row = (tab.scroll_y / m.line_h).floor() as usize;
        let y_off = -(tab.scroll_y - first_row as f32 * m.line_h);
        let n_rows = (bounds.height / m.line_h).ceil() as usize + 1;
        let last_row = (first_row + n_rows).min(rows_total);

        let text_rect = Rectangle::new(
            Point::new(bounds.x + g.gutter_w, bounds.y),
            Size::new(g.minimap_x - g.gutter_w, bounds.height),
        );
        let x_text = bounds.x + g.text_x - tab.scroll_x;
        let y_at = |row: usize| bounds.y + y_off + (row - first_row) as f32 * m.line_h;

        // Text area ------------------------------------------------------------------------
        r.with_layer(text_rect, |r| {
            let primary_line = doc.char_to_line(tab.cursors.head());
            for row_i in first_row..last_row {
                let row = tab.map.row(doc, row_i);
                let line = row.line as usize;
                let y = y_at(row_i);
                let lstart = doc.line_start(line);
                // Where the debugger stopped.
                if let Some((l, top)) = self.exec_line
                    && l == line
                {
                    let c = if top { pal.warning } else { pal.accent };
                    quad(r, Rectangle::new(Point::new(text_rect.x, y), Size::new(text_rect.width, m.line_h)), alpha(c, 0.2));
                }
                // Current line.
                if s.highlight_current_line && tab.cursors.iter().any(|c| c.is_empty() && doc.char_to_line(c.head) == line) {
                    quad(r, Rectangle::new(Point::new(text_rect.x, y), Size::new(text_rect.width, m.line_h)), pal.current_line);
                }
                let pad_x = row.pad as f32 * m.cell_w;
                let x0 = x_text + pad_x;
                // Indent guides.
                if s.indent_guides && row.start == 0 {
                    let indent = guide_indent(doc, line, tabw);
                    let mut v = doc.indent.width.max(1);
                    let mut i = 1;
                    while v < indent {
                        let gx = x_text + v as f32 * m.cell_w;
                        quad(r, Rectangle::new(Point::new(gx, y), Size::new(1.0, m.line_h)), pal.guide);
                        v = doc.indent.width.max(1) * { i += 1; i };
                    }
                }
                // Find matches.
                let row_a = lstart + row.start as usize;
                let row_b = lstart + row.end as usize;
                let matches = &self.entry.matches;
                let i0 = matches.partition_point(|mm| mm.1 <= row_a);
                for (i, mm) in matches.iter().enumerate().skip(i0) {
                    if mm.0 > row_b {
                        break;
                    }
                    if let Some(rc) = self.range_rect(row, lstart, mm.0, mm.1, x0, y, tabw, false) {
                        let c = if self.current_match == Some(i) { pal.find_current } else { pal.find };
                        rquad(r, rc, c, 2.0);
                    }
                }
                // Occurrences of the word or selection under the cursor.
                if s.occurrences_highlight
                    && let Some((word, whole)) = &tab.occurrence
                {
                    draw_occurrences(self, r, row, lstart, word, *whole, x0, y, tabw, pal);
                }
                // Selections.
                for sel in tab.cursors.iter() {
                    if sel.is_empty() || sel.end() < row_a || sel.start() > row_b {
                        continue;
                    }
                    if let Some(rc) = self.range_rect(row, lstart, sel.start(), sel.end(), x0, y, tabw, true) {
                        let c = if self.focused { pal.selection } else { pal.selection_dim };
                        rquad(r, rc, c, 3.0);
                    }
                }
                // Matching bracket boxes.
                if let Some((a, b)) = tab.bracket_pair {
                    for p in [a, b] {
                        if p >= row_a && p < row_b.max(row_a + 1) && p < lstart + doc.line_len(line) {
                            let x = x0 + self.col_x(row, p - lstart, tabw);
                            let rect = Rectangle::new(Point::new(x, y), Size::new(m.cell_w, m.line_h));
                            r.fill_quad(
                                Quad {
                                    bounds: rect,
                                    border: Border { color: alpha(pal.fg, 0.55), width: 1.0, radius: 2.0 },
                                    ..Quad::default()
                                },
                                alpha(pal.accent, 0.18),
                            );
                        }
                    }
                }
                let clip = text_rect;
                self.draw_row_text(r, row, RowCtx { x0, y, clip }, pal, tabw);

                // Fold marker at the end of a folded header.
                if tab.folds.is_folded(line) && row_b == lstart + doc.line_len(line) {
                    let x = x0 + self.col_x(row, row.end as usize, tabw) + m.cell_w;
                    let rc = Rectangle::new(Point::new(x, y + 2.0), Size::new(m.cell_w * 3.0, m.line_h - 4.0));
                    rquad(r, rc, alpha(pal.accent, 0.25), 3.0);
                    r.fill_text(text_primitive("⋯".into(), &m, m.font), Point::new(x + m.cell_w, y), pal.fg, clip);
                }
                // Diagnostics: squiggles, and the message at the end of the line.
                draw_diagnostics(self, r, row, lstart, row_a, row_b, x0, y, tabw, pal, clip);
                // Inline blame after the primary line.
                if line == primary_line
                    && row_b == lstart + doc.line_len(line)
                    && let Some(b) = self.inline_blame
                {
                    let x = x0 + self.col_x(row, row.end as usize, tabw) + m.cell_w * 3.0;
                    r.fill_text(text_primitive(b.to_string(), &m, m.font), Point::new(x, y), alpha(pal.fg, 0.4), clip);
                }
            }
            // Rulers.
            for col in &s.rulers {
                let x = x_text + *col as f32 * m.cell_w;
                quad(r, Rectangle::new(Point::new(x, bounds.y), Size::new(1.0, bounds.height)), alpha(pal.fg, 0.08));
            }
            // Preedit while composing with an input method.
            if !tab.preedit.is_empty() {
                let (row, v) = tab.map.pos_to_row(doc, tab.cursors.head());
                if row >= first_row && row < last_row {
                    let x = x_text + v as f32 * m.cell_w;
                    let y = y_at(row);
                    let n = unicode_width_str(&tab.preedit) as f32;
                    quad(r, Rectangle::new(Point::new(x, y), Size::new(n * m.cell_w, m.line_h)), pal.bg);
                    r.fill_text(text_primitive(tab.preedit.clone(), &m, m.font), Point::new(x, y), pal.accent, text_rect);
                    quad(r, Rectangle::new(Point::new(x, y + m.line_h - 2.0), Size::new(n * m.cell_w, 1.0)), pal.accent);
                }
            }
        });
        // Cursors go in their own layer so they sit on top of the text.
        if state.blink_on || !self.focused || !s.cursor_blink {
            r.with_layer(text_rect, |r| {
                for sel in tab.cursors.iter() {
                    let (row, v) = tab.map.pos_to_row(doc, sel.head);
                    if row < first_row || row >= last_row {
                        continue;
                    }
                    let rr = tab.map.row(doc, row);
                    let x = x_text + (v + rr.pad as usize) as f32 * m.cell_w;
                    let y = y_at(row);
                    let color = if self.focused { pal.accent } else { alpha(pal.fg, 0.35) };
                    let rect = match self.cursor_style {
                        CursorStyle::Bar => Rectangle::new(Point::new(x - 1.0, y), Size::new(2.0, m.line_h)),
                        CursorStyle::Block => Rectangle::new(Point::new(x, y), Size::new(m.cell_w, m.line_h)),
                        CursorStyle::Underline => Rectangle::new(Point::new(x, y + m.line_h - 2.0), Size::new(m.cell_w, 2.0)),
                    };
                    let color = if self.cursor_style == CursorStyle::Block { alpha(color, 0.55) } else { color };
                    quad(r, rect, color);
                }
            });
        }

        // Gutter ---------------------------------------------------------------------------
        let gutter = Rectangle::new(bounds.position(), Size::new(g.gutter_w, bounds.height));
        r.with_layer(gutter, |r| {
            let primary_line = doc.char_to_line(tab.cursors.head());
            let selected: Vec<usize> = tab.cursors.iter().map(|c| doc.char_to_line(c.head)).collect();
            for row_i in first_row..last_row {
                let row = tab.map.row(doc, row_i);
                let y = y_at(row_i);
                self.draw_gutter_row(r, row, y, bounds, &g, pal, primary_line, &selected, tabw, hover);
            }
        });

        // Sticky scroll ---------------------------------------------------------------------
        let sticky = self.sticky_lines();
        if !sticky.is_empty() {
            let h = sticky.len() as f32 * m.line_h;
            let rect = Rectangle::new(bounds.position(), Size::new(g.minimap_x, h + 3.0));
            r.with_layer(rect, |r| {
                quad(r, Rectangle::new(bounds.position(), Size::new(g.minimap_x, h)), pal.bg);
                quad(r, Rectangle::new(Point::new(bounds.x, bounds.y + h), Size::new(g.minimap_x, 1.0)), alpha(pal.fg, 0.14));
                quad(r, Rectangle::new(Point::new(bounds.x, bounds.y + h + 1.0), Size::new(g.minimap_x, 2.0)), Color { a: 0.08, ..Color::BLACK });
            });
            r.with_layer(rect, |r| {
                let clip = Rectangle::new(Point::new(bounds.x + g.gutter_w, bounds.y), Size::new(g.minimap_x - g.gutter_w, h));
                for (i, line) in sticky.iter().enumerate() {
                    let y = bounds.y + i as f32 * m.line_h;
                    let row = tinta_core::display::Row { line: *line as u32, start: 0, end: doc.line_len(*line) as u32, pad: 0 };
                    self.draw_row_text(r, row, RowCtx { x0: x_text, y, clip }, pal, tabw);
                    if g.digits_w > 0.0 {
                        let label = (line + 1).to_string();
                        let x = bounds.x + MARK_W + g.digits_w - label.len() as f32 * m.cell_w;
                        r.fill_text(text_primitive(label, &m, m.font), Point::new(x, y), pal.gutter, gutter);
                    }
                }
            });
        }

        // Minimap ---------------------------------------------------------------------------
        if g.minimap_w > 0.0 {
            self.draw_minimap(r, bounds, &g, pal);
        }

        // Scrollbar with overview marks -------------------------------------------------------
        self.draw_scrollbar(state, r, bounds, &g, pal);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_gutter_row(
        &self,
        r: &mut Renderer,
        row: tinta_core::display::Row,
        y: f32,
        bounds: Rectangle,
        g: &Geometry,
        pal: &Palette,
        primary_line: usize,
        selected: &[usize],
        tabw: usize,
        hover: Option<Point>,
    ) {
        let m = self.metrics;
        let doc = &self.entry.doc;
        let s = self.settings;
        let line = row.line as usize;
        let gutter = Rectangle::new(bounds.position(), Size::new(g.gutter_w, bounds.height));
        if row.start == 0 && g.digits_w > 0.0 {
            let number = match s.line_numbers {
                LineNumbers::Relative if line != primary_line => line.abs_diff(primary_line),
                _ => line + 1,
            };
            let label = number.to_string();
            let x = bounds.x + MARK_W + g.digits_w - label.len() as f32 * m.cell_w;
            let color = match self.entry.diag_severity_at_line(line) {
                Some(Severity::Error) => pal.error,
                Some(Severity::Warning) => pal.warning,
                _ if selected.contains(&line) => pal.gutter_active,
                _ => pal.gutter,
            };
            r.fill_text(text_primitive(label, &m, m.font), Point::new(x, y), color, gutter);
        }
        // Git marks.
        if s.git_gutter && row.start == 0 {
            match self.entry.git_diff.mark_at(line) {
                Some(LineMark::Added) => quad(r, Rectangle::new(Point::new(bounds.x + 2.0, y), Size::new(3.0, m.line_h)), pal.added),
                Some(LineMark::Modified) => quad(r, Rectangle::new(Point::new(bounds.x + 2.0, y), Size::new(3.0, m.line_h)), pal.modified),
                Some(LineMark::Deleted) => quad(r, Rectangle::new(Point::new(bounds.x + 1.0, y - 2.0), Size::new(6.0, 4.0)), pal.deleted),
                None => {}
            }
        }
        if row.start == 0 && self.entry.bookmarks.contains(&line) {
            rquad(r, Rectangle::new(Point::new(bounds.x + 10.0, y + m.line_h / 2.0 - 4.0), Size::new(8.0, 8.0)), pal.accent, 4.0);
        }
        if row.start == 0 {
            let dot = Rectangle::new(Point::new(bounds.x + 8.0, y + m.line_h / 2.0 - 6.0), Size::new(12.0, 12.0));
            let hole = Rectangle::new(Point::new(dot.x + 3.0, dot.y + 3.0), Size::new(6.0, 6.0));
            if let Some(b) = self.breakpoints.iter().find(|b| b.line == line) {
                rquad(r, dot, if b.enabled { pal.error } else { pal.gutter }, 6.0);
                if !b.enabled || b.state == tinta_core::dap::BpState::Pending {
                    // Hollow: it won't stop anything right now.
                    rquad(r, hole, pal.bg, 3.0);
                } else if b.condition.is_some() {
                    // A little `=`, as a condition's mark.
                    quad(r, Rectangle::new(Point::new(dot.x + 3.0, dot.y + 3.5), Size::new(6.0, 1.5)), pal.bg);
                    quad(r, Rectangle::new(Point::new(dot.x + 3.0, dot.y + 7.0), Size::new(6.0, 1.5)), pal.bg);
                }
            } else if hover.is_some_and(|p| p.x - bounds.x < MARK_W && p.y >= y && p.y < y + m.line_h) {
                rquad(r, dot, alpha(pal.error, 0.35), 6.0);
            }
            // The arrow for where execution is, drawn over a breakpoint on the same line.
            if let Some((l, top)) = self.exec_line
                && l == line
            {
                let c = if top { pal.warning } else { pal.accent };
                let cy = y + m.line_h / 2.0;
                for i in 0..6 {
                    let f = i as f32;
                    let h = 11.0 - f * 2.0;
                    quad(r, Rectangle::new(Point::new(bounds.x + 10.0 + f * 1.4, cy - h / 2.0), Size::new(1.5, h)), c);
                }
            }
        }
        // Fold arrows, drawn as small triangles out of quads so no font is needed.
        if s.folding && row.start == 0 {
            let folded = self.tab.folds.is_folded(line);
            if folded || is_foldable(doc, line, tabw) {
                let cx = bounds.x + g.gutter_w - FOLD_W / 2.0;
                let cy = y + m.line_h / 2.0;
                let c = if folded { pal.accent } else { alpha(pal.fg, 0.32) };
                for i in 0..4 {
                    let f = i as f32;
                    if folded {
                        // Pointing right.
                        let h = 7.0 - f * 2.0;
                        quad(r, Rectangle::new(Point::new(cx - 2.0 + f, cy - h / 2.0), Size::new(1.0, h)), c);
                    } else {
                        // Pointing down.
                        let w = 7.0 - f * 2.0;
                        quad(r, Rectangle::new(Point::new(cx - w / 2.0, cy - 2.0 + f), Size::new(w, 1.0)), c);
                    }
                }
            }
        }
    }

    fn draw_minimap(&self, r: &mut Renderer, bounds: Rectangle, g: &Geometry, pal: &Palette) {
        let m = self.metrics;
        let doc = &self.entry.doc;
        let tab = self.tab;
        let rect = Rectangle::new(Point::new(bounds.x + g.minimap_x, bounds.y), Size::new(g.minimap_w, bounds.height));
        r.with_layer(rect, |r| {
            quad(r, Rectangle::new(rect.position(), Size::new(1.0, rect.height)), alpha(pal.fg, 0.07));
            let (first_f, _) = self.minimap_offset(bounds.height);
            let first = first_f as usize;
            let frac = first_f - first as f32;
            let count = (bounds.height / MM_LINE_H) as usize + 2;
            let total = tab.map.row_count(doc);
            let view_rows = bounds.height / m.line_h;
            // Viewport slider.
            let top = (tab.scroll_y / m.line_h - first_f) * MM_LINE_H;
            quad(
                r,
                Rectangle::new(Point::new(rect.x, rect.y + top), Size::new(rect.width, view_rows * MM_LINE_H)),
                alpha(pal.fg, 0.08),
            );
            let hl = doc.highlighter.as_ref();
            for i in 0..count.min(total.saturating_sub(first)) {
                let row = tab.map.row(doc, first + i);
                let line = row.line as usize;
                let y = rect.y + (i as f32 - frac) * MM_LINE_H;
                let x0 = rect.x + 6.0;
                let max_w = rect.width - 8.0;
                let width_cols = (max_w / MM_CHAR_W) as u32;
                let start = row.start;
                let lstart = doc.line_start(line);
                // Selection and matches as bands.
                if self.entry.matches.binary_search_by(|mm| {
                    let (a, b) = (doc.line_start(line), doc.line_end(line));
                    if mm.1 < a { std::cmp::Ordering::Less } else if mm.0 > b { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Equal }
                }).is_ok() {
                    quad(r, Rectangle::new(Point::new(rect.x, y), Size::new(rect.width, MM_LINE_H)), pal.find);
                }
                if tab.cursors.iter().any(|c| c.start() <= lstart + row.end as usize && c.end() >= lstart + row.start as usize && !c.is_empty()) {
                    quad(r, Rectangle::new(Point::new(rect.x, y), Size::new(rect.width, MM_LINE_H)), pal.selection);
                }
                let spans = hl.and_then(|h| h.line(line)).map(|l| &l.spans[..]).unwrap_or(&[]);
                let slice = doc.line_slice(line);
                if spans.is_empty() {
                    let n = (row.end - row.start).min(width_cols) as f32;
                    let ws = slice.chars().skip(start as usize).take_while(|c| c.is_whitespace()).count() as f32;
                    if n > ws {
                        quad(r, Rectangle::new(Point::new(x0 + ws * MM_CHAR_W, y), Size::new((n - ws) * MM_CHAR_W, MM_LINE_H - 1.0)), alpha(pal.fg, 0.4));
                    }
                } else {
                    for sp in spans {
                        if sp.end <= start || sp.start >= row.end {
                            continue;
                        }
                        let a = sp.start.max(start) - start;
                        let b = (sp.end.min(row.end) - start).min(width_cols);
                        if a >= b {
                            continue;
                        }
                        // Skip whitespace at the start of a span: indentation shows as a gap.
                        let ws = slice.chars().skip((start + a) as usize).take((b - a) as usize).take_while(|c| c.is_whitespace()).count() as u32;
                        if a + ws >= b {
                            continue;
                        }
                        let c = Color::from_rgba8(sp.style.fg[0], sp.style.fg[1], sp.style.fg[2], 0.75);
                        quad(
                            r,
                            Rectangle::new(
                                Point::new(x0 + (a + ws) as f32 * MM_CHAR_W, y),
                                Size::new((b - a - ws) as f32 * MM_CHAR_W, MM_LINE_H - 1.0),
                            ),
                            c,
                        );
                    }
                }
            }
        });
    }

    fn draw_scrollbar(&self, _state: &State, r: &mut Renderer, bounds: Rectangle, g: &Geometry, pal: &Palette) {
        let m = self.metrics;
        let doc = &self.entry.doc;
        let tab = self.tab;
        let rows = tab.map.row_count(doc) as f32;
        let rect = Rectangle::new(Point::new(bounds.x + g.scrollbar_x, bounds.y), Size::new(SCROLLBAR_W, bounds.height));
        r.with_layer(rect, |r| {
            let content = rows * m.line_h + bounds.height;
            let thumb_h = (bounds.height / content * bounds.height).max(24.0);
            let thumb_y = tab.scroll_y / content * bounds.height;
            if rows * m.line_h > bounds.height {
                rquad(
                    r,
                    Rectangle::new(Point::new(rect.x + 3.0, rect.y + thumb_y), Size::new(SCROLLBAR_W - 6.0, thumb_h)),
                    alpha(pal.fg, 0.22),
                    3.0,
                );
            }
            // Overview ruler: where the matches, problems and changes are.
            let mark_y = |pos_line: usize| {
                let row = tab.map.line_to_row(pos_line) as f32;
                rect.y + (row / rows.max(1.0)) * bounds.height
            };
            for mm in self.entry.matches.iter().take(4000) {
                let y = mark_y(doc.char_to_line(mm.0));
                quad(r, Rectangle::new(Point::new(rect.x + 2.0, y), Size::new(4.0, 2.0)), pal.find_current);
            }
            for d in &self.entry.diags {
                let y = mark_y(doc.char_to_line(d.start));
                quad(r, Rectangle::new(Point::new(rect.x + 6.0, y), Size::new(5.0, 3.0)), pal.severity(d.severity));
            }
            if self.settings.git_gutter {
                for (line, mark) in self.entry.git_diff.marks.iter().take(4000) {
                    let c = match mark {
                        LineMark::Added => pal.added,
                        LineMark::Modified => pal.modified,
                        LineMark::Deleted => pal.deleted,
                    };
                    quad(r, Rectangle::new(Point::new(rect.x, mark_y(*line as usize)), Size::new(2.0, 2.0)), c);
                }
            }
            // Where the cursor is.
            let cy = mark_y(doc.char_to_line(tab.cursors.head()));
            quad(r, Rectangle::new(Point::new(rect.x, cy), Size::new(SCROLLBAR_W, 1.0)), alpha(pal.fg, 0.5));
        });
    }
}

fn unicode_width_str(s: &str) -> usize {
    s.chars().map(|c| char_width(c, 0, 4)).sum()
}

/// A blank line takes its guides from whatever comes next.
fn guide_indent(doc: &tinta_core::Document, line: usize, tab: usize) -> usize {
    if !doc.line_is_blank(line) {
        return display::line_indent_vcols(doc, line, tab);
    }
    let mut below = 0;
    for l in line + 1..(line + 40).min(doc.len_lines()) {
        if !doc.line_is_blank(l) {
            below = display::line_indent_vcols(doc, l, tab);
            break;
        }
    }
    let mut above = 0;
    for l in (line.saturating_sub(40)..line).rev() {
        if !doc.line_is_blank(l) {
            above = display::line_indent_vcols(doc, l, tab);
            break;
        }
    }
    above.min(below)
}

#[allow(clippy::too_many_arguments)]
fn draw_occurrences<Message: Clone>(
    v: &EditorView<'_, Message>,
    r: &mut Renderer,
    row: tinta_core::display::Row,
    lstart: usize,
    word: &str,
    whole: bool,
    x0: f32,
    y: f32,
    tabw: usize,
    pal: &Palette,
) {
    if word.is_empty() {
        return;
    }
    let doc = &v.entry.doc;
    let line = row.line as usize;
    let text = doc.line_text(line);
    let n = word.chars().count();
    for (byte, _) in text.match_indices(word) {
        let col = text[..byte].chars().count();
        if whole {
            let before = text[..byte].chars().next_back();
            let after = text[byte + word.len()..].chars().next();
            let is_word = |c: char| c.is_alphanumeric() || c == '_';
            if before.is_some_and(is_word) || after.is_some_and(is_word) {
                continue;
            }
        }
        let (a, b) = (lstart + col, lstart + col + n);
        if let Some(rc) = v.range_rect(row, lstart, a, b, x0, y, tabw, false) {
            rquad(r, rc, pal.occurrence, 2.0);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_diagnostics<Message: Clone>(
    v: &EditorView<'_, Message>,
    r: &mut Renderer,
    row: tinta_core::display::Row,
    lstart: usize,
    row_a: usize,
    row_b: usize,
    x0: f32,
    y: f32,
    tabw: usize,
    pal: &Palette,
    clip: Rectangle,
) {
    let m = v.metrics;
    let doc = &v.entry.doc;
    let line = row.line as usize;
    let mut message: Option<(&str, Color)> = None;
    for d in &v.entry.diags {
        if d.end < row_a || d.start > row_b {
            continue;
        }
        let color = pal.severity(d.severity);
        let (s, e) = (d.start, d.end.max(d.start + 1));
        if let Some(rc) = v.range_rect(row, lstart, s, e.min(lstart + doc.line_len(line).max(1)), x0, y, tabw, false) {
            // Squiggle.
            let base = rc.y + m.line_h - 3.0;
            let mut x = rc.x;
            let mut i = 0;
            while x < rc.x + rc.width.max(m.cell_w) {
                let dy = [0.0, 1.0, 2.0, 1.0][i % 4];
                quad(r, Rectangle::new(Point::new(x, base + dy), Size::new(2.0, 1.0)), color);
                x += 2.0;
                i += 1;
            }
        }
        let starts_here = d.start >= row_a && d.start <= row_b;
        if starts_here && (message.is_none() || d.severity == Severity::Error) {
            message = Some((d.message.as_str(), color));
        }
    }
    if let Some((text, color)) = message
        && row_b == lstart + doc.line_len(line)
    {
        let first = text.lines().next().unwrap_or("");
        let x = x0 + v.col_x(row, row.end as usize, tabw) + m.cell_w * 3.0;
        let width = first.chars().count() as f32 * m.cell_w + m.cell_w;
        let rc = Rectangle::new(Point::new(x - m.cell_w / 2.0, y), Size::new(width, m.line_h));
        rquad(r, rc, alpha(color, 0.12), 3.0);
        r.fill_text(text_primitive(first.to_string(), &m, m.font), Point::new(x, y), alpha(color, 0.95), clip);
    }
}

