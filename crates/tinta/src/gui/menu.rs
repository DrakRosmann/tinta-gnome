//! The primary menu, the editor's context menu, and the action every menu item runs: a command
//! from the editor's own table, by id.

use super::Shell;
use crate::app::{App, Message};
use crate::i18n::{is_portuguese, tr};
use adw::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;
use tinta_core::keymap::{Command, Keymap, Stroke};

/// `ctrl+shift+p` as GTK writes accelerators: `<Control><Shift>p`.
pub fn accel(label: &str) -> Option<String> {
    if label.contains(' ') {
        return None;
    }
    let s = Stroke::parse(label)?;
    let mut out = String::new();
    if s.ctrl {
        out.push_str("<Control>");
    }
    if s.shift {
        out.push_str("<Shift>");
    }
    if s.alt {
        out.push_str("<Alt>");
    }
    if s.logo {
        out.push_str("<Super>");
    }
    let key = match s.key.as_str() {
        "enter" => "Return",
        "tab" => "Tab",
        "space" => "space",
        "backspace" => "BackSpace",
        "delete" => "Delete",
        "insert" => "Insert",
        "escape" => "Escape",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" => "Page_Up",
        "pagedown" => "Page_Down",
        "/" => "slash",
        "\\" => "backslash",
        "." => "period",
        "," => "comma",
        ";" => "semicolon",
        "'" => "apostrophe",
        "`" => "grave",
        "[" => "bracketleft",
        "]" => "bracketright",
        "=" => "equal",
        "-" => "minus",
        k if k.starts_with('f') && k.len() > 1 && k[1..].parse::<u8>().is_ok() => {
            out.push('F');
            out.push_str(&k[1..]);
            return Some(out);
        }
        k => k,
    };
    out.push_str(key);
    Some(out)
}

/// A menu item for a command, with its shortcut shown.
pub fn item(c: Command, keymap: Option<&Keymap>) -> gio::MenuItem {
    let it = gio::MenuItem::new(Some(c.title(is_portuguese())), None);
    it.set_action_and_target_value(Some("app.cmd"), Some(&c.id().to_variant()));
    if let Some(a) = keymap.and_then(|k| k.label(c)).and_then(accel) {
        it.set_attribute_value("accel", Some(&a.to_variant()));
    }
    it
}

fn section(km: &Keymap, cmds: &[Command]) -> gio::Menu {
    let m = gio::Menu::new();
    for c in cmds {
        m.append_item(&item(*c, Some(km)));
    }
    m
}

fn submenu(km: &Keymap, sections: &[&[Command]]) -> gio::Menu {
    let m = gio::Menu::new();
    for s in sections {
        m.append_section(None, &section(km, s));
    }
    m
}

pub fn main_menu(app: &App) -> gio::Menu {
    use Command as C;
    let km = &app.keymap;
    let menu = gio::Menu::new();
    let files = gio::Menu::new();
    files.append_section(None, &section(km, &[C::NewFile, C::OpenFile, C::OpenFolder, C::OpenRecent]));
    files.append_section(None, &section(km, &[C::Save, C::SaveAs, C::SaveAll, C::Revert, C::CompareSaved]));
    files.append_section(None, &section(km, &[C::CloseTab, C::CloseOtherTabs, C::CloseAllTabs, C::ReopenTab, C::PinTab, C::CloseFolder]));
    let config = section(km, &[C::OpenSettingsJson, C::OpenKeybindings, C::OpenSnippets]);
    let s = section(km, &[C::OpenSettings]);
    s.append_submenu(Some(tr("Configuration Files")), &config);
    files.append_section(None, &s);
    menu.append_submenu(Some(tr("File")), &files);
    menu.append_submenu(
        Some(tr("Edit")),
        &submenu(
            km,
            &[
                &[C::Undo, C::Redo],
                &[C::Cut, C::Copy, C::Paste],
                &[C::Find, C::Replace, C::FindInFiles, C::ReplaceInFiles],
                &[C::ToggleComment, C::ToggleBlockComment, C::InsertSnippet],
                &[C::DuplicateSelection, C::CopyLineUp, C::CopyLineDown, C::MoveLineUp, C::MoveLineDown, C::DeleteLine, C::JoinLines],
                &[C::SortAsc, C::SortDesc, C::SortCaseless, C::ReverseLines, C::UniqueLines, C::ShuffleLines, C::TrimTrailing],
                &[C::UpperCase, C::LowerCase, C::TitleCase, C::SnakeCase, C::CamelCase, C::PascalCase, C::KebabCase, C::Transpose],
            ],
        ),
    );
    menu.append_submenu(
        Some(tr("Selection")),
        &submenu(
            km,
            &[
                &[C::SelectAll, C::SelectLine, C::ExpandSelection, C::ShrinkSelection],
                &[C::AddCursorAbove, C::AddCursorBelow, C::SplitIntoLines],
                &[C::AddNextOccurrence, C::SkipOccurrence, C::SelectAllOccurrences],
            ],
        ),
    );
    menu.append_submenu(
        Some(tr("View")),
        &submenu(
            km,
            &[
                &[C::CommandPalette, C::ChangeTheme],
                &[C::ToggleSidebar, C::ShowExplorer, C::ShowSearch, C::ShowGit, C::ShowOutline, C::ShowDebug],
                &[C::TogglePanel, C::ShowProblems, C::SplitRight, C::CloseGroup],
                &[C::ToggleWordWrap, C::ToggleMinimap, C::ToggleStickyScroll, C::ToggleBlame, C::ToggleVim, C::ToggleWhitespace, C::ToggleLineNumbers, C::MarkdownPreview],
                &[C::Fold, C::Unfold, C::FoldAll, C::UnfoldAll],
                &[C::ZoomIn, C::ZoomOut, C::ZoomReset, C::ToggleFullscreen, C::ToggleZen],
            ],
        ),
    );
    menu.append_submenu(
        Some(tr("Go")),
        &submenu(
            km,
            &[
                &[C::QuickOpen, C::GoToLine, C::GoToSymbol],
                &[C::GoBack, C::GoForward, C::JumpToBracket],
                &[C::NextTab, C::PrevTab, C::FocusNextGroup, C::FocusPrevGroup],
                &[C::NextDiagnostic, C::PrevDiagnostic, C::NextChange, C::PrevChange],
                &[C::ToggleBookmark, C::NextBookmark, C::PrevBookmark, C::ClearBookmarks],
            ],
        ),
    );
    menu.append_submenu(
        Some(tr("Code")),
        &submenu(
            km,
            &[
                &[C::TriggerCompletion, C::ParameterHints, C::ShowHover],
                &[C::GoToDefinition, C::GoToDeclaration, C::GoToImplementation, C::GoToTypeDefinition, C::FindReferences],
                &[C::Rename, C::FormatDocument, C::CodeAction],
                &[C::ChangeLanguage, C::ChangeIndentSize, C::ChangeEncoding, C::RestartLsp],
            ],
        ),
    );
    menu.append_submenu(
        Some("Git"),
        &submenu(
            km,
            &[&[C::GitCommit, C::GitStageAll, C::GitPush, C::GitPull, C::GitFetch, C::GitCheckout], &[C::GitDiffFile, C::GitLog, C::RevertHunk, C::GitRefresh]],
        ),
    );
    menu.append_submenu(
        Some(tr("Terminal")),
        &submenu(
            km,
            &[&[C::ToggleTerminal, C::NewTerminal], &[C::RunBuild, C::RunTask, C::RerunTask, C::StopTask, C::ShowTasks, C::NextTaskProblem, C::PrevTaskProblem]],
        ),
    );
    menu.append_submenu(
        Some(tr("Debug")),
        &submenu(
            km,
            &[
                &[C::DebugStart, C::DebugStop, C::DebugRestart, C::DebugPause],
                &[C::StepOver, C::StepInto, C::StepOut, C::RunToCursor],
                &[C::ToggleBreakpoint, C::BreakpointCondition, C::RemoveBreakpoints],
                &[C::ShowDebug, C::ShowDebugConsole, C::DebugSelectConfig, C::DebugConfigure],
            ],
        ),
    );
    let help = section(km, &[C::ShowShortcuts, C::About]);
    menu.append_section(None, &help);
    menu
}

/// What a right click in the text offers.
pub fn editor_menu(app: &App) -> gio::Menu {
    use Command as C;
    submenu(
        &app.keymap,
        &[
            &[C::GoToDefinition, C::FindReferences, C::Rename, C::CodeAction, C::FormatDocument],
            &[C::Cut, C::Copy, C::Paste],
            &[C::ToggleBreakpoint, C::RunToCursor, C::ToggleComment, C::AddNextOccurrence, C::SelectAllOccurrences],
            &[C::CommandPalette],
        ],
    )
}

pub fn install_actions(gapp: &adw::Application, _window: &adw::ApplicationWindow, shell: &Rc<Shell>) {
    let action = gio::SimpleAction::new("cmd", Some(glib::VariantTy::STRING));
    let s = Rc::downgrade(shell);
    action.connect_activate(move |_, param| {
        let (Some(s), Some(id)) = (s.upgrade(), param.and_then(|p| p.get::<String>())) else { return };
        if let Some(c) = Command::from_id(&id) {
            s.dispatch(Message::Cmd(c));
        }
    });
    gapp.add_action(&action);
}
