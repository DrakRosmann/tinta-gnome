//! Driving the app from a file. Lets tests and screenshots go through the real update loop and
//! renderer: `TINTA_SCRIPT=steps.txt tinta-gnome`.
//!
//! Each line is a step; `#` starts a comment:
//!   wait 200                 pause, in milliseconds
//!   cmd view.split           run a command by id
//!   key ctrl+shift+p         press a key
//!   type hello               type text
//!   input hello              set the text of whatever field is open (palette, find)
//!   click 3 10               left click at row 3, visual column 10
//!   open /path/file          open a file
//!   folder /path             open a folder
//!   shot /path/out.png       save a screenshot of the window
//!   dump /path/out.json      write the editor's state, for assertions
//!   quit

use super::keys::KeyEv;
use super::{App, Message};
use crate::bus;
use crate::ui::editor_view::{EditorEvent, PressButton};
use crate::fx::{self, Task};
use crate::gfx::Modifiers;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tinta_core::keymap::{Command, Stroke};

#[derive(Debug, Clone)]
pub enum Step {
    Cmd(String),
    Key(String),
    Type(String),
    Input(String),
    Click(usize, usize, u8),
    Open(PathBuf),
    Folder(PathBuf),
    Shot(PathBuf),
    Dump(PathBuf),
    Quit,
}

pub fn start() {
    let Ok(path) = std::env::var("TINTA_SCRIPT") else { return };
    std::thread::spawn(move || {
        let Ok(src) = std::fs::read_to_string(&path) else {
            eprintln!("script: cannot read {path}");
            return;
        };
        // Give the first frame time to appear.
        std::thread::sleep(Duration::from_millis(800));
        for raw in src.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
            let step = match word {
                "wait" => {
                    std::thread::sleep(Duration::from_millis(rest.trim().parse().unwrap_or(100)));
                    continue;
                }
                "cmd" => Step::Cmd(rest.trim().into()),
                "key" => Step::Key(rest.trim().into()),
                "type" => Step::Type(rest.replace("\\n", "\n")),
                "input" => Step::Input(rest.to_string()),
                "click" => {
                    let mut it = rest.split_whitespace().filter_map(|s| s.parse::<usize>().ok());
                    Step::Click(it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(1) as u8)
                }
                "open" => Step::Open(rest.trim().into()),
                "folder" => Step::Folder(rest.trim().into()),
                "shot" => Step::Shot(rest.trim().into()),
                "dump" => Step::Dump(rest.trim().into()),
                "quit" => Step::Quit,
                other => {
                    eprintln!("script: unknown step {other}");
                    continue;
                }
            };
            bus::send(Message::Script(step));
            std::thread::sleep(Duration::from_millis(60));
        }
    });
}

fn key_event(spec: &str) -> Option<KeyEv> {
    let s = Stroke::parse(spec)?;
    let mut mods = Modifiers::empty();
    if s.ctrl {
        mods |= Modifiers::CTRL;
    }
    if s.shift {
        mods |= Modifiers::SHIFT;
    }
    if s.alt {
        mods |= Modifiers::ALT;
    }
    if s.logo {
        mods |= Modifiers::LOGO;
    }
    let text = (s.key.chars().count() == 1).then(|| s.key.clone());
    Some(KeyEv { name: s.key, shifted: None, mods, text, captured: false })
}

impl App {
    pub fn on_script(&mut self, step: Step) -> Task {
        match step {
            Step::Cmd(id) => match Command::from_id(&id) {
                Some(c) => self.run(c),
                None => {
                    eprintln!("script: unknown command {id}");
                    Task::none()
                }
            },
            Step::Key(spec) => match key_event(&spec) {
                Some(ev) => self.on_key(ev),
                None => Task::none(),
            },
            Step::Type(text) => {
                let mut tasks = Vec::new();
                for ch in text.chars() {
                    if ch == '\n' {
                        tasks.push(self.key_enter());
                    } else {
                        let ev = KeyEv {
                            name: ch.to_lowercase().to_string(),
                            shifted: None,
                            mods: Modifiers::empty(),
                            text: Some(ch.to_string()),
                            captured: false,
                        };
                        tasks.push(self.on_key(ev));
                    }
                }
                Task::batch(tasks)
            }
            Step::Input(text) => {
                if self.palette.is_some() {
                    self.on_palette(super::PaletteMsg::Input(text))
                } else if self.dialog.is_some() {
                    self.on_dialog(super::DialogMsg::Input(text))
                } else if self.sidebar == Some(super::SidebarView::Search) && !self.find.open {
                    self.on_search_msg(super::SearchMsg::Query(text))
                } else {
                    self.on_find(super::FindMsg::Query(text))
                }
            }
            Step::Click(row, vcol, clicks) => match self.active_tab_id() {
                Some(id) => self.on_editor_event(
                    id,
                    EditorEvent::Press { row, vcol, clicks, mods: Modifiers::empty(), button: PressButton::Left },
                ),
                None => Task::none(),
            },
            Step::Open(p) => self.open_path(&p),
            Step::Folder(p) => self.open_folder(p),
            Step::Shot(path) => Task::effect(fx::Effect::Screenshot(path)),
            Step::Dump(path) => {
                let _ = std::fs::write(path, self.dump_state().to_string());
                Task::none()
            }
            Step::Quit => {
                self.persist_on_quit();
                Task::effect(fx::Effect::Quit)
            }
        }
    }

    /// The state tests assert on.
    pub fn dump_state(&self) -> serde_json::Value {
        let active = self.active().map(|(t, e)| {
            json!({
                "file": e.doc.file_name(),
                "text": e.doc.rope.to_string(),
                "dirty": e.doc.is_dirty(),
                "language": e.doc.language.name,
                "cursors": t.cursors.iter().map(|s| json!([s.anchor, s.head])).collect::<Vec<_>>(),
                "folds": t.folds.ranges,
                "scroll_y": t.scroll_y,
                "rows": t.map.row_count(&e.doc),
                "matches": e.matches.len(),
                "diagnostics": e.diags.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
                "git_marks": e.git_diff.marks.len(),
                "bookmarks": e.bookmarks.iter().copied().collect::<Vec<_>>(),
                "highlighted": e.doc.highlighter.as_ref().map(|h| h.frontier()),
            })
        });
        json!({
            "active": active,
            "tabs": self.tabs.len(),
            "groups": self.groups.iter().map(|g| g.tabs().len()).collect::<Vec<_>>(),
            "debug": {
                "state": self.debug.session.as_ref().map(|s| format!("{:?}", s.state)),
                "frames": self.debug.session.as_ref().map(|s| s.frames.iter().map(|f| json!([f.name, f.line])).collect::<Vec<_>>()),
                "variables": self.debug.session.as_ref().map(|s| s.variable_rows().iter().map(|r| json!([r.path, r.value])).collect::<Vec<_>>()),
                "breakpoints": self.debug.breakpoints.iter().map(|(p, l)| json!([p, l.iter().map(|b| json!([b.line, format!("{:?}", b.state)])).collect::<Vec<_>>()])).collect::<Vec<_>>(),
                "console": self.debug.console.iter().map(|l| l.text.clone()).collect::<Vec<_>>(),
            },
            "title": self.title,
            "sidebar": self.sidebar.map(|s| format!("{s:?}")),
            "panel": self.panel.map(|s| format!("{s:?}")),
            "palette": self.palette.as_ref().map(|p| json!({
                "query": p.query,
                "items": p.items.iter().map(|i| i.label.clone()).take(30).collect::<Vec<_>>(),
                "selected": p.selected,
            })),
            "completion": self.completion.as_ref().map(|c| c.visible_labels()),
            "hover": self.hover.as_ref().map(|h| h.text.clone()),
            "search_rows": self.search.rows.len(),
            "git": { "branch": self.git.branch, "entries": self.git.entries.len() },
            "explorer_rows": self.explorer.rows.len(),
            "terminals": self.terms.len(),
            "toast": self.toast,
            "folder": self.folder,
            "focus": format!("{:?}", self.focus),
            "font_size": self.settings.font_size,
            "cell_w": self.metrics.cell_w,
            "lsp": self.lsp.keys().collect::<Vec<_>>(),
        })
    }
}
