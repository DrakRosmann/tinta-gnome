//! The status bar: branch, problems, language server progress, caret position, file settings.

use super::Shell;
use crate::app::{App, Focus, Message, PanelView, SidebarView};
use crate::i18n::tr;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use tinta_core::keymap::Command;
use tinta_core::lsp::Severity;

#[derive(Clone, Copy, PartialEq)]
enum Act {
    None,
    Cmd(Command),
    Panel(PanelView),
    Sidebar(SidebarView),
}

#[derive(Clone, PartialEq)]
struct Item {
    icon: Option<&'static str>,
    text: String,
    act: Act,
    dim: bool,
}

pub struct StatusBar {
    pub root: gtk::Box,
    start: gtk::Box,
    end: gtk::Box,
    shell: Weak<Shell>,
    last: RefCell<(Vec<Item>, Vec<Item>)>,
}

fn item(text: impl Into<String>, act: Act) -> Item {
    Item { icon: None, text: text.into(), act, dim: false }
}

fn icon_item(icon: &'static str, text: impl Into<String>, act: Act) -> Item {
    Item { icon: Some(icon), text: text.into(), act, dim: false }
}

fn dim(text: impl Into<String>) -> Item {
    Item { icon: None, text: text.into(), act: Act::None, dim: true }
}

impl StatusBar {
    pub fn build(shell: &Rc<Shell>) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        root.add_css_class("tinta-status");
        let start = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let end = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        start.set_hexpand(true);
        root.append(&start);
        root.append(&end);
        StatusBar { root, start, end, shell: Rc::downgrade(shell), last: RefCell::new((Vec::new(), Vec::new())) }
    }

    fn widget(&self, it: &Item) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        if let Some(i) = it.icon {
            content.append(&gtk::Image::from_icon_name(i));
        }
        let label = gtk::Label::new(Some(&it.text));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(60);
        content.append(&label);
        if it.dim {
            content.add_css_class("dim-label");
        }
        if it.act == Act::None {
            content.set_margin_start(8);
            content.set_margin_end(8);
            return content.upcast();
        }
        let b = gtk::Button::builder().child(&content).build();
        b.add_css_class("flat");
        let s = self.shell.clone();
        let act = it.act;
        b.connect_clicked(move |_| {
            let Some(s) = s.upgrade() else { return };
            s.dispatch(match act {
                Act::Cmd(c) => Message::Cmd(c),
                Act::Panel(p) => Message::PanelShow(p),
                Act::Sidebar(v) => Message::SidebarShow(v),
                Act::None => return,
            });
        });
        b.upcast()
    }

    pub fn sync(&self, app: &App) {
        let mut start = Vec::new();
        if app.settings.vim_mode {
            start.push(item(app.vim.status(), Act::None));
        }
        if let Some(b) = &app.git.branch {
            let mut label = b.clone();
            if let Some((ahead, behind)) = app.git.ahead_behind
                && (ahead > 0 || behind > 0)
            {
                label.push_str(&format!(" ↑{ahead} ↓{behind}"));
            }
            start.push(icon_item("tinta-git-symbolic", label, Act::Cmd(Command::GitCheckout)));
        }
        let (mut errors, mut warnings) = (0, 0);
        for d in app.project_diags.values().flatten() {
            match d.severity {
                Severity::Error => errors += 1,
                Severity::Warning => warnings += 1,
                _ => {}
            }
        }
        start.push(icon_item("dialog-error-symbolic", errors.to_string(), Act::Panel(PanelView::Problems)));
        start.push(icon_item("dialog-warning-symbolic", warnings.to_string(), Act::Panel(PanelView::Problems)));
        if let Some(p) = &app.lsp_progress {
            start.push(dim(p.clone()));
        }
        if let Some(s) = app.debug.session.as_ref().filter(|s| s.is_live()) {
            let state = if s.is_stopped() { tr("Paused") } else { tr("Running") };
            start.push(icon_item("tinta-debug-symbolic", format!("{} · {state}", s.name), Act::Sidebar(SidebarView::Debug)));
        }
        // A firmware project: the board, and the serial port with its speed.
        if let Some(p) = &app.embedded.current {
            start.push(icon_item("media-flash-symbolic", p.summary(), Act::Cmd(Command::EmbeddedBoard)));
            let port = p.port.as_deref().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).unwrap_or_else(|| tr("auto").to_string());
            let label = if app.narrow { port } else { format!("{port} · {}", p.baud) };
            start.push(icon_item("drive-harddisk-usb-symbolic", label, Act::Cmd(Command::EmbeddedPort)));
        }
        if let Some(k) = app.keymap.pending_label() {
            start.push(item(format!("{k} …"), Act::None));
        }

        let mut end = Vec::new();
        let focus_terminal = app.focus == Focus::Terminal && app.panel == Some(PanelView::Terminal);
        if let Some(d) = app.active().and_then(|(t, _)| t.diff.as_ref()) {
            end.push(item(format!("+{} −{}", d.added, d.removed), Act::None));
            let pos = match d.current {
                Some(c) => format!("{} {} {}", c + 1, tr("of"), d.hunks.len()),
                None => format!("{}", d.hunks.len()),
            };
            end.push(item(pos, Act::Cmd(Command::NextChange)));
            end.push(item(d.right.language.name.clone(), Act::None));
        } else if let Some((t, e)) = app.active()
            && !focus_terminal
        {
            let d = &e.doc;
            let p = t.cursors.primary();
            let (line, col) = d.line_col(p.head);
            let mut pos = format!("{} {}, {} {}", tr("Ln"), line + 1, tr("Col"), col + 1);
            if t.cursors.is_multi() {
                pos.push_str(&format!("  ({} {})", t.cursors.len(), tr("cursors")));
            } else if !p.is_empty() {
                pos.push_str(&format!("  ({} {})", p.end() - p.start(), tr("selected")));
            }
            end.push(item(pos, Act::Cmd(Command::GoToLine)));
            // A narrow window keeps where the cursor is and the language; the rest is in the palette.
            if !app.narrow {
                let indent = if d.indent.spaces { format!("{}: {}", tr("Spaces"), d.indent.width) } else { format!("{}: {}", tr("Tab Size"), d.indent.width) };
                end.push(item(indent, Act::Cmd(Command::ChangeIndentSize)));
                end.push(item(d.encoding.name().to_string(), Act::Cmd(Command::ChangeEncoding)));
                let eol = if d.eol == tinta_core::buffer::LineEnding::Lf { Command::SetCrlf } else { Command::SetLf };
                end.push(item(d.eol.label().to_string(), Act::Cmd(eol)));
            }
            end.push(item(d.language.name.clone(), Act::Cmd(Command::ChangeLanguage)));
            if let Some(k) = e.lsp_key.as_ref().filter(|_| !app.narrow) {
                end.push(dim(k.split('@').next().unwrap_or("").to_string()));
            }
            if d.readonly {
                end.push(dim(if app.pt() { "somente leitura" } else { "read-only" }));
            }
        }
        let mut last = self.last.borrow_mut();
        if last.0 != start {
            while let Some(c) = self.start.first_child() {
                self.start.remove(&c);
            }
            for it in &start {
                self.start.append(&self.widget(it));
            }
            last.0 = start;
        }
        if last.1 != end {
            while let Some(c) = self.end.first_child() {
                self.end.remove(&c);
            }
            for it in &end {
                self.end.append(&self.widget(it));
            }
            last.1 = end;
        }
    }
}
