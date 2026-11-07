//! The few drawing and input types the editor's widgets are written against, drawn with GSK and
//! Pango. They keep the shape of what the widgets used before (points, quads, text runs, layers,
//! mouse events), so the code that decides what goes where on screen didn't have to change.

use gtk::prelude::*;
use gtk::{gdk, graphene, gsk, pango};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

// Geometry ------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ORIGIN: Point = Point { x: 0.0, y: 0.0 };
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rectangle {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rectangle {
    pub fn new(p: Point, s: Size) -> Self {
        Self { x: p.x, y: p.y, width: s.width, height: s.height }
    }
    pub fn with_size(s: Size) -> Self {
        Self { x: 0.0, y: 0.0, width: s.width, height: s.height }
    }
    pub fn position(&self) -> Point {
        Point::new(self.x, self.y)
    }
    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.x + self.width && p.y >= self.y && p.y < self.y + self.height
    }
    fn graphene(&self) -> graphene::Rect {
        graphene::Rect::new(self.x, self.y, self.width.max(0.0), self.height.max(0.0))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const BLACK: Color = Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub fn from_rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }
    pub fn from_rgb8(r: u8, g: u8, b: u8) -> Self {
        Self::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
    }
    pub fn from_rgba8(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { a, ..Self::from_rgb8(r, g, b) }
    }
    pub fn rgba(&self) -> gdk::RGBA {
        gdk::RGBA::new(self.r, self.g, self.b, self.a)
    }
}

impl From<gdk::RGBA> for Color {
    fn from(c: gdk::RGBA) -> Self {
        Self { r: c.red(), g: c.green(), b: c.blue(), a: c.alpha() }
    }
}

// Fonts ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Weight {
    #[default]
    Normal,
    Semibold,
    Bold,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Font {
    pub family: &'static str,
    pub weight: Weight,
    pub style: FontStyle,
}

impl Font {
    pub const MONOSPACE: Font = Font { family: "Monospace", weight: Weight::Normal, style: FontStyle::Normal };
    pub fn with_name(family: &'static str) -> Self {
        Self { family, weight: Weight::Normal, style: FontStyle::Normal }
    }

    pub fn description(&self, size: f32) -> pango::FontDescription {
        let mut d = pango::FontDescription::new();
        d.set_family(self.family);
        d.set_weight(match self.weight {
            Weight::Normal => pango::Weight::Normal,
            Weight::Semibold => pango::Weight::Semibold,
            Weight::Bold => pango::Weight::Bold,
        });
        d.set_style(if self.style == FontStyle::Italic { pango::Style::Italic } else { pango::Style::Normal });
        d.set_absolute_size(size as f64 * pango::SCALE as f64);
        d
    }
}

// Input ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Modifiers(u8);

impl Modifiers {
    pub const SHIFT: Modifiers = Modifiers(1);
    pub const CTRL: Modifiers = Modifiers(2);
    pub const ALT: Modifiers = Modifiers(4);
    pub const LOGO: Modifiers = Modifiers(8);
    pub const fn empty() -> Self {
        Modifiers(0)
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    pub fn shift(self) -> bool {
        self.0 & 1 != 0
    }
    pub fn control(self) -> bool {
        self.0 & 2 != 0
    }
    pub fn command(self) -> bool {
        self.control()
    }
    pub fn alt(self) -> bool {
        self.0 & 4 != 0
    }
    pub fn logo(self) -> bool {
        self.0 & 8 != 0
    }
    pub fn from_gdk(s: gdk::ModifierType) -> Self {
        let mut m = 0;
        if s.contains(gdk::ModifierType::SHIFT_MASK) {
            m |= 1;
        }
        if s.contains(gdk::ModifierType::CONTROL_MASK) {
            m |= 2;
        }
        if s.contains(gdk::ModifierType::ALT_MASK) {
            m |= 4;
        }
        if s.contains(gdk::ModifierType::SUPER_MASK) || s.contains(gdk::ModifierType::META_MASK) {
            m |= 8;
        }
        Modifiers(m)
    }
}

impl Modifiers {
    pub fn remove(&mut self, other: Modifiers) {
        self.0 &= !other.0;
    }
    pub fn insert(&mut self, other: Modifiers) {
        self.0 |= other.0;
    }
}

impl std::ops::BitOrAssign for Modifiers {
    fn bitor_assign(&mut self, o: Modifiers) {
        self.0 |= o.0;
    }
}

impl std::ops::BitOr for Modifiers {
    type Output = Modifiers;
    fn bitor(self, o: Modifiers) -> Modifiers {
        Modifiers(self.0 | o.0)
    }
}

pub mod mouse {
    use super::{Point, Rectangle};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Button {
        Left,
        Middle,
        Right,
        Other,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum ScrollDelta {
        Lines { x: f32, y: f32 },
        Pixels { x: f32, y: f32 },
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Event {
        ButtonPressed(Button),
        ButtonReleased(Button),
        CursorMoved { position: Point },
        CursorEntered,
        CursorLeft,
        WheelScrolled { delta: ScrollDelta },
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Cursor {
        Available(Point),
        Unavailable,
    }

    impl Cursor {
        pub fn position(&self) -> Option<Point> {
            match self {
                Cursor::Available(p) => Some(*p),
                Cursor::Unavailable => None,
            }
        }
        pub fn position_over(&self, bounds: Rectangle) -> Option<Point> {
            self.position().filter(|p| bounds.contains(*p))
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum Interaction {
        #[default]
        Idle,
        Pointer,
        Text,
    }

    impl Interaction {
        pub fn css_name(self) -> &'static str {
            match self {
                Interaction::Idle => "default",
                Interaction::Pointer => "pointer",
                Interaction::Text => "text",
            }
        }
    }
}

pub mod window {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Event {
        RedrawRequested(std::time::Instant),
    }
}

pub mod input_method {
    #[derive(Debug, Clone, PartialEq)]
    pub enum Event {
        Opened,
        Closed,
        Preedit(String, Option<std::ops::Range<usize>>),
        Commit(String),
    }
}

pub mod keyboard {
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Event {
        ModifiersChanged(super::Modifiers),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Mouse(mouse::Event),
    Window(window::Event),
    InputMethod(input_method::Event),
    Keyboard(keyboard::Event),
}

/// What a widget's event handling hands back: messages, and whether it wants another frame.
pub struct Shell<M> {
    pub messages: Vec<M>,
    pub captured: bool,
    pub redraw: bool,
    pub redraw_at: Option<Instant>,
    /// Where the caret is, for the input method's popup.
    pub ime_cursor: Option<Rectangle>,
}

impl<M> Default for Shell<M> {
    fn default() -> Self {
        Self { messages: Vec::new(), captured: false, redraw: false, redraw_at: None, ime_cursor: None }
    }
}

impl<M> Shell<M> {
    pub fn publish(&mut self, m: M) {
        self.messages.push(m);
    }
    pub fn capture_event(&mut self) {
        self.captured = true;
    }
    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }
    pub fn request_redraw_at(&mut self, at: Instant) {
        self.redraw_at = Some(match self.redraw_at {
            Some(old) if old < at => old,
            _ => at,
        });
    }
}

// Drawing -------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default)]
pub struct Border {
    pub radius: f32,
    pub width: f32,
    pub color: Color,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Quad {
    pub bounds: Rectangle,
    pub border: Border,
}

/// A run of text to draw: one line, one font, as the widgets describe it.
pub struct Text {
    pub content: String,
    pub size: f32,
    pub line_h: f32,
    pub font: Font,
}

type LayoutKey = (String, Font, u32);

/// Text already shaped by Pango, kept between frames: most of what's on screen didn't change.
#[derive(Default)]
pub struct TextCache {
    layouts: HashMap<LayoutKey, (pango::Layout, f32, f32)>,
    used: u64,
}

impl TextCache {
    const MAX: usize = 6000;

    pub fn clear(&mut self) {
        self.layouts.clear();
    }

    fn get(&mut self, ctx: &pango::Context, t: &Text) -> (pango::Layout, f32, f32) {
        let key = (t.content.clone(), t.font, (t.size * 100.0) as u32);
        if let Some(l) = self.layouts.get(&key) {
            return l.clone();
        }
        if self.layouts.len() > Self::MAX {
            self.layouts.clear();
        }
        self.used += 1;
        let layout = pango::Layout::new(ctx);
        layout.set_font_description(Some(&t.font.description(t.size)));
        layout.set_text(&t.content);
        let (_, logical) = layout.pixel_extents();
        let entry = (layout, logical.width() as f32, logical.height() as f32);
        self.layouts.insert(key, entry.clone());
        entry
    }

    /// The width of a run, as it would be drawn.
    pub fn measure(&mut self, ctx: &pango::Context, t: &Text) -> Size {
        let (_, w, h) = self.get(ctx, t);
        Size::new(w, h)
    }
}

pub struct Painter<'a> {
    pub snapshot: &'a gtk::Snapshot,
    pub pango: pango::Context,
    pub cache: &'a RefCell<TextCache>,
}

impl Painter<'_> {
    pub fn fill_quad(&mut self, q: Quad, color: Color) {
        if q.bounds.width <= 0.0 || q.bounds.height <= 0.0 {
            return;
        }
        let rect = q.bounds.graphene();
        let rounded = gsk::RoundedRect::from_rect(rect, q.border.radius);
        if color.a > 0.0 {
            if q.border.radius > 0.0 {
                self.snapshot.push_rounded_clip(&rounded);
                self.snapshot.append_color(&color.rgba(), &rect);
                self.snapshot.pop();
            } else {
                self.snapshot.append_color(&color.rgba(), &rect);
            }
        }
        // The outline, drawn inside the bounds.
        if q.border.width > 0.0 && q.border.color.a > 0.0 {
            let c = q.border.color.rgba();
            self.snapshot.append_border(&rounded, &[q.border.width; 4], &[c, c, c, c]);
        }
    }

    pub fn with_layer(&mut self, clip: Rectangle, f: impl FnOnce(&mut Self)) {
        self.snapshot.push_clip(&clip.graphene());
        f(self);
        self.snapshot.pop();
    }

    /// Text, centered on its line like the widgets expect.
    pub fn fill_text(&mut self, t: Text, at: Point, color: Color, _clip: Rectangle) {
        if t.content.is_empty() {
            return;
        }
        let (layout, _, h) = self.cache.borrow_mut().get(&self.pango, &t);
        let dy = ((t.line_h - h) / 2.0).round();
        self.snapshot.save();
        self.snapshot.translate(&graphene::Point::new(at.x.round(), at.y + dy));
        self.snapshot.append_layout(&layout, &color.rgba());
        self.snapshot.restore();
    }
}
