//! Dialogs the app asks for, the shortcuts list, About, and the preferences window.

use super::Shell;
use super::rows;
use super::window::Window;
use crate::app::{App, BoolSetting as B, DIALOG_INPUT, DialogMsg, DialogState, Message, SettingMsg as S};
use crate::i18n::tr;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use tinta_core::desktop::InterfaceStyle;
use tinta_core::settings::{AppTheme, AutoSave, CursorStyle, LineNumbers, Settings, Whitespace};

#[derive(Default)]
pub struct Dialogs {
    open: RefCell<Option<(String, adw::Dialog)>>,
    prefs: RefCell<Option<adw::PreferencesDialog>>,
}

fn file_of(p: &std::path::Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn kind_of(d: &DialogState) -> String {
    format!("{d:?}")
}

impl Dialogs {
    pub fn sync(&self, win: &Window, shell: &Rc<Shell>, app: &App) {
        let wanted = app.dialog.as_ref().map(kind_of);
        let current = self.open.borrow().as_ref().map(|(k, _)| k.clone());
        if wanted != current {
            if let Some((_, d)) = self.open.borrow_mut().take() {
                d.force_close();
            }
            if let Some(state) = &app.dialog {
                let d = self.build(win, shell, app, state);
                d.present(Some(&win.window));
                *self.open.borrow_mut() = Some((kind_of(state), d));
            }
        }
        self.sync_prefs(win, shell, app);
    }

    fn build(&self, win: &Window, shell: &Rc<Shell>, app: &App, state: &DialogState) -> adw::Dialog {
        let send = |m: DialogMsg| {
            let s = Rc::downgrade(shell);
            move || {
                if let Some(s) = s.upgrade() {
                    s.dispatch(Message::Dialog(m.clone()));
                }
            }
        };
        match state {
            DialogState::About => {
                let d = adw::AboutDialog::builder()
                    .application_name("Tinta")
                    .application_icon("io.github.DrakRosmann.TintaGnome")
                    .version(env!("CARGO_PKG_VERSION"))
                    .developer_name("DrakRosmann")
                    .license_type(gtk::License::Gpl30Only)
                    .website(env!("CARGO_PKG_REPOSITORY"))
                    .comments(tr("A code editor for GNOME, written in Rust with GTK 4 and libadwaita."))
                    .build();
                let cancel = send(DialogMsg::Cancel);
                d.connect_closed(move |_| cancel());
                return d.upcast();
            }
            DialogState::Shortcuts => return self.shortcuts(shell, app),
            _ => {}
        }
        let (title, body, input, primary, destructive, extra): (String, String, Option<&str>, &str, bool, Option<&str>) = match state {
            DialogState::CloseUnsaved(id) => {
                let name = app.tabs.get(id).and_then(|t| app.docs.get(&t.doc)).map(|e| e.doc.file_name()).unwrap_or_default();
                (name, tr("Save changes before closing?").into(), None, "Save", false, Some("Don't Save"))
            }
            DialogState::QuitUnsaved => (tr("Unsaved changes").into(), tr("You have unsaved files. Save them before quitting?").into(), None, "Save All", false, Some("Don't Save")),
            DialogState::ChangedOnDisk(doc) => {
                let name = app.docs.get(doc).map(|e| e.doc.file_name()).unwrap_or_default();
                (format!("{}: {name}", tr("File changed on disk")), tr("This file changed on disk and you have unsaved changes.").into(), None, "Reload", false, None)
            }
            DialogState::NewFile(dir) => (format!("{} · {}", tr("New File"), file_of(dir)), String::new(), Some("Name"), "Create", false, None),
            DialogState::NewFolder(dir) => (format!("{} · {}", tr("New Folder"), file_of(dir)), String::new(), Some("Name"), "Create", false, None),
            DialogState::RenamePath(p) => (format!("{} · {}", tr("Rename"), file_of(p)), String::new(), Some("Name"), "Rename", false, None),
            DialogState::TrashPath(p) => (file_of(p), tr("Move to Trash").into(), None, "Move to Trash", true, None),
            DialogState::RenameSymbol => (tr("Rename symbol to").into(), String::new(), Some("Name"), "Rename", false, None),
            DialogState::BreakpointCondition(p, line) => (
                format!("{}: {}:{}", tr("Breakpoint condition"), file_of(p), line + 1),
                tr("Stops only when this is true. Empty: always.").into(),
                Some("x > 10"),
                "OK",
                false,
                None,
            ),
            DialogState::ConfirmDiscard(e) => (e.path.clone(), tr("Discard changes to this file? This cannot be undone.").into(), None, "Discard Changes", true, None),
            DialogState::About | DialogState::Shortcuts => unreachable!(),
        };
        let d = adw::AlertDialog::new(Some(&title), (!body.is_empty()).then_some(body.as_str()));
        d.add_response("cancel", tr("Cancel"));
        if let Some(x) = extra {
            d.add_response("secondary", tr(x));
            d.set_response_appearance("secondary", adw::ResponseAppearance::Destructive);
        }
        d.add_response("primary", tr(primary));
        d.set_response_appearance("primary", if destructive { adw::ResponseAppearance::Destructive } else { adw::ResponseAppearance::Suggested });
        d.set_default_response(Some("primary"));
        d.set_close_response("cancel");
        if let Some(ph) = input {
            let e = rows::entry(shell, tr(ph), |t| Message::Dialog(DialogMsg::Input(t)));
            rows::set_text(&e, &app.dialog_input);
            e.set_activates_default(true);
            d.set_extra_child(Some(&e));
            win.register_field(DIALOG_INPUT, &e);
            let e2 = e.clone();
            rows::idle(move || {
                e2.grab_focus();
                e2.select_region(0, -1);
            });
        }
        let (p, s2, c) = (send(DialogMsg::Primary), send(DialogMsg::Secondary), send(DialogMsg::Cancel));
        d.connect_response(None, move |_, r| match r {
            "primary" => p(),
            "secondary" => s2(),
            _ => c(),
        });
        d.upcast()
    }

    fn shortcuts(&self, shell: &Rc<Shell>, app: &App) -> adw::Dialog {
        let d = adw::Dialog::builder().title(tr("Keyboard Shortcuts")).content_width(620).content_height(640).build();
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&adw::HeaderBar::new());
        let col = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let search = gtk::SearchEntry::new();
        search.set_margin_start(12);
        search.set_margin_end(12);
        col.append(&search);
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_margin_start(12);
        list.set_margin_end(12);
        list.set_margin_bottom(12);
        let pt = app.pt();
        for c in tinta_core::keymap::Command::ALL {
            let keys = app.keymap.label(*c).unwrap_or("").to_string();
            let row = adw::ActionRow::builder().title(c.title(pt)).subtitle(c.id()).build();
            let k = gtk::Label::new(Some(&keys));
            k.add_css_class("monospace");
            k.add_css_class("dim-label");
            row.add_suffix(&k);
            list.append(&row);
        }
        let l = list.clone();
        search.connect_search_changed(move |s| {
            let q = s.text().to_lowercase();
            let mut i = 0;
            while let Some(row) = l.row_at_index(i).and_downcast::<adw::ActionRow>() {
                let text = format!("{} {}", row.title(), row.subtitle().unwrap_or_default()).to_lowercase();
                row.set_visible(q.is_empty() || text.contains(&q));
                i += 1;
            }
        });
        col.append(&gtk::ScrolledWindow::builder().child(&list).vexpand(true).build());
        tv.set_content(Some(&col));
        d.set_child(Some(&tv));
        let s = Rc::downgrade(shell);
        d.connect_closed(move |_| {
            if let Some(s) = s.upgrade() {
                s.dispatch(Message::Dialog(DialogMsg::Cancel));
            }
        });
        d
    }

    fn sync_prefs(&self, win: &Window, shell: &Rc<Shell>, app: &App) {
        let showing = self.prefs.borrow().is_some();
        if app.settings_open && !showing {
            let d = prefs_dialog(shell, &app.settings, app);
            let s = Rc::downgrade(shell);
            d.connect_closed(move |_| {
                if let Some(s) = s.upgrade() {
                    if let Some(w) = s.window.get() {
                        w.dialogs_closed_prefs();
                    }
                    s.dispatch(Message::SettingsClose);
                }
            });
            d.present(Some(&win.window));
            *self.prefs.borrow_mut() = Some(d);
        } else if !app.settings_open && showing {
            if let Some(d) = self.prefs.borrow_mut().take() {
                d.force_close();
            }
        }
    }

    pub fn forget_prefs(&self) {
        self.prefs.borrow_mut().take();
    }
}

impl Window {
    pub fn dialogs_closed_prefs(&self) {
        self.dialogs_ref().forget_prefs();
    }
}

fn send(shell: &Rc<Shell>, m: Message) {
    if let Some(s) = Rc::downgrade(shell).upgrade() {
        s.dispatch(m);
    }
}

fn switch(shell: &Rc<Shell>, title: &'static str, on: bool, key: B) -> adw::SwitchRow {
    let r = adw::SwitchRow::builder().title(tr(title)).active(on).build();
    let s = Rc::downgrade(shell);
    r.connect_active_notify(move |r| {
        if let Some(s) = s.upgrade() {
            s.dispatch(Message::Setting(S::Bool(key, r.is_active())));
        }
    });
    r
}

fn combo<T: Copy + PartialEq + 'static>(shell: &Rc<Shell>, title: &'static str, options: &[(&'static str, T)], value: T, to_msg: fn(T) -> S) -> adw::ComboRow {
    let labels: Vec<&str> = options.iter().map(|(l, _)| tr(l)).collect();
    let r = adw::ComboRow::builder().title(tr(title)).model(&gtk::StringList::new(&labels)).build();
    if let Some(i) = options.iter().position(|(_, v)| *v == value) {
        r.set_selected(i as u32);
    }
    let values: Vec<T> = options.iter().map(|(_, v)| *v).collect();
    let s = Rc::downgrade(shell);
    r.connect_selected_notify(move |r| {
        if let (Some(s), Some(v)) = (s.upgrade(), values.get(r.selected() as usize)) {
            s.dispatch(Message::Setting(to_msg(*v)));
        }
    });
    r
}

fn spin(shell: &Rc<Shell>, title: &'static str, value: f64, min: f64, max: f64, step: f64, to_msg: fn(f64) -> S) -> adw::SpinRow {
    let r = adw::SpinRow::with_range(min, max, step);
    r.set_title(tr(title));
    r.set_value(value);
    let s = Rc::downgrade(shell);
    r.connect_value_notify(move |r| {
        if let Some(s) = s.upgrade() {
            s.dispatch(Message::Setting(to_msg(r.value())));
        }
    });
    r
}

fn prefs_dialog(shell: &Rc<Shell>, st: &Settings, app: &App) -> adw::PreferencesDialog {
    let d = adw::PreferencesDialog::new();
    d.set_search_enabled(true);

    let page = adw::PreferencesPage::builder().title(tr("Appearance")).icon_name("applications-graphics-symbolic").build();
    let look = adw::PreferencesGroup::new();
    look.add(&combo(shell, "Theme", &[("Follow system", AppTheme::System), ("Dark", AppTheme::Dark), ("Light", AppTheme::Light)], st.ui_theme, S::UiTheme));
    let themes: Vec<&str> = app.syntax_theme_names.iter().map(String::as_str).collect();
    let syntax = adw::ComboRow::builder().title(tr("Syntax colors")).model(&gtk::StringList::new(&themes)).build();
    if let Some(i) = app.syntax_theme_names.iter().position(|n| *n == st.syntax_theme) {
        syntax.set_selected(i as u32);
    }
    let names = app.syntax_theme_names.clone();
    let s = Rc::downgrade(shell);
    syntax.connect_selected_notify(move |r| {
        if let (Some(s), Some(n)) = (s.upgrade(), names.get(r.selected() as usize)) {
            s.dispatch(Message::Setting(S::SyntaxTheme(n.clone())));
        }
    });
    look.add(&syntax);
    let mut fonts = vec![tr("System monospace font").to_string()];
    fonts.extend(app.font_names.iter().cloned());
    let font_refs: Vec<&str> = fonts.iter().map(String::as_str).collect();
    let font = adw::ComboRow::builder().title(tr("Font")).model(&gtk::StringList::new(&font_refs)).enable_search(true).build();
    let default_font = Settings::default().font_family;
    font.set_selected(if st.font_family == default_font { 0 } else { app.font_names.iter().position(|n| *n == st.font_family).map_or(0, |i| i as u32 + 1) });
    let names = app.font_names.clone();
    let s = Rc::downgrade(shell);
    font.connect_selected_notify(move |r| {
        let Some(s) = s.upgrade() else { return };
        let f = match r.selected() {
            0 => default_font.clone(),
            i => names.get(i as usize - 1).cloned().unwrap_or_else(|| default_font.clone()),
        };
        s.dispatch(Message::Setting(S::FontFamily(f)));
    });
    look.add(&font);
    look.add(&spin(shell, "Font size", st.font_size as f64, 6.0, 48.0, 1.0, |v| S::FontSize(v as f32)));
    look.add(&spin(shell, "Line height", (st.line_height * 100.0) as f64, 100.0, 250.0, 5.0, |v| S::LineHeight(v as f32 / 100.0)));
    look.add(&combo(shell, "Line numbers", &[("On", LineNumbers::On), ("Relative", LineNumbers::Relative), ("Off", LineNumbers::Off)], st.line_numbers, S::LineNumbers));
    look.add(&combo(
        shell,
        "Show whitespace",
        &[("Never", Whitespace::None), ("In selection", Whitespace::Selection), ("At boundaries", Whitespace::Boundary), ("Always", Whitespace::All)],
        st.render_whitespace,
        S::Whitespace,
    ));
    look.add(&combo(shell, "Cursor style", &[("Bar", CursorStyle::Bar), ("Block", CursorStyle::Block), ("Underline", CursorStyle::Underline)], st.cursor_style, S::CursorStyle));
    for (t, v, k) in [
        ("Blinking cursor", st.cursor_blink, B::CursorBlink),
        ("Minimap", st.minimap, B::Minimap),
        ("Highlight current line", st.highlight_current_line, B::CurrentLine),
        ("Rainbow brackets", st.bracket_colors, B::BracketColors),
        ("Indent guides", st.indent_guides, B::IndentGuides),
        ("Sticky scroll", st.sticky_scroll, B::StickyScroll),
        ("Highlight occurrences", st.occurrences_highlight, B::Occurrences),
    ] {
        look.add(&switch(shell, t, v, k));
    }
    page.add(&look);
    d.add(&page);

    let page = adw::PreferencesPage::builder().title(tr("Editor")).icon_name("document-edit-symbolic").build();
    let ed = adw::PreferencesGroup::new();
    ed.add(&spin(shell, "Tab size", st.tab_size as f64, 1.0, 16.0, 1.0, |v| S::TabSize(v as usize)));
    for (t, v, k) in [
        ("Insert spaces", st.insert_spaces, B::InsertSpaces),
        ("Detect indentation", st.detect_indentation, B::DetectIndent),
        ("Word wrap", st.word_wrap, B::WordWrap),
        ("Vim mode", st.vim_mode, B::Vim),
        ("Code folding", st.folding, B::Folding),
        ("Auto close brackets", st.auto_pairs, B::AutoPairs),
        ("Auto indent", st.auto_indent, B::AutoIndent),
    ] {
        ed.add(&switch(shell, t, v, k));
    }
    page.add(&ed);
    let files = adw::PreferencesGroup::builder().title(tr("Files")).build();
    files.add(&combo(shell, "Auto save", &[("Off", AutoSave::Off), ("After a delay", AutoSave::Delay), ("When focus leaves", AutoSave::Focus)], st.auto_save, S::AutoSave));
    for (t, v, k) in [
        ("Format on save", st.format_on_save, B::FormatOnSave),
        ("Trim trailing whitespace on save", st.trim_trailing_whitespace, B::TrimTrailing),
        ("Final newline on save", st.insert_final_newline, B::FinalNewline),
        ("Restore session", st.restore_session, B::RestoreSession),
        ("Keep unsaved work between runs", st.hot_exit, B::HotExit),
        ("Show hidden files", st.show_hidden_files, B::ShowHidden),
    ] {
        files.add(&switch(shell, t, v, k));
    }
    page.add(&files);
    d.add(&page);

    let page = adw::PreferencesPage::builder().title(tr("Code intelligence")).icon_name("applications-engineering-symbolic").build();
    let code = adw::PreferencesGroup::new();
    for (t, v, k) in [
        ("Language servers", st.lsp_enabled, B::Lsp),
        ("Suggestions while typing", st.auto_complete, B::AutoComplete),
        ("Word suggestions", st.word_completion, B::WordCompletion),
        ("Snippets", st.snippets, B::Snippets),
        ("Git marks in the gutter", st.git_gutter, B::GitGutter),
        ("Git blame on the current line", st.inline_blame, B::InlineBlame),
    ] {
        code.add(&switch(shell, t, v, k));
    }
    page.add(&code);
    let files_group = adw::PreferencesGroup::builder().title(tr("Configuration Files")).build();
    for (t, c) in [
        ("settings.json", tinta_core::keymap::Command::OpenSettingsJson),
        ("keybindings.json", tinta_core::keymap::Command::OpenKeybindings),
        ("snippets.json", tinta_core::keymap::Command::OpenSnippets),
    ] {
        let r = adw::ActionRow::builder().title(t).activatable(true).build();
        r.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let s = shell.clone();
        r.connect_activated(move |_| send(&s, Message::Cmd(c)));
        files_group.add(&r);
    }
    page.add(&files_group);
    d.add(&page);
    let _ = InterfaceStyle::Auto;
    d
}
