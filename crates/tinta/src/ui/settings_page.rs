//! The settings drawer, with COSMIC's own settings widgets.

use crate::app::{App, BoolSetting as B, Message, SettingMsg as S};
use crate::i18n::tr;
use cosmic::Element;
use cosmic::iced::Length;
use cosmic::widget::{self, button, column, settings, text};
use tinta_core::keymap::Command;
use tinta_core::settings::{AppTheme, AutoSave, CursorStyle, LineNumbers, Whitespace};

fn toggle<'a>(label: &'static str, value: bool, key: B) -> Element<'a, Message> {
    settings::item(tr(label), widget::toggler(value).on_toggle(move |v| Message::Setting(S::Bool(key, v)))).into()
}

fn choice<'a, T: Copy + PartialEq + Send + Sync + 'static>(
    label: &'static str,
    options: &[(&'static str, T)],
    current: T,
    to_msg: impl Fn(T) -> S + Send + Sync + 'static,
) -> Element<'a, Message> {
    let names: Vec<&'static str> = options.iter().map(|(n, _)| tr(n)).collect();
    let values: Vec<T> = options.iter().map(|(_, v)| *v).collect();
    let selected = values.iter().position(|v| *v == current);
    settings::item::builder(tr(label))
        .control(widget::dropdown(names, selected, move |i| Message::Setting(to_msg(values[i]))))
        .into()
}

pub fn view(app: &App) -> Element<'_, Message> {
    let s = &app.settings;
    let appearance = settings::section()
        .title(tr("Appearance"))
        .add(choice(
            "Theme",
            &[("Follow system", AppTheme::System), ("Dark", AppTheme::Dark), ("Light", AppTheme::Light)],
            s.ui_theme,
            S::UiTheme,
        ))
        .add(choice(
            "Interface style",
            &[("Follow the desktop", tinta_core::desktop::InterfaceStyle::Auto), ("COSMIC", tinta_core::desktop::InterfaceStyle::Cosmic), ("GNOME", tinta_core::desktop::InterfaceStyle::Gnome)],
            s.interface_style,
            S::InterfaceStyle,
        ))
        .add(
            settings::item::builder(tr("Syntax colors")).control(widget::dropdown(
                app.syntax_theme_names.as_slice(),
                app.syntax_theme_names.iter().position(|n| *n == s.syntax_theme),
                |i| Message::Setting(S::SyntaxTheme(tinta_core::syntax::theme_names()[i].clone())),
            )),
        )
        .add(
            settings::item::builder(tr("Font")).control(widget::dropdown(
                app.font_names.as_slice(),
                app.font_names.iter().position(|n| *n == s.font_family),
                {
                    let names = app.font_names.clone();
                    move |i| Message::Setting(S::FontFamily(names[i].clone()))
                },
            )),
        )
        .add(settings::item::builder(tr("Font size")).control(widget::spin_button(
            format!("{}", app.base_font_size as i32),
            tr("Font size"),
            app.base_font_size as i32,
            1,
            6,
            72,
            |v| Message::Setting(S::FontSize(v as f32)),
        )))
        .add(settings::item::builder(tr("Line height")).control(widget::spin_button(
            format!("{:.2}", s.line_height),
            tr("Line height"),
            (s.line_height * 100.0).round() as i32,
            5,
            100,
            300,
            |v| Message::Setting(S::LineHeight(v as f32 / 100.0)),
        )))
        .add(choice(
            "Line numbers",
            &[("On", LineNumbers::On), ("Relative", LineNumbers::Relative), ("Off", LineNumbers::Off)],
            s.line_numbers,
            S::LineNumbers,
        ))
        .add(choice(
            "Show whitespace",
            &[("Never", Whitespace::None), ("In selection", Whitespace::Selection), ("At boundaries", Whitespace::Boundary), ("Always", Whitespace::All)],
            s.render_whitespace,
            S::Whitespace,
        ))
        .add(choice(
            "Cursor style",
            &[("Bar", CursorStyle::Bar), ("Block", CursorStyle::Block), ("Underline", CursorStyle::Underline)],
            s.cursor_style,
            S::CursorStyle,
        ))
        .add(toggle("Blinking cursor", s.cursor_blink, B::CursorBlink))
        .add(toggle("Minimap", s.minimap, B::Minimap))
        .add(toggle("Highlight current line", s.highlight_current_line, B::CurrentLine))
        .add(toggle("Rainbow brackets", s.bracket_colors, B::BracketColors))
        .add(toggle("Indent guides", s.indent_guides, B::IndentGuides))
        .add(toggle("Sticky scroll", s.sticky_scroll, B::StickyScroll))
        .add(toggle("Highlight occurrences", s.occurrences_highlight, B::Occurrences));

    let editor = settings::section()
        .title(tr("Editor"))
        .add(settings::item::builder(tr("Tab size")).control(widget::spin_button(
            s.tab_size.to_string(),
            tr("Tab size"),
            s.tab_size as i32,
            1,
            1,
            16,
            |v| Message::Setting(S::TabSize(v as usize)),
        )))
        .add(toggle("Insert spaces", s.insert_spaces, B::InsertSpaces))
        .add(toggle("Detect indentation", s.detect_indentation, B::DetectIndent))
        .add(toggle("Word wrap", s.word_wrap, B::WordWrap))
        .add(toggle("Vim mode", s.vim_mode, B::Vim))
        .add(toggle("Code folding", s.folding, B::Folding))
        .add(toggle("Auto close brackets", s.auto_pairs, B::AutoPairs))
        .add(toggle("Auto indent", s.auto_indent, B::AutoIndent));

    let files = settings::section()
        .title(tr("Files"))
        .add(choice(
            "Auto save",
            &[("Off", AutoSave::Off), ("After a delay", AutoSave::Delay), ("When focus leaves", AutoSave::Focus)],
            s.auto_save,
            S::AutoSave,
        ))
        .add(toggle("Format on save", s.format_on_save, B::FormatOnSave))
        .add(toggle("Trim trailing whitespace on save", s.trim_trailing_whitespace, B::TrimTrailing))
        .add(toggle("Final newline on save", s.insert_final_newline, B::FinalNewline))
        .add(toggle("Restore session", s.restore_session, B::RestoreSession))
        .add(toggle("Keep unsaved work between runs", s.hot_exit, B::HotExit))
        .add(toggle("Show hidden files", s.show_hidden_files, B::ShowHidden));

    let code = settings::section()
        .title(tr("Code intelligence"))
        .add(toggle("Language servers", s.lsp_enabled, B::Lsp))
        .add(toggle("Suggestions while typing", s.auto_complete, B::AutoComplete))
        .add(toggle("Word suggestions", s.word_completion, B::WordCompletion))
        .add(toggle("Snippets", s.snippets, B::Snippets))
        .add(toggle("Git marks in the gutter", s.git_gutter, B::GitGutter))
        .add(toggle("Git blame on the current line", s.inline_blame, B::InlineBlame));

    column::with_children(vec![
        appearance.into(),
        editor.into(),
        files.into(),
        code.into(),
        button::text(tr("Open settings file")).on_press(Message::Cmd(Command::OpenSettingsJson)).into(),
        button::text(tr("Open keyboard shortcuts file")).on_press(Message::Cmd(Command::OpenKeybindings)).into(),
        button::text(tr("Open user snippets file")).on_press(Message::Cmd(Command::OpenSnippets)).into(),
        text(format!("{} · {}", tinta_core::settings::settings_path().display(), env!("CARGO_PKG_VERSION"))).size(11).into(),
    ])
    .spacing(14)
    .width(Length::Fill)
    .into()
}
