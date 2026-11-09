//! Settings stored as a JSON file the user can also edit by hand, like VS Code's.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LineNumbers {
    #[default]
    On,
    Relative,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Whitespace {
    None,
    #[default]
    Selection,
    Boundary,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Bar,
    Block,
    Underline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AutoSave {
    #[default]
    Off,
    Delay,
    Focus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AppTheme {
    #[default]
    System,
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LspServer {
    pub command: String,
    pub args: Vec<String>,
}

impl Default for LspServer {
    fn default() -> Self {
        Self { command: String::new(), args: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    pub ui_theme: AppTheme,
    pub syntax_theme: String,
    pub tab_size: usize,
    pub insert_spaces: bool,
    pub detect_indentation: bool,
    pub word_wrap: bool,
    pub line_numbers: LineNumbers,
    pub minimap: bool,
    pub render_whitespace: Whitespace,
    pub rulers: Vec<usize>,
    pub highlight_current_line: bool,
    pub bracket_colors: bool,
    pub indent_guides: bool,
    pub sticky_scroll: bool,
    pub folding: bool,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    pub auto_pairs: bool,
    pub auto_indent: bool,
    pub auto_complete: bool,
    pub word_completion: bool,
    pub snippets: bool,
    pub occurrences_highlight: bool,
    pub inline_blame: bool,
    pub git_gutter: bool,
    pub auto_save: AutoSave,
    pub auto_save_delay_ms: u64,
    pub format_on_save: bool,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
    pub restore_session: bool,
    pub hot_exit: bool,
    pub show_hidden_files: bool,
    /// Leaves out of the file tree the programs built next to their source: `prova` beside
    /// `prova.c`.
    pub hide_build_outputs: bool,
    pub lsp_enabled: bool,
    pub lsp_servers: HashMap<String, LspServer>,
    /// How Ctrl+F5 runs files, by language or `.extension`, before the built-in table.
    pub run_commands: crate::run::UserRunners,
    pub scroll_speed: f32,
    pub terminal_shell: String,
    pub terminal_font_size: f32,
    pub language: String,
    pub vim_mode: bool,
    /// Buttons of the header bar, as command ids, left to right: beside the main menu and at the
    /// window's controls.
    pub header_start: Vec<String>,
    pub header_end: Vec<String>,
}

pub fn default_header_start() -> Vec<String> {
    vec!["view.sidebar".into()]
}

pub fn default_header_end() -> Vec<String> {
    ["run.file", "view.split", "view.terminal", "app.settings"].map(String::from).to_vec()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            font_family: "Noto Sans Mono".into(),
            font_size: 14.0,
            line_height: 1.45,
            ui_theme: AppTheme::System,
            syntax_theme: "COSMIC".into(),
            tab_size: 4,
            insert_spaces: true,
            detect_indentation: true,
            word_wrap: false,
            line_numbers: LineNumbers::On,
            minimap: true,
            render_whitespace: Whitespace::Selection,
            rulers: Vec::new(),
            highlight_current_line: true,
            bracket_colors: true,
            indent_guides: true,
            sticky_scroll: true,
            folding: true,
            cursor_style: CursorStyle::Bar,
            cursor_blink: true,
            auto_pairs: true,
            auto_indent: true,
            auto_complete: true,
            word_completion: true,
            snippets: true,
            occurrences_highlight: true,
            inline_blame: false,
            git_gutter: true,
            auto_save: AutoSave::Off,
            auto_save_delay_ms: 1000,
            format_on_save: false,
            trim_trailing_whitespace: false,
            insert_final_newline: false,
            restore_session: true,
            hot_exit: true,
            show_hidden_files: true,
            hide_build_outputs: true,
            lsp_enabled: true,
            lsp_servers: HashMap::new(),
            run_commands: HashMap::new(),
            scroll_speed: 3.0,
            terminal_shell: String::new(),
            terminal_font_size: 13.0,
            language: String::new(),
            vim_mode: false,
            header_start: default_header_start(),
            header_end: default_header_end(),
        }
    }
}

pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".config")).join("tinta-gnome")
}

pub fn state_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| PathBuf::from(".local/state")).join("tinta-gnome")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn keybindings_path() -> PathBuf {
    config_dir().join("keybindings.json")
}

pub fn snippets_path() -> PathBuf {
    config_dir().join("snippets.json")
}

/// Removes `//` comments outside strings and trailing commas, so hand-edited files keep working.
pub fn strip_jsonc(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas.
    let mut cleaned = String::with_capacity(out.len());
    let bytes: Vec<char> = out.chars().collect();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == '"' && (i == 0 || bytes[i - 1] != '\\') {
            in_str = !in_str;
        }
        if c == ',' && !in_str {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_whitespace() {
                j += 1;
            }
            if j < bytes.len() && matches!(bytes[j], '}' | ']') {
                i += 1;
                continue;
            }
        }
        cleaned.push(c);
        i += 1;
    }
    cleaned
}

impl Settings {
    pub fn parse(src: &str) -> Result<Self, String> {
        let s: Settings = serde_json::from_str(&strip_jsonc(src)).map_err(|e| e.to_string())?;
        Ok(s.sanitized())
    }

    fn sanitized(mut self) -> Self {
        self.font_size = self.font_size.clamp(6.0, 72.0);
        self.line_height = self.line_height.clamp(1.0, 3.0);
        self.tab_size = self.tab_size.clamp(1, 16);
        self.terminal_font_size = self.terminal_font_size.clamp(6.0, 72.0);
        self.scroll_speed = self.scroll_speed.clamp(0.5, 30.0);
        self
    }

    /// Reads the file. A missing one gives defaults; a broken one gives defaults and the error.
    pub fn load() -> (Self, Option<String>) {
        match std::fs::read_to_string(settings_path()) {
            Ok(src) => match Self::parse(&src) {
                Ok(s) => (s, None),
                Err(e) => (Self::default(), Some(e)),
            },
            Err(_) => (Self::default(), None),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        std::fs::create_dir_all(config_dir()).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::buffer::atomic_write(&settings_path(), text.as_bytes()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_fill_in_defaults() {
        let s = Settings::parse(r#"{ "fontSize": 20, "wordWrap": true }"#).unwrap();
        assert_eq!(s.font_size, 20.0);
        assert!(s.word_wrap);
        assert_eq!(s.tab_size, 4);
    }

    #[test]
    fn comments_and_trailing_commas_are_fine() {
        let s = Settings::parse("{\n // size\n \"fontSize\": 18, /* x */\n \"rulers\": [80, 100,],\n}\n").unwrap();
        assert_eq!(s.font_size, 18.0);
        assert_eq!(s.rulers, vec![80, 100]);
    }

    #[test]
    fn strings_keep_their_slashes_and_commas() {
        let s = Settings::parse(r#"{ "terminalShell": "/bin//zsh,}" }"#).unwrap();
        assert_eq!(s.terminal_shell, "/bin//zsh,}");
    }

    #[test]
    fn nonsense_values_are_clamped_and_broken_json_is_an_error() {
        let s = Settings::parse(r#"{ "fontSize": 9000, "tabSize": 0 }"#).unwrap();
        assert_eq!((s.font_size, s.tab_size), (72.0, 1));
        assert!(Settings::parse("{ nope").is_err());
    }

    #[test]
    fn round_trips() {
        let s = Settings::default();
        let t = serde_json::to_string(&s).unwrap();
        assert_eq!(Settings::parse(&t).unwrap(), s);
    }
}
