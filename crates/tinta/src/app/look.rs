//! How the editor looks: COSMIC's own style, or GNOME's when it runs there.

use super::{App, Message};
use crate::i18n::tr;
use cosmic::Theme;
use cosmic::app::Task;
use cosmic::iced::font::{Stretch, Style as FontStyle, Weight};
use tinta_core::desktop::{self, GnomePrefs, Style};
use tinta_core::settings::{AppTheme, Settings};

#[derive(Debug, Clone, PartialEq)]
pub struct Look {
    pub style: Style,
    pub gnome: GnomePrefs,
}

impl Look {
    pub fn detect(settings: &Settings) -> Self {
        let style = desktop::detect_from_env(settings.interface_style);
        let gnome = if style == Style::Gnome { GnomePrefs::read() } else { GnomePrefs::default() };
        Self { style, gnome }
    }

    #[cfg(test)]
    pub fn cosmic() -> Self {
        Self { style: Style::Cosmic, gnome: GnomePrefs::default() }
    }

    pub fn is_gnome(&self) -> bool {
        self.style == Style::Gnome
    }

    /// GNOME's theme, or `None` to keep COSMIC's.
    pub fn theme(&self, ui: AppTheme) -> Option<Theme> {
        if !self.is_gnome() {
            return None;
        }
        let dark = match ui {
            AppTheme::System => self.gnome.dark,
            AppTheme::Dark => true,
            AppTheme::Light => false,
        };
        Some(crate::ui::adwaita::theme(dark, &self.gnome.accent))
    }

    /// Fonts, icons and window buttons, which libcosmic reads from its own settings. On GNOME
    /// there are no COSMIC settings to read, so these are the toolkit's defaults until set here.
    pub fn apply_toolkit(&self) {
        // The headless tests share one process, and these are process-wide: a test that changed
        // them would change the fonts under every test running beside it.
        if !self.is_gnome() || cfg!(test) {
            return;
        }
        let font = |family: &str| cosmic::config::FontConfig { family: family.to_string(), weight: Weight::Normal, stretch: Stretch::Normal, style: FontStyle::Normal };
        if let Ok(mut tk) = cosmic::config::COSMIC_TK.write() {
            tk.interface_font = font(&self.gnome.font.family);
            tk.monospace_font = font(&self.gnome.mono.family);
            tk.icon_theme = self.gnome.icon_theme.clone();
            tk.show_minimize = self.gnome.minimize;
            tk.show_maximize = self.gnome.maximize;
        }
        cosmic::icon_theme::set_default(self.gnome.icon_theme.clone());
    }
}

impl App {
    /// The editor's font: the one in the settings, unless it's the default and GNOME has its own.
    pub fn editor_font_family(&self) -> String {
        if self.look.is_gnome() && self.settings.font_family == Settings::default().font_family {
            return self.look.gnome.mono.family.clone();
        }
        self.settings.font_family.clone()
    }

    /// Follows GNOME's settings as they change, while the editor runs.
    pub fn watch_desktop(&self) {
        if self.look.is_gnome() {
            desktop::watch(|key, value| crate::bus::send(Message::GnomeSetting(key, value)));
        }
    }

    pub fn on_gnome_setting(&mut self, key: String, value: String) -> Task<Message> {
        if !self.look.is_gnome() || !self.look.gnome.apply(&key, &value) {
            return Task::none();
        }
        self.look.apply_toolkit();
        match key.as_str() {
            "color-scheme" | "accent-color" => self.apply_ui_theme(),
            "monospace-font-name" => {
                self.apply_font();
                Task::none()
            }
            _ => Task::none(),
        }
    }

    pub fn set_interface_style(&mut self, style: tinta_core::desktop::InterfaceStyle) -> Task<Message> {
        self.settings.interface_style = style;
        let was_gnome = self.look.is_gnome();
        self.look = Look::detect(&self.settings);
        self.look.apply_toolkit();
        if self.look.is_gnome() && !was_gnome {
            self.watch_desktop();
        }
        self.apply_font();
        self.persist_settings();
        let theme = self.apply_ui_theme();
        // libcosmic reads fonts and icons once; going back to COSMIC's needs a fresh start.
        Task::batch([theme, self.toast(tr("Some of this changes when the editor restarts"))])
    }
}
