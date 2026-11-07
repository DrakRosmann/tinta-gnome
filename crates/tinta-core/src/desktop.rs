//! Which desktop this is, and on GNOME, what it's set to: dark or light, accent, fonts, icons
//! and which window buttons to show. Read through `gsettings`, so it needs no GNOME libraries.

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::process::Stdio;

/// What the settings ask for. `Auto` goes by the desktop the editor finds itself on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InterfaceStyle {
    #[default]
    Auto,
    Cosmic,
    Gnome,
}

/// The look the editor takes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Cosmic,
    Gnome,
}

/// `XDG_CURRENT_DESKTOP` is a list, like `ubuntu:GNOME`. Anything that isn't GNOME keeps COSMIC's
/// look, which is what this editor was made with.
pub fn detect(current_desktop: &str, choice: InterfaceStyle) -> Style {
    match choice {
        InterfaceStyle::Cosmic => Style::Cosmic,
        InterfaceStyle::Gnome => Style::Gnome,
        InterfaceStyle::Auto => {
            let names: Vec<String> = current_desktop.split(':').map(|s| s.trim().to_ascii_uppercase()).collect();
            if names.iter().any(|n| n == "COSMIC") {
                Style::Cosmic
            } else if names.iter().any(|n| n == "GNOME" || n == "GNOME-CLASSIC") {
                Style::Gnome
            } else {
                Style::Cosmic
            }
        }
    }
}

pub fn detect_from_env(choice: InterfaceStyle) -> Style {
    detect(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(), choice)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub family: String,
    pub size: f32,
}

impl Font {
    /// GNOME writes fonts as one string: `Adwaita Sans 11`, `Cantarell Bold 11`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (rest, size) = match s.rsplit_once(' ') {
            Some((r, n)) if n.parse::<f32>().is_ok() => (r, n.parse::<f32>().unwrap()),
            _ => (s, 11.0),
        };
        // Style words aren't part of the family's name.
        let mut words: Vec<&str> = rest.split_whitespace().collect();
        while words.len() > 1
            && matches!(
                words.last().map(|w| w.to_ascii_lowercase()).as_deref(),
                Some("bold" | "italic" | "oblique" | "light" | "medium" | "semibold" | "regular" | "thin" | "heavy" | "black" | "condensed")
            )
        {
            words.pop();
        }
        let family = words.join(" ");
        (!family.is_empty()).then_some(Self { family, size })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GnomePrefs {
    /// `color-scheme` is `prefer-dark`.
    pub dark: bool,
    /// The name GNOME uses, like `blue` or `teal`.
    pub accent: String,
    pub font: Font,
    pub mono: Font,
    pub icon_theme: String,
    pub minimize: bool,
    pub maximize: bool,
}

impl Default for GnomePrefs {
    /// What a fresh GNOME has.
    fn default() -> Self {
        Self {
            dark: false,
            accent: "blue".into(),
            font: Font { family: "Adwaita Sans".into(), size: 11.0 },
            mono: Font { family: "Adwaita Mono".into(), size: 11.0 },
            icon_theme: "Adwaita".into(),
            minimize: false,
            maximize: false,
        }
    }
}

/// `'prefer-dark'` to `prefer-dark`.
fn unquote(v: &str) -> &str {
    let v = v.trim();
    v.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')).unwrap_or(v)
}

impl GnomePrefs {
    /// One key as `gsettings` prints it. True if anything changed.
    pub fn apply(&mut self, key: &str, raw: &str) -> bool {
        let v = unquote(raw);
        let before = self.clone();
        match key {
            "color-scheme" => self.dark = v == "prefer-dark",
            "accent-color" => self.accent = v.to_string(),
            "font-name" => {
                if let Some(f) = Font::parse(v) {
                    self.font = f;
                }
            }
            "monospace-font-name" => {
                if let Some(f) = Font::parse(v) {
                    self.mono = f;
                }
            }
            "icon-theme" if !v.is_empty() => self.icon_theme = v.to_string(),
            "button-layout" => {
                // `appmenu:minimize,maximize,close`: which buttons, on which side.
                let buttons: Vec<&str> = v.split([':', ',']).collect();
                self.minimize = buttons.contains(&"minimize");
                self.maximize = buttons.contains(&"maximize");
            }
            _ => {}
        }
        *self != before
    }

    /// The output of `gsettings list-recursively SCHEMA`: `schema key value` per line.
    pub fn apply_listing(&mut self, text: &str) {
        for line in text.lines() {
            let mut parts = line.splitn(3, ' ');
            if let (Some(_), Some(key), Some(value)) = (parts.next(), parts.next(), parts.next()) {
                self.apply(key, value);
            }
        }
    }

    /// What this session has, or GNOME's defaults where it can't tell.
    pub fn read() -> Self {
        let mut p = Self::default();
        for schema in ["org.gnome.desktop.interface", "org.gnome.desktop.wm.preferences"] {
            if let Ok(out) = crate::host::command("gsettings", ["list-recursively", schema], None, &[]).stdin(Stdio::null()).stderr(Stdio::null()).output() {
                p.apply_listing(&String::from_utf8_lossy(&out.stdout));
            }
        }
        p
    }
}

/// A line of `gsettings monitor`: `accent-color: 'teal'`.
pub fn parse_monitor_line(line: &str) -> Option<(String, String)> {
    let (k, v) = line.split_once(':')?;
    Some((k.trim().to_string(), v.trim().to_string()))
}

/// Calls `on_change` from a thread whenever an interface setting changes, while the editor runs.
pub fn watch(on_change: impl Fn(String, String) + Send + 'static) {
    std::thread::spawn(move || {
        let Ok(mut child) = crate::host::command("gsettings", ["monitor", "org.gnome.desktop.interface"], None, &[])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        else {
            return;
        };
        let Some(out) = child.stdout.take() else { return };
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some((k, v)) = parse_monitor_line(&line) {
                on_change(k, v);
            }
        }
        let _ = child.wait();
    });
}

/// GNOME's accent colors, as libadwaita paints them.
pub fn accent_rgb(name: &str) -> [u8; 3] {
    match name {
        "teal" => [0x21, 0x90, 0xa4],
        "green" => [0x3a, 0x94, 0x4a],
        "yellow" => [0xc8, 0x88, 0x00],
        "orange" => [0xed, 0x5b, 0x00],
        "red" => [0xe6, 0x2d, 0x42],
        "pink" => [0xd5, 0x61, 0x99],
        "purple" => [0x91, 0x41, 0xac],
        "slate" => [0x6f, 0x83, 0x96],
        _ => [0x35, 0x84, 0xe4],
    }
}

/// libadwaita's surfaces: window, view (where text goes), header bar and sidebar, popovers.
pub struct Surfaces {
    pub window: [u8; 3],
    pub view: [u8; 3],
    pub sidebar: [u8; 3],
    pub popover: [u8; 3],
    pub success: [u8; 3],
    pub warning: [u8; 3],
    pub error: [u8; 3],
}

pub fn surfaces(dark: bool) -> Surfaces {
    if dark {
        Surfaces {
            window: [0x22, 0x22, 0x26],
            view: [0x1d, 0x1d, 0x20],
            sidebar: [0x2e, 0x2e, 0x32],
            popover: [0x36, 0x36, 0x3a],
            success: [0x26, 0xa2, 0x69],
            warning: [0xcd, 0x93, 0x09],
            error: [0xc0, 0x1c, 0x28],
        }
    } else {
        Surfaces {
            window: [0xfa, 0xfa, 0xfb],
            view: [0xff, 0xff, 0xff],
            sidebar: [0xeb, 0xeb, 0xed],
            popover: [0xff, 0xff, 0xff],
            success: [0x2e, 0xc2, 0x7e],
            warning: [0xe5, 0xa5, 0x0a],
            error: [0xe0, 0x1b, 0x24],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_desktop() {
        assert_eq!(detect("GNOME", InterfaceStyle::Auto), Style::Gnome);
        assert_eq!(detect("ubuntu:GNOME", InterfaceStyle::Auto), Style::Gnome);
        assert_eq!(detect("GNOME-Classic:GNOME", InterfaceStyle::Auto), Style::Gnome);
        assert_eq!(detect("COSMIC", InterfaceStyle::Auto), Style::Cosmic);
        assert_eq!(detect("pop:COSMIC", InterfaceStyle::Auto), Style::Cosmic);
        // KDE, Sway, nothing at all: COSMIC's look, as before.
        assert_eq!(detect("KDE", InterfaceStyle::Auto), Style::Cosmic);
        assert_eq!(detect("", InterfaceStyle::Auto), Style::Cosmic);
        // The setting wins over the desktop.
        assert_eq!(detect("COSMIC", InterfaceStyle::Gnome), Style::Gnome);
        assert_eq!(detect("GNOME", InterfaceStyle::Cosmic), Style::Cosmic);
    }

    #[test]
    fn fonts_as_gnome_writes_them() {
        assert_eq!(Font::parse("Adwaita Sans 11"), Some(Font { family: "Adwaita Sans".into(), size: 11.0 }));
        assert_eq!(Font::parse("Cantarell Bold 10.5"), Some(Font { family: "Cantarell".into(), size: 10.5 }));
        assert_eq!(Font::parse("Noto Sans"), Some(Font { family: "Noto Sans".into(), size: 11.0 }));
        assert_eq!(Font::parse("Source Code Pro Medium 12").unwrap().family, "Source Code Pro");
        assert_eq!(Font::parse(""), None);
    }

    #[test]
    fn reading_gsettings_output() {
        let mut p = GnomePrefs::default();
        p.apply_listing(
            "org.gnome.desktop.interface accent-color 'teal'\n\
             org.gnome.desktop.interface color-scheme 'prefer-dark'\n\
             org.gnome.desktop.interface font-name 'Cantarell 12'\n\
             org.gnome.desktop.interface monospace-font-name 'JetBrains Mono 10'\n\
             org.gnome.desktop.interface icon-theme 'Papirus'\n\
             org.gnome.desktop.interface enable-animations true\n\
             org.gnome.desktop.wm.preferences button-layout 'appmenu:minimize,close'\n",
        );
        assert!(p.dark);
        assert_eq!(p.accent, "teal");
        assert_eq!(p.font, Font { family: "Cantarell".into(), size: 12.0 });
        assert_eq!(p.mono.family, "JetBrains Mono");
        assert_eq!(p.icon_theme, "Papirus");
        assert!(p.minimize && !p.maximize);
        // A light scheme, or the default one, isn't dark.
        assert!(p.apply("color-scheme", "'default'"));
        assert!(!p.dark);
        assert!(!p.apply("color-scheme", "'prefer-light'"), "no change to report");
    }

    #[test]
    fn what_gsettings_monitor_prints() {
        assert_eq!(parse_monitor_line("accent-color: 'pink'"), Some(("accent-color".into(), "'pink'".into())));
        let mut p = GnomePrefs::default();
        let (k, v) = parse_monitor_line("color-scheme: 'prefer-dark'").unwrap();
        assert!(p.apply(&k, &v));
        assert!(p.dark);
    }

    #[test]
    fn a_fresh_gnome_by_default() {
        let p = GnomePrefs::default();
        assert!(!p.dark && !p.minimize && !p.maximize);
        assert_eq!(accent_rgb(&p.accent), [0x35, 0x84, 0xe4]);
        assert_eq!(accent_rgb("an accent from the future"), accent_rgb("blue"));
    }

    /// Through the real `gsettings`, with a settings file of its own, never the user's.
    #[test]
    fn reading_and_watching_a_real_gsettings() {
        if std::process::Command::new("gsettings").arg("--version").output().is_err()
            || !std::process::Command::new("gsettings").args(["list-keys", "org.gnome.desktop.interface"]).output().is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains("accent-color"))
        {
            eprintln!("skipped: no gsettings with GNOME's schemas");
            return;
        }
        let dir = std::env::temp_dir().join(format!("tinta-desktop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| {
            std::process::Command::new("gsettings").args(args).env("GSETTINGS_BACKEND", "keyfile").env("XDG_CONFIG_HOME", &dir).output().unwrap()
        };
        run(&["set", "org.gnome.desktop.interface", "accent-color", "purple"]);
        run(&["set", "org.gnome.desktop.interface", "color-scheme", "prefer-dark"]);
        let out = run(&["list-recursively", "org.gnome.desktop.interface"]);
        let mut p = GnomePrefs::default();
        p.apply_listing(&String::from_utf8_lossy(&out.stdout));
        assert_eq!(p.accent, "purple");
        assert!(p.dark);
        assert!(dir.join("glib-2.0/settings/keyfile").exists(), "the test's own file, not the user's settings");
    }
}
