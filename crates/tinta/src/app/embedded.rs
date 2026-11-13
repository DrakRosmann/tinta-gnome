//! Microcontrollers: the board's serial port and its monitor, choosing the board, flashing, and
//! new firmware projects. Building and running go through the tasks and Ctrl+F5, debugging
//! through F5: this is what's left.

use super::{App, DialogState, Focus, Message, PAction, PItem, PaletteMode};
use crate::bus;
use crate::fx::{self, Task};
use crate::i18n::tr;
use std::path::{Path, PathBuf};
use tinta_core::embedded::{self, Kind, Need, Project, prefs, templates};
use tinta_core::keymap::Command;
use tinta_core::run::trust;

#[derive(Default)]
pub struct EmbeddedPanel {
    /// The open folder's firmware project, for the status bar and the header bar's buttons. Kept
    /// up to date as files change.
    pub current: Option<Project>,
}

#[derive(Debug, Clone)]
pub enum EmbeddedMsg {
    /// Arduino's boards: the ones plugged in, then all it knows, as (FQBN, name).
    Boards { connected: Vec<(String, String)>, all: Vec<(String, String)>, error: Option<String> },
    /// Where to create a new project, picked in the folder dialog.
    CreateIn { template: &'static str, name: String, parent: PathBuf },
}

fn find(tool: &str) -> bool {
    tinta_core::host::find(tool)
}

/// Arduino's boards from `arduino-cli board list` and `board listall`, as JSON.
fn arduino_boards() -> EmbeddedMsg {
    let run = |args: &[&str]| -> Result<serde_json::Value, String> {
        let out = tinta_core::host::command("arduino-cli", args, None, &[]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
    };
    let pairs = |boards: &serde_json::Value| -> Vec<(String, String)> {
        boards
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| Some((b["fqbn"].as_str()?.to_string(), b["name"].as_str().unwrap_or("").to_string())))
            .collect()
    };
    let mut connected = Vec::new();
    if let Ok(v) = run(&["board", "list", "--format", "json"]) {
        // Newer versions wrap the list in "detected_ports"; each port has its matching boards.
        let ports = v.get("detected_ports").cloned().unwrap_or(v);
        for p in ports.as_array().into_iter().flatten() {
            connected.extend(pairs(&p["matching_boards"]));
        }
    }
    match run(&["board", "listall", "--format", "json"]) {
        Ok(v) => EmbeddedMsg::Boards { connected, all: pairs(&v["boards"]), error: None },
        Err(e) => EmbeddedMsg::Boards { connected, all: Vec::new(), error: Some(e) },
    }
}

/// The names the project's tasks get, in Portuguese.
pub fn task_label(name: &str, pt: bool) -> String {
    if !pt {
        return name.to_string();
    }
    for (en, ptbr) in [(": build", ": compilar"), (": upload", ": gravar na placa"), (": clean", ": limpar"), (": erase flash", ": apagar a flash"), (": test", ": testes")] {
        if let Some(head) = name.strip_suffix(en) {
            return format!("{head}{ptbr}");
        }
    }
    name.to_string()
}

impl App {
    /// The folder's firmware project, if it is one. Asks first before its own embedded.json is
    /// used, like the other .tinta files; then `then` runs again.
    pub fn embedded_project(&mut self, root: &Path, then: Command) -> Result<Option<Project>, Task> {
        let own = self.project_files_ok(root, embedded::CONFIG_PATH, then)?;
        match embedded::detect(root, own) {
            Ok(p) => Ok(p),
            Err(e) => {
                self.output.push(e.clone());
                Err(self.toast(e))
            }
        }
    }

    fn firmware_project(&mut self, then: Command) -> Result<Project, Task> {
        let Some(root) = self.task_root() else { return Err(self.toast(tr("No folder open"))) };
        match self.embedded_project(&root, then)? {
            Some(p) => Ok(p),
            None => Err(self.toast(tr("This folder isn't a firmware project: open its folder, or create one with \"Embedded: New Project…\""))),
        }
    }

    /// Keeps the status bar's idea of the project in step, without asking anything.
    pub fn refresh_embedded(&mut self) {
        self.embedded.current = self.folder.clone().and_then(|root| {
            let own = !self.runner.untrusted.contains(&root) && (trust::is_trusted(&root) || !root.join(embedded::CONFIG_PATH).exists());
            embedded::detect(&root, own).ok().flatten()
        });
    }

    /// What can't be done yet, said: a program to install gets the banner with its command.
    pub fn embedded_need(&mut self, need: Need) -> Task {
        match need {
            Need::Tool { tool, hint } => {
                let language = self.active().map(|(_, e)| e.doc.language.name.clone()).unwrap_or_default();
                self.run_missing(&language, &language, &tool, hint.command)
            }
            Need::Setting(s) => {
                self.output.push(s.clone());
                self.toast(s)
            }
        }
    }

    // The serial port ---------------------------------------------------------------------------

    /// Where the serial port's choices are kept when no firmware project is open.
    pub(super) fn prefs_root(&self) -> PathBuf {
        self.folder.clone().unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
    }

    pub fn embedded_pick_port(&mut self) -> Task {
        let root = self.prefs_root();
        let current = self.embedded.current.as_ref().and_then(|p| p.port.clone()).or_else(|| prefs::load(&root).port);
        let ports = embedded::list_ports();
        let pt = self.pt();
        let mark = |on: bool| if on { "✓ " } else { "" };
        let mut items = vec![PItem {
            label: format!("{}{}", mark(current.is_none()), tr("Automatic: the first port plugged in")),
            detail: String::new(),
            hint: String::new(),
            positions: Vec::new(),
            action: PAction::SerialPort(None),
        }];
        for p in &ports {
            let mut detail = p.device.clone();
            if !p.description.is_empty() {
                detail = format!("{} · {detail}", p.description);
            }
            if !p.accessible {
                detail.push_str(if pt { " · sem permissão" } else { " · no permission" });
            }
            let label = p.path.rsplit('/').next().unwrap_or(&p.path).to_string();
            items.push(PItem { label: format!("{}{label}", mark(current.as_deref() == Some(p.path.as_str()))), detail, hint: String::new(), positions: Vec::new(), action: PAction::SerialPort(Some(p.path.clone())) });
        }
        let empty = if ports.is_empty() { self.toast(tr("No serial port found: plug the board in")) } else { Task::none() };
        Task::batch([empty, self.open_palette_with(PaletteMode::Locations, items, Vec::new())])
    }

    fn save_prefs(&mut self, change: impl FnOnce(&mut prefs::Prefs)) -> Task {
        let root = self.prefs_root();
        let mut p = prefs::load(&root);
        change(&mut p);
        if let Err(e) = prefs::save(&root, &p) {
            return self.toast(e);
        }
        self.refresh_embedded();
        Task::none()
    }

    pub fn set_serial_port(&mut self, port: Option<String>) -> Task {
        let t = self.save_prefs(|p| p.port = port.clone());
        let Some(port) = port else { return Task::batch([t, self.serial_follow_choice()]) };
        // Linux lets only some groups open serial ports: say which, and how to join.
        let accessible = embedded::list_ports().iter().find(|p| p.path == port).is_none_or(|p| p.accessible);
        if !accessible {
            let cmd = embedded::port_permission_command();
            let text = if self.pt() {
                format!("Sem permissão para abrir {port}: rode \"{cmd}\" e entre de novo na sessão")
            } else {
                format!("No permission to open {port}: run \"{cmd}\" and log in again")
            };
            self.output.push(text.clone());
            return Task::batch([t, fx::clipboard_write(cmd), self.toast(text)]);
        }
        let follow = self.serial_follow_choice();
        Task::batch([t, follow, self.toast(format!("{}: {port}", tr("Serial port")))])
    }

    pub fn embedded_pick_baud(&mut self) -> Task {
        let current = self.embedded.current.as_ref().map(|p| p.baud).or_else(|| prefs::load(&self.prefs_root()).baud).unwrap_or(embedded::DEFAULT_BAUD);
        let items = embedded::BAUDS
            .iter()
            .rev()
            .map(|b| PItem {
                label: format!("{}{b}", if *b == current { "✓ " } else { "" }),
                detail: String::new(),
                hint: String::new(),
                positions: Vec::new(),
                action: PAction::SerialBaud(*b),
            })
            .collect();
        self.open_palette_with(PaletteMode::Locations, items, Vec::new())
    }

    pub fn set_serial_baud(&mut self, baud: u32) -> Task {
        let t = self.save_prefs(|p| p.baud = Some(baud));
        let follow = self.serial_follow_choice();
        Task::batch([t, follow, self.toast(format!("{}: {baud}", tr("Baud rate")))])
    }

    // The board ----------------------------------------------------------------------------------

    pub fn embedded_pick_board(&mut self) -> Task {
        let p = match self.firmware_project(Command::EmbeddedBoard) {
            Ok(p) => p,
            Err(t) => return t,
        };
        match p.kind {
            Kind::Arduino => {
                if !find("arduino-cli") {
                    return self.embedded_need(Need::Tool { tool: "arduino-cli".into(), hint: embedded::tool_hint("arduino-cli") });
                }
                std::thread::spawn(|| bus::send(Message::Embedded(arduino_boards())));
                self.toast(tr("Looking for boards…"))
            }
            _ if !p.boards.is_empty() => {
                let items = p
                    .boards
                    .iter()
                    .map(|b| PItem {
                        label: format!("{}{b}", if p.board.as_deref() == Some(b.as_str()) { "✓ " } else { "" }),
                        detail: String::new(),
                        hint: String::new(),
                        positions: Vec::new(),
                        action: PAction::Board(b.clone()),
                    })
                    .collect();
                self.open_palette_with(PaletteMode::Locations, items, Vec::new())
            }
            _ => {
                let t = self.toast(tr("Say which board or chip in .tinta/embedded.json"));
                Task::batch([t, self.embedded_configure()])
            }
        }
    }

    pub fn set_board(&mut self, board: String) -> Task {
        let t = self.save_prefs(|p| p.board = Some(board.clone()));
        Task::batch([t, self.toast(format!("{}: {board}", tr("Board")))])
    }

    pub fn on_embedded(&mut self, m: EmbeddedMsg) -> Task {
        match m {
            EmbeddedMsg::Boards { connected, all, error } => {
                if let Some(e) = error.filter(|_| all.is_empty()) {
                    self.output.push(format!("arduino-cli: {e}"));
                    return self.toast(format!("arduino-cli: {e}"));
                }
                let plugged = tr("plugged in");
                let mut items: Vec<PItem> = connected
                    .iter()
                    .map(|(fqbn, name)| PItem { label: format!("● {name}"), detail: format!("{fqbn} · {plugged}"), hint: String::new(), positions: Vec::new(), action: PAction::Board(fqbn.clone()) })
                    .collect();
                items.extend(all.into_iter().filter(|(f, _)| !connected.iter().any(|(c, _)| c == f)).map(|(fqbn, name)| PItem {
                    label: name,
                    detail: fqbn.clone(),
                    hint: String::new(),
                    positions: Vec::new(),
                    action: PAction::Board(fqbn),
                }));
                if items.is_empty() {
                    let hint = if self.pt() { "Nenhuma placa instalada: arduino-cli core install arduino:avr" } else { "No boards installed: arduino-cli core install arduino:avr" };
                    return self.toast(hint);
                }
                self.open_palette_with(PaletteMode::Locations, items, Vec::new())
            }
            EmbeddedMsg::CreateIn { template, name, parent } => self.create_embedded_project(template, &name, &parent),
        }
    }

    // Flashing -----------------------------------------------------------------------------------

    /// The project's task that ends with `verb` (": build", ": upload"), or why there's none.
    fn embedded_task(&mut self, verb: &str, then: Command) -> Result<tinta_core::tasks::Task, Task> {
        let p = self.firmware_project(then)?;
        if p.kind == Kind::Arduino && p.board.is_none() {
            return Err(self.embedded_pick_board());
        }
        match p.tasks(&find).into_iter().find(|t| t.name.ends_with(verb)) {
            Some(t) => Ok(t),
            None => {
                let pt = self.pt();
                Err(match p.build(&find, pt).and_then(|_| p.upload(&find, pt)) {
                    Err(n) => self.embedded_need(n),
                    Ok(_) => self.toast(tr("Nothing to flash with")),
                })
            }
        }
    }

    /// Verify: builds the firmware, errors in the code, nothing written to the board.
    pub fn embedded_verify(&mut self) -> Task {
        match self.embedded_task(": build", Command::EmbeddedVerify) {
            Ok(t) => self.run_task(t),
            Err(t) => t,
        }
    }

    /// Builds, then puts the firmware on the board, in the task panel: errors go to the code. The
    /// serial monitor lets go of the port meanwhile, and comes back after; `then_watch` opens it
    /// even if it wasn't.
    pub fn embedded_upload(&mut self, then_watch: bool) -> Task {
        match self.embedded_task(": upload", Command::EmbeddedFlash) {
            Ok(t) => {
                let started = self.run_task(t);
                if then_watch && let Some((g, _)) = self.serial.after_upload {
                    self.serial.after_upload = Some((g, true));
                }
                started
            }
            Err(t) => t,
        }
    }

    pub fn embedded_configure(&mut self) -> Task {
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        let path = root.join(embedded::CONFIG_PATH);
        if !path.exists() {
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, embedded::CONFIG_TEMPLATE));
            if let Err(e) = written {
                return self.toast(e.to_string());
            }
            // Written here, by the user: theirs to trust.
            let _ = trust::trust(&root);
            self.runner.untrusted.remove(&root);
            self.reload_explorer();
        }
        self.open_path(&path)
    }

    // New projects -------------------------------------------------------------------------------

    pub fn embedded_new_project(&mut self) -> Task {
        let pt = self.pt();
        let items = templates::all()
            .iter()
            .map(|t| PItem { label: t.name.to_string(), detail: t.detail(pt).to_string(), hint: String::new(), positions: Vec::new(), action: PAction::EmbeddedTemplate(t.id) })
            .collect();
        self.open_palette_with(PaletteMode::Locations, items, Vec::new())
    }

    pub fn embedded_template_chosen(&mut self, id: &'static str) -> Task {
        self.dialog_input = if self.pt() { "pisca".into() } else { "blink".into() };
        self.dialog = Some(DialogState::NewEmbeddedProject(id));
        self.focus = Focus::Input;
        Task::batch([fx::focus(super::DIALOG_INPUT), fx::select_all(super::DIALOG_INPUT)])
    }

    /// The name given: next, where the project's folder goes.
    pub fn embedded_name_given(&mut self, template: &'static str, name: String) -> Task {
        let name = name.trim().to_string();
        if name.is_empty() || name.contains('/') {
            return self.toast(tr("Invalid name"));
        }
        let t = self.toast(tr("Choose the folder to create the project in"));
        Task::batch([t, Task::effect(fx::Effect::OpenFolder(Box::new(move |parent| Message::Embedded(EmbeddedMsg::CreateIn { template, name, parent }))))])
    }

    fn create_embedded_project(&mut self, id: &'static str, name: &str, parent: &Path) -> Task {
        let Some(t) = templates::by_id(id) else { return Task::none() };
        let dir = parent.join(templates::folder_name(t, name));
        let open = match templates::create(t, &dir, name) {
            Ok(f) => f,
            Err(e) => return self.toast(e),
        };
        // Made here, by the user: its .tinta files are theirs.
        let _ = trust::trust(&dir);
        let folder = self.open_folder(dir.clone());
        let file = self.open_path(&open);
        self.refresh_embedded();
        let text = if self.pt() { format!("Projeto criado em {}: Ctrl+F5 compila e grava na placa", dir.display()) } else { format!("Project created in {}: Ctrl+F5 builds and flashes the board", dir.display()) };
        Task::batch([folder, file, self.toast(text)])
    }
}
