//! The menu bar and right-click menus: COSMIC's own menu widgets, fed from the command table.

use crate::app::{App, ExplorerMsg, Message, RowAction};
use crate::i18n::tr;
use cosmic::Element;
use cosmic::iced::keyboard::key::Named;
use cosmic::iced::keyboard::Key;
use cosmic::widget::menu::key_bind::Modifier;
use cosmic::widget::menu::{self, Item, KeyBind};
use std::collections::HashMap;
use tinta_core::keymap::{Command, Stroke};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Act(pub Command);

impl menu::Action for Act {
    type Message = Message;
    fn message(&self) -> Message {
        Message::Cmd(self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct RowAct(pub usize, pub RowAction);

impl menu::Action for RowAct {
    type Message = Message;
    fn message(&self) -> Message {
        Message::Explorer(ExplorerMsg::RowMenu(self.0, self.1))
    }
}

fn key_of(name: &str) -> Key {
    let n = match name {
        "enter" => Named::Enter,
        "tab" => Named::Tab,
        "space" => return Key::Character(" ".into()),
        "backspace" => Named::Backspace,
        "delete" => Named::Delete,
        "insert" => Named::Insert,
        "up" => Named::ArrowUp,
        "down" => Named::ArrowDown,
        "left" => Named::ArrowLeft,
        "right" => Named::ArrowRight,
        "home" => Named::Home,
        "end" => Named::End,
        "pageup" => Named::PageUp,
        "pagedown" => Named::PageDown,
        "f1" => Named::F1,
        "f2" => Named::F2,
        "f3" => Named::F3,
        "f4" => Named::F4,
        "f5" => Named::F5,
        "f6" => Named::F6,
        "f7" => Named::F7,
        "f8" => Named::F8,
        "f9" => Named::F9,
        "f10" => Named::F10,
        "f11" => Named::F11,
        "f12" => Named::F12,
        other => return Key::Character(other.into()),
    };
    Key::Named(n)
}

/// The shortcut shown next to each entry. Chords don't fit KeyBind, so those go without.
fn binds(app: &App) -> HashMap<KeyBind, Act> {
    let mut out = HashMap::new();
    for c in Command::ALL {
        let Some(label) = app.keymap.label(*c) else { continue };
        if label.contains(' ') {
            continue;
        }
        let Some(s) = Stroke::parse(label) else { continue };
        let mut modifiers = Vec::new();
        if s.logo {
            modifiers.push(Modifier::Super);
        }
        if s.ctrl {
            modifiers.push(Modifier::Ctrl);
        }
        if s.alt {
            modifiers.push(Modifier::Alt);
        }
        if s.shift {
            modifiers.push(Modifier::Shift);
        }
        out.insert(KeyBind { modifiers, key: key_of(&s.key) }, Act(*c));
    }
    out
}

static MENU_BAR: std::sync::LazyLock<cosmic::widget::Id> = std::sync::LazyLock::new(|| cosmic::widget::Id::new("menu-bar"));

fn menu_trees(app: &App) -> Vec<(&'static str, Vec<Item<Act, &'static str>>)> {
    let pt = app.pt();
    let b = |c: Command| Item::Button(c.title(pt), None, Act(c));
    let check = |c: Command, on: bool| Item::CheckBox(c.title(pt), None, on, Act(c));
    let d = || Item::Divider;
    let s = &app.settings;
    use Command as C;
    let mut recent_or_open = vec![b(C::NewFile), b(C::OpenFile), b(C::OpenFolder), b(C::OpenRecent), d()];
    recent_or_open.extend([
        b(C::Save),
        b(C::SaveAs),
        b(C::SaveAll),
        b(C::Revert),
        b(C::CompareSaved),
        d(),
        b(C::CloseTab),
        b(C::CloseOtherTabs),
        b(C::CloseAllTabs),
        b(C::ReopenTab),
        b(C::PinTab),
        b(C::CloseFolder),
        d(),
        b(C::OpenSettings),
        Item::Folder(
            tr("Configuration Files"),
            vec![b(C::OpenSettingsJson), b(C::OpenKeybindings), b(C::OpenSnippets)],
        ),
        d(),
        b(C::Quit),
    ]);
    let tree = |label: &'static str, items| (label, items);
    let trees = vec![
        tree(tr("File"), recent_or_open),
        tree(
            tr("Edit"),
            vec![
                b(C::Undo),
                b(C::Redo),
                d(),
                b(C::Cut),
                b(C::Copy),
                b(C::Paste),
                d(),
                b(C::Find),
                b(C::Replace),
                b(C::FindInFiles),
                b(C::ReplaceInFiles),
                d(),
                b(C::ToggleComment),
                b(C::ToggleBlockComment),
                b(C::InsertSnippet),
                d(),
                Item::Folder(
                    if pt { "Linhas" } else { "Lines" },
                    vec![
                        b(C::DuplicateSelection),
                        b(C::CopyLineUp),
                        b(C::CopyLineDown),
                        b(C::MoveLineUp),
                        b(C::MoveLineDown),
                        b(C::DeleteLine),
                        b(C::JoinLines),
                        d(),
                        b(C::SortAsc),
                        b(C::SortDesc),
                        b(C::SortCaseless),
                        b(C::ReverseLines),
                        b(C::UniqueLines),
                        b(C::ShuffleLines),
                        b(C::TrimTrailing),
                    ],
                ),
                Item::Folder(
                    if pt { "Transformar" } else { "Transform" },
                    vec![
                        b(C::UpperCase),
                        b(C::LowerCase),
                        b(C::TitleCase),
                        b(C::SnakeCase),
                        b(C::CamelCase),
                        b(C::PascalCase),
                        b(C::KebabCase),
                        b(C::Transpose),
                    ],
                ),
            ],
        ),
        tree(
            tr("Selection"),
            vec![
                b(C::SelectAll),
                b(C::SelectLine),
                b(C::ExpandSelection),
                b(C::ShrinkSelection),
                d(),
                b(C::AddCursorAbove),
                b(C::AddCursorBelow),
                b(C::SplitIntoLines),
                d(),
                b(C::AddNextOccurrence),
                b(C::SkipOccurrence),
                b(C::SelectAllOccurrences),
            ],
        ),
        tree(
            tr("View"),
            vec![
                b(C::CommandPalette),
                b(C::ChangeTheme),
                d(),
                check(C::ToggleSidebar, app.sidebar.is_some()),
                b(C::ShowExplorer),
                b(C::ShowSearch),
                b(C::ShowGit),
                b(C::ShowOutline),
                d(),
                check(C::TogglePanel, app.panel.is_some()),
                b(C::ShowProblems),
                b(C::SplitRight),
                b(C::CloseGroup),
                d(),
                check(C::ToggleWordWrap, s.word_wrap),
                check(C::ToggleMinimap, s.minimap),
                check(C::ToggleStickyScroll, s.sticky_scroll),
                check(C::ToggleBlame, s.inline_blame),
                check(C::ToggleVim, s.vim_mode),
                b(C::ToggleWhitespace),
                b(C::ToggleLineNumbers),
                b(C::MarkdownPreview),
                d(),
                b(C::Fold),
                b(C::Unfold),
                b(C::FoldAll),
                b(C::UnfoldAll),
                d(),
                b(C::ZoomIn),
                b(C::ZoomOut),
                b(C::ZoomReset),
                check(C::ToggleFullscreen, app.fullscreen),
                check(C::ToggleZen, app.zen),
            ],
        ),
        tree(
            tr("Go"),
            vec![
                b(C::QuickOpen),
                b(C::GoToLine),
                b(C::GoToSymbol),
                d(),
                b(C::GoBack),
                b(C::GoForward),
                b(C::JumpToBracket),
                d(),
                b(C::NextTab),
                b(C::PrevTab),
                b(C::FocusNextGroup),
                b(C::FocusPrevGroup),
                d(),
                b(C::NextDiagnostic),
                b(C::PrevDiagnostic),
                b(C::NextChange),
                b(C::PrevChange),
                d(),
                b(C::ToggleBookmark),
                b(C::NextBookmark),
                b(C::PrevBookmark),
                b(C::ClearBookmarks),
            ],
        ),
        tree(
            tr("Code"),
            vec![
                b(C::TriggerCompletion),
                b(C::ParameterHints),
                b(C::ShowHover),
                d(),
                b(C::GoToDefinition),
                b(C::GoToDeclaration),
                b(C::GoToImplementation),
                b(C::GoToTypeDefinition),
                b(C::FindReferences),
                d(),
                b(C::Rename),
                b(C::FormatDocument),
                b(C::CodeAction),
                d(),
                b(C::ChangeLanguage),
                b(C::ChangeIndentSize),
                b(C::ChangeEncoding),
                b(C::RestartLsp),
            ],
        ),
        tree(
            "Git",
            vec![
                b(C::GitCommit),
                b(C::GitStageAll),
                b(C::GitPush),
                b(C::GitPull),
                b(C::GitFetch),
                b(C::GitCheckout),
                d(),
                b(C::GitDiffFile),
                b(C::GitLog),
                b(C::RevertHunk),
                b(C::GitRefresh),
            ],
        ),
        tree(
            tr("Terminal"),
            vec![
                b(C::ToggleTerminal),
                b(C::NewTerminal),
                d(),
                b(C::RunBuild),
                b(C::RunTask),
                b(C::RerunTask),
                b(C::StopTask),
                b(C::ShowTasks),
                b(C::NextTaskProblem),
                b(C::PrevTaskProblem),
            ],
        ),
        tree(
            tr("Debug"),
            vec![
                b(C::DebugStart),
                b(C::DebugStop),
                b(C::DebugRestart),
                b(C::DebugPause),
                d(),
                b(C::StepOver),
                b(C::StepInto),
                b(C::StepOut),
                d(),
                b(C::ToggleBreakpoint),
                b(C::BreakpointCondition),
                b(C::RemoveBreakpoints),
                d(),
                b(C::ShowDebug),
                b(C::ShowDebugConsole),
                b(C::DebugSelectConfig),
                b(C::DebugConfigure),
            ],
        ),
        tree(tr("Help"), vec![b(C::ShowShortcuts), b(C::About)]),
    ];
    trees
}

pub fn menu_bar(app: &App) -> Element<'_, Message> {
    let keys = binds(app);
    let trees = menu_trees(app);
    let bar = cosmic::widget::responsive_menu_bar()
        .item_height(menu::ItemHeight::Dynamic(36))
        .item_width(menu::ItemWidth::Uniform(360))
        .spacing(2.0)
        .into_element(&app.core, &keys, MENU_BAR.clone(), Message::Surface, trees);
    // Menu roots paint their label in the inherited icon colour, and with the content container
    // off nothing above sets one: without this the labels are invisible.
    header_colors(bar.into())
}

/// Menu roots and the title paint in the inherited colours, and with the content container off
/// nothing above the header bar sets them: without this they're invisible.
pub fn header_colors(el: Element<'_, Message>) -> Element<'_, Message> {
    cosmic::widget::container(el)
        .class(cosmic::theme::Container::custom(|t: &cosmic::Theme| {
            let on: cosmic::iced::Color = t.cosmic().background(false).on.into();
            cosmic::iced::widget::container::Style { icon_color: Some(on), text_color: Some(on), ..Default::default() }
        }))
        .into()
}

/// GNOME's primary menu: every menu behind one button at the end of the header bar.
pub fn primary_menu(app: &App) -> Element<'_, Message> {
    let keys = binds(app);
    let folders = menu_trees(app).into_iter().map(|(label, items)| Item::Folder(label, items)).collect();
    let bar = menu::bar(vec![menu::Tree::with_children(
        Element::from(cosmic::widget::button::icon(cosmic::widget::icon::from_name("open-menu-symbolic")).padding([4, 12]).class(cosmic::theme::Button::MenuRoot)),
        menu::items(&keys, folders).into_iter().map(|t| t.width(360)).collect(),
    )])
    .item_height(menu::ItemHeight::Dynamic(36))
    .item_width(menu::ItemWidth::Uniform(360))
    .spacing(2.0)
    .on_surface_action(Message::Surface)
    .window_id_maybe(app.core.main_window_id());
    header_colors(bar.into())
}

/// Right click in the text.
pub fn editor_menu(app: &App) -> Vec<menu::Tree<Message>> {
    let pt = app.pt();
    let b = |c: Command| Item::Button(c.title(pt), None, Act(c));
    let d = || Item::Divider;
    use Command as C;
    menu::items(
        &binds(app),
        vec![
            b(C::GoToDefinition),
            b(C::FindReferences),
            b(C::Rename),
            b(C::CodeAction),
            b(C::FormatDocument),
            d(),
            b(C::Cut),
            b(C::Copy),
            b(C::Paste),
            d(),
            b(C::ToggleBreakpoint),
            b(C::ToggleComment),
            b(C::AddNextOccurrence),
            b(C::SelectAllOccurrences),
            d(),
            b(C::CommandPalette),
        ],
    )
}

/// Right click in the file tree.
pub fn row_menu(index: usize, is_dir: bool) -> Vec<menu::Tree<Message>> {
    let empty: HashMap<KeyBind, RowAct> = HashMap::new();
    let b = |label: &'static str, a: RowAction| Item::Button(tr(label), None, RowAct(index, a));
    let mut items = Vec::new();
    if !is_dir {
        items.push(b("Open to the Side", RowAction::OpenSide));
    }
    items.extend([
        b("New File", RowAction::NewFile),
        b("New Folder", RowAction::NewFolder),
        Item::Divider,
        b("Rename", RowAction::Rename),
        b("Move to Trash", RowAction::Trash),
        Item::Divider,
        b("Copy Path", RowAction::CopyPath),
        b("Copy Relative Path", RowAction::CopyRelative),
        b("Reveal in File Manager", RowAction::Reveal),
        b("Open in Terminal", RowAction::Terminal),
    ]);
    menu::items(&empty, items)
}

/// A right-click menu as a popup of its own, so it isn't cut off by the window's edge.
pub fn context<'a>(app: &App, content: impl Into<Element<'a, Message>>, items: Vec<menu::Tree<Message>>) -> Element<'a, Message> {
    let mut m = cosmic::widget::context_menu(content, Some(items));
    if let Some(id) = app.core.main_window_id() {
        m = m.window_id(id).on_surface_action(Message::Surface);
    }
    m.into()
}
