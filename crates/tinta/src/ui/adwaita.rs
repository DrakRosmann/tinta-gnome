//! GNOME's look, as a COSMIC theme: libadwaita's surfaces, accent, status colors and corners.

use cosmic::Theme;
use cosmic::cosmic_theme::palette::{Srgb, Srgba};
use cosmic::cosmic_theme::{CornerRadii, ThemeBuilder};
use std::sync::Arc;
use tinta_core::desktop::{accent_rgb, surfaces};

fn rgb(c: [u8; 3]) -> Srgb {
    Srgb::new(c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0)
}

fn rgba(c: [u8; 3]) -> Srgba {
    let s = rgb(c);
    Srgba::new(s.red, s.green, s.blue, 1.0)
}

/// libadwaita's corners: 6 for buttons and entries, 12 for popovers and cards.
pub fn corners() -> CornerRadii {
    CornerRadii {
        radius_0: [0.0; 4],
        radius_xs: [4.0; 4],
        radius_s: [6.0; 4],
        radius_m: [12.0; 4],
        radius_l: [12.0; 4],
        radius_xl: [6.0; 4],
    }
}

pub fn theme(dark: bool, accent: &str) -> Theme {
    let s = surfaces(dark);
    let mut b = if dark { ThemeBuilder::dark() } else { ThemeBuilder::light() };
    b = b
        .bg_color(rgba(s.window))
        .primary_container_bg(rgba(s.sidebar))
        .accent(rgb(accent_rgb(accent)))
        .success(rgb(s.success))
        .warning(rgb(s.warning))
        .destructive(rgb(s.error))
        .corner_radii(corners());
    b.secondary_container_bg = Some(rgba(s.popover));
    Theme::custom(Arc::new(b.build()))
}
