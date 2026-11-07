pub mod adwaita;
pub mod debug_view;
pub mod diff_view;
pub mod editor_view;
pub mod fresh;
pub mod icons;
pub mod markdown;
pub mod menu;
pub mod panel;
pub mod palette;
pub mod popups;
pub mod sash;
pub mod settings_page;
pub mod sidebar;
pub mod status;
pub mod terminal;
pub mod workspace;

use cosmic::iced::{Background, Color};
use cosmic::widget::button::Style;
use cosmic::{Theme, theme};

fn flat(t: &Theme, fill: Option<Color>, text: Option<Color>) -> Style {
    let c = t.cosmic();
    let on: Color = c.background(false).on.into();
    let mut s = Style::new();
    s.background = fill.map(Background::Color);
    s.border_radius = c.corner_radii.radius_s.into();
    s.text_color = Some(text.unwrap_or(on));
    s.icon_color = Some(text.unwrap_or(on));
    s
}

/// A row in a list or the status bar: no box until hovered, no accent colour on the text.
pub fn row_button(selected: bool) -> theme::Button {
    let tint = move |t: &Theme, amount: f32| -> Option<Color> {
        let on: Color = t.cosmic().background(false).on.into();
        Some(Color { a: amount, ..on })
    };
    let base = if selected { 0.14 } else { 0.0 };
    theme::Button::Custom {
        active: Box::new(move |_, t| flat(t, if selected { tint(t, base) } else { None }, None)),
        disabled: Box::new(move |t| flat(t, None, None)),
        hovered: Box::new(move |_, t| flat(t, tint(t, base + 0.08), None)),
        pressed: Box::new(move |_, t| flat(t, tint(t, base + 0.14), None)),
    }
}
