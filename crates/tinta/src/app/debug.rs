//! Debugging: starting sessions, breakpoints, and what the debugger's replies mean for the screen.

use super::lsp::HoverPopup;
use super::{App, DialogState, Focus, Message, PAction, PItem, PaletteMode, PanelView, SidebarView};
use crate::bus;
use crate::i18n::{tr, tr_message};
use crate::fx::{self, Task};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tinta_core::dap::{self, Breakpoints, Debugger, Incoming, Notice, Plan, VarRow};
use tinta_core::tasks;

pub const DEBUG_INPUT: &str = "debug-input";
pub const WATCH_INPUT: &str = "watch-input";
const MAX_CONSOLE: usize = 20_000;

#[derive(Debug, Clone)]
pub struct ConsoleLine {
    pub text: String,
    pub category: String,
    /// The program hasn't finished this line yet.
    pub open: bool,
}

#[derive(Default)]
pub struct DebugPanel {
    pub session: Option<Debugger>,
    pub generation: u64,
    pub breakpoints: Breakpoints,
    pub watches: Vec<String>,
    pub console: Vec<ConsoleLine>,
    pub input: String,
    pub watch_input: String,
    /// The configuration F5 runs, when the project has several.
    pub selected: Option<String>,
    /// A build under way, and what to run when it succeeds.
    pub after_build: Option<(u64, Plan)>,
    /// The terminal the program runs in, by its id in the panel.
    pub tty: Option<(u64, dap::ProgramTty)>,
    /// "Run to cursor" with nothing running: the place to go once the session first stops.
    pub run_to_after_start: Option<(PathBuf, usize)>,
}

#[derive(Debug, Clone)]
pub enum DebugMsg {
    Incoming(u64, Incoming),
    Ready(Result<Plan, String>),
    ToggleRow(VarRow),
    /// Asks for a new value for a line of the variables tree.
    EditValue(VarRow),
    SelectFrame(usize),
    SelectThread(i64),
    Input(String),
    Submit,
    WatchInput(String),
    WatchSubmit,
    WatchRemove(String),
    BreakpointRemove(PathBuf, usize),
    BreakpointEnable(PathBuf, usize),
    BreakpointJump(PathBuf, usize),
}

impl DebugPanel {
    pub fn is_live(&self) -> bool {
        self.session.as_ref().is_some_and(Debugger::is_live)
    }

    pub fn is_stopped(&self) -> bool {
        self.session.as_ref().is_some_and(Debugger::is_stopped)
    }

    /// Text from the debugger or the program. It may come in pieces, or several lines at once.
    fn print(&mut self, category: &str, text: &str) {
        for piece in text.split_inclusive('\n') {
            let (body, complete) = match piece.strip_suffix('\n') {
                Some(b) => (b.strip_suffix('\r').unwrap_or(b), true),
                None => (piece, false),
            };
            match self.console.last_mut() {
                Some(last) if last.open && last.category == category => {
                    last.text.push_str(body);
                    last.open = !complete;
                }
                _ => self.console.push(ConsoleLine { text: body.to_string(), category: category.to_string(), open: !complete }),
            }
        }
        let n = self.console.len();
        if n > MAX_CONSOLE {
            self.console.drain(..n - MAX_CONSOLE);
        }
    }
}

impl App {
    fn debug_problem(&mut self, message: String) -> Task {
        let text = tr_message(&message);
        self.debug.print("stderr", &format!("{text}\n"));
        self.panel = Some(PanelView::Debug);
        self.toast(text)
    }

    /// F5: on with what's running, or a new session.
    pub fn debug_start(&mut self, stop_on_entry: bool) -> Task {
        if let Some(s) = &mut self.debug.session
            && s.is_live()
        {
            s.continue_();
            return Task::none();
        }
        if self.debug.after_build.as_ref().is_some_and(|(g, _)| *g == self.tasks.generation) && self.tasks.running.is_some() {
            return Task::none();
        }
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        let (file, language) = match self.active() {
            Some((_, e)) => (e.doc.path.clone(), e.doc.language.name.clone()),
            None => (None, String::new()),
        };
        let then = if stop_on_entry { tinta_core::keymap::Command::RunToCursor } else { tinta_core::keymap::Command::DebugStart };
        let own = match self.project_files_ok(&root, dap::LAUNCH_PATH, then) {
            Ok(b) => b,
            Err(t) => return t,
        };
        // A firmware project is debugged on its board.
        let embedded = match self.embedded_project(&root, then) {
            Ok(p) => p,
            Err(t) => return t,
        };
        let pt = self.pt();
        let mut plan = match dap::plan(&root, file.as_deref(), &language, self.debug.selected.as_deref(), own, embedded.as_ref(), pt) {
            Ok(p) => p,
            Err(e) => return self.debug_problem(e),
        };
        plan.config.stop_on_entry |= stop_on_entry;
        self.debug.console.clear();
        self.sidebar = Some(SidebarView::Debug);
        self.last_sidebar = SidebarView::Debug;
        if let Some(command) = plan.config.build.clone() {
            // Through the task panel: a failed build shows its errors in the code like any other.
            let task = tasks::Task { name: tr("Build for debugging").to_string(), command, cwd: None, group: String::new() };
            let t = self.run_task(task);
            self.debug.after_build = Some((self.tasks.generation, plan));
            return t;
        }
        let save = self.save_all_dirty_named();
        Task::batch([save, self.debug_launch(Ok(plan))])
    }

    /// A build finished. Called with its task's generation, so old builds don't start anything.
    pub fn debug_build_finished(&mut self, generation: u64, success: bool) {
        let Some((g, plan)) = self.debug.after_build.take() else { return };
        if g != generation {
            self.debug.after_build = Some((g, plan));
            return;
        }
        if success {
            // Asking cargo where the binary went takes a moment.
            std::thread::spawn(move || {
                bus::send(Message::Debug(DebugMsg::Ready(dap::finish_plan(plan))));
            });
        }
    }

    fn debug_launch(&mut self, plan: Result<Plan, String>) -> Task {
        let plan = match plan {
            Ok(p) => p,
            Err(e) => return self.debug_problem(e),
        };
        self.debug.generation += 1;
        let generation = self.debug.generation;
        let sink: dap::Sink = Arc::new(move |m| bus::send(Message::Debug(DebugMsg::Incoming(generation, m))));
        let tty = match plan.config.wants_terminal().then(dap::open_tty) {
            Some(Ok(t)) => Some(t),
            Some(Err(e)) => {
                self.debug.print("stderr", &format!("{}: {e}\n", tr("No terminal for the program")));
                None
            }
            None => None,
        };
        match Debugger::start(&plan, &self.debug.watches, sink, tty.as_ref()) {
            Ok(s) => {
                self.debug.session = Some(s);
                match tty {
                    Some(t) => self.program_terminal(t, &plan.config.name),
                    None => {
                        self.panel = Some(PanelView::Debug);
                        Task::none()
                    }
                }
            }
            Err(e) => self.debug_problem(e),
        }
    }

    /// The program's terminal in the panel, in place of the last run's.
    fn program_terminal(&mut self, tty: dap::ProgramTty, name: &str) -> Task {
        let mut tasks = Vec::new();
        if let Some((old, _)) = self.debug.tty.take()
            && let Some(i) = self.terms.iter().position(|t| t.id == old)
        {
            self.terms.remove(i);
            tasks.push(Task::effect(fx::Effect::TermKill(old)));
        }
        let master = match tty.master.try_clone() {
            Ok(m) => m,
            Err(e) => {
                self.debug.print("stderr", &format!("{}: {e}\n", tr("No terminal for the program")));
                self.panel = Some(PanelView::Debug);
                return Task::batch(tasks);
            }
        };
        let id = self.new_id();
        self.terms.push(super::TermInfo { id, title: format!("{}: {name}", tr("Program")) });
        self.active_term = self.terms.len() - 1;
        self.panel = Some(PanelView::Terminal);
        self.debug.tty = Some((id, tty));
        tasks.push(Task::effect(fx::Effect::TermAttach { id, master }));
        Task::batch(tasks)
    }

    /// Ctrl+F10: on to the line with the cursor. With nothing running, a session starts and
    /// goes there from the start of the program.
    pub fn debug_run_to_cursor(&mut self) -> Task {
        let Some((path, line)) = self.active_path_line() else { return self.toast(tr("Save the file first")) };
        let debug = &mut self.debug;
        match &mut debug.session {
            Some(s) if s.is_stopped() => {
                let list = debug.breakpoints.get(&path).cloned().unwrap_or_default();
                s.run_to(&path, line, &list);
                Task::none()
            }
            Some(s) if s.is_live() => self.toast(tr("Pause the program first")),
            _ => {
                self.debug.run_to_after_start = Some((path, line));
                self.debug_start(true)
            }
        }
    }

    pub fn debug_stop(&mut self) {
        self.debug.after_build = None;
        if let Some(mut s) = self.debug.session.take() {
            s.stop();
            self.debug.generation += 1;
        }
    }

    pub fn debug_restart(&mut self) -> Task {
        self.debug_stop();
        self.debug_start(false)
    }

    /// Runs a step, or begins a session paused at the start if none is running.
    pub fn debug_step(&mut self, f: impl FnOnce(&mut Debugger)) -> Task {
        match &mut self.debug.session {
            Some(s) if s.is_live() => {
                f(s);
                Task::none()
            }
            _ => self.debug_start(true),
        }
    }

    /// Called now and then: some debuggers go quiet instead of saying a program couldn't start.
    pub fn debug_tick(&mut self) -> Task {
        match self.debug.session.as_mut().and_then(Debugger::check_started) {
            Some(n) => self.debug_notice(n),
            None => Task::none(),
        }
    }

    /// When everything's closing: the program mustn't outlive the editor.
    pub fn debug_shutdown(&mut self) {
        if let Some(mut s) = self.debug.session.take() {
            s.stop();
            std::thread::sleep(std::time::Duration::from_millis(120));
            s.kill();
        }
    }

    pub fn toggle_breakpoint_at(&mut self, path: &Path, line: usize) {
        let list = self.debug.breakpoints.entry(path.to_path_buf()).or_default();
        dap::toggle_breakpoint(list, line);
        self.debug_sync_breakpoints(path);
    }

    /// Tells a running debugger, and forgets files with nothing left.
    fn debug_sync_breakpoints(&mut self, path: &Path) {
        let list = self.debug.breakpoints.get(path).cloned().unwrap_or_default();
        if let Some(s) = self.debug.session.as_mut().filter(|s| s.is_live()) {
            s.send_breakpoints(path, &list);
        }
        if list.is_empty() {
            self.debug.breakpoints.remove(path);
        }
    }

    fn active_path_line(&self) -> Option<(PathBuf, usize)> {
        let (t, e) = self.active()?;
        Some((e.doc.path.clone()?, e.doc.char_to_line(t.cursors.head())))
    }

    pub fn toggle_breakpoint_here(&mut self) -> Task {
        match self.active_path_line() {
            Some((p, l)) => {
                self.toggle_breakpoint_at(&p, l);
                Task::none()
            }
            None => self.toast(tr("Save the file first")),
        }
    }

    pub fn start_breakpoint_condition(&mut self) -> Task {
        let Some((p, l)) = self.active_path_line() else { return self.toast(tr("Save the file first")) };
        self.dialog_input = self.debug.breakpoints.get(&p).and_then(|list| list.iter().find(|b| b.line == l)).and_then(|b| b.condition.clone()).unwrap_or_default();
        self.dialog = Some(DialogState::BreakpointCondition(p, l));
        self.focus = Focus::Input;
        Task::batch([fx::focus(super::DIALOG_INPUT), fx::select_all(super::DIALOG_INPUT)])
    }

    pub fn set_breakpoint_condition(&mut self, path: PathBuf, line: usize, condition: String) {
        let list = self.debug.breakpoints.entry(path.clone()).or_default();
        if !list.iter().any(|b| b.line == line) {
            dap::toggle_breakpoint(list, line);
        }
        if let Some(b) = list.iter_mut().find(|b| b.line == line) {
            let c = condition.trim();
            b.condition = (!c.is_empty()).then(|| c.to_string());
        }
        self.debug_sync_breakpoints(&path);
    }

    /// Keeps breakpoints on their code as the file changes.
    pub fn debug_follow_edit(&mut self, path: &Path, change: &tinta_core::buffer::ChangeRec) {
        if let Some(list) = self.debug.breakpoints.get_mut(path) {
            dap::shift_breakpoints(list, |l| change.map_line(l));
        }
    }

    pub fn debug_configure(&mut self) -> Task {
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        let path = root.join(dap::LAUNCH_PATH);
        if !path.exists() {
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, dap::LAUNCH_TEMPLATE));
            if let Err(e) = written {
                return self.toast(e.to_string());
            }
            self.reload_explorer();
        }
        self.open_path(&path)
    }

    pub fn debug_pick_config(&mut self) -> Task {
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        match self.project_files_ok(&root, dap::LAUNCH_PATH, tinta_core::keymap::Command::DebugSelectConfig) {
            Ok(true) => {}
            Ok(false) => return self.toast(tr("This folder isn't trusted: its launch.json is ignored")),
            Err(t) => return t,
        }
        let configs = match dap::load_configs(&root) {
            Ok(c) => c,
            Err(e) => return self.debug_problem(e),
        };
        if configs.is_empty() {
            return self.debug_configure();
        }
        let items = configs
            .into_iter()
            .map(|c| PItem { label: c.name.clone(), detail: c.program.clone(), hint: String::new(), positions: Vec::new(), action: PAction::DebugConfig(c.name) })
            .collect();
        self.open_palette_with(PaletteMode::Locations, items, Vec::new())
    }

    /// The value under the pointer, while stopped. True if the debugger was asked.
    pub fn debug_hover(&mut self, line_text: &str, col: usize, pos: usize) -> bool {
        // Right after a step the new stack may not be in yet: asked without a frame, the
        // debugger looks in the wrong place.
        let Some(s) = self.debug.session.as_mut().filter(|s| s.is_stopped() && s.current_frame_id().is_some()) else { return false };
        match dap::hover_expression(line_text, col) {
            Some(expr) => {
                s.hover(&expr, pos);
                true
            }
            None => false,
        }
    }

    pub fn on_debug(&mut self, m: DebugMsg) -> Task {
        match m {
            DebugMsg::Incoming(generation, incoming) => {
                if generation != self.debug.generation {
                    return Task::none();
                }
                let debug = &mut self.debug;
                let Some(s) = &mut debug.session else { return Task::none() };
                let notices = s.handle(incoming, &mut debug.breakpoints);
                let tasks: Vec<_> = notices.into_iter().map(|n| self.debug_notice(n)).collect();
                return Task::batch(tasks);
            }
            DebugMsg::Ready(plan) => return self.debug_launch(plan),
            DebugMsg::ToggleRow(row) => {
                if let Some(s) = &mut self.debug.session {
                    s.toggle_row(&row);
                }
            }
            DebugMsg::EditValue(row) => {
                if !self.debug.session.as_ref().is_some_and(|s| s.can_set(&row)) {
                    return self.toast(tr("This debugger can't change this value"));
                }
                self.dialog_input = row.value.clone();
                self.dialog = Some(DialogState::SetValue(row));
                self.focus = Focus::Input;
                return Task::batch([fx::focus(super::DIALOG_INPUT), fx::select_all(super::DIALOG_INPUT)]);
            }
            DebugMsg::SelectFrame(i) => {
                if let Some(s) = &mut self.debug.session {
                    let notices = s.select_frame(i);
                    let tasks: Vec<_> = notices.into_iter().map(|n| self.debug_notice(n)).collect();
                    return Task::batch(tasks);
                }
            }
            DebugMsg::SelectThread(id) => {
                if let Some(s) = &mut self.debug.session {
                    s.select_thread(id);
                }
            }
            DebugMsg::Input(text) => self.debug.input = text,
            DebugMsg::Submit => {
                let text = std::mem::take(&mut self.debug.input);
                let text = text.trim();
                if text.is_empty() {
                    return Task::none();
                }
                match self.debug.session.as_mut().filter(|s| s.is_live()) {
                    Some(s) => s.console_input(text),
                    None => self.debug.print("stderr", &format!("{}\n", tr("No debug session"))),
                }
            }
            DebugMsg::WatchInput(text) => self.debug.watch_input = text,
            DebugMsg::WatchSubmit => {
                let text = std::mem::take(&mut self.debug.watch_input).trim().to_string();
                if text.is_empty() || self.debug.watches.contains(&text) {
                    return Task::none();
                }
                if let Some(s) = self.debug.session.as_mut().filter(|s| s.is_live()) {
                    s.add_watch(&text);
                }
                self.debug.watches.push(text);
            }
            DebugMsg::WatchRemove(text) => {
                if let Some(s) = &mut self.debug.session {
                    s.remove_watch(&text);
                }
                self.debug.watches.retain(|w| *w != text);
            }
            DebugMsg::BreakpointRemove(path, line) => {
                if let Some(list) = self.debug.breakpoints.get_mut(&path) {
                    list.retain(|b| b.line != line);
                }
                self.debug_sync_breakpoints(&path);
            }
            DebugMsg::BreakpointEnable(path, line) => {
                if let Some(b) = self.debug.breakpoints.get_mut(&path).and_then(|l| l.iter_mut().find(|b| b.line == line)) {
                    b.enabled = !b.enabled;
                    b.state = dap::BpState::Unknown;
                }
                self.debug_sync_breakpoints(&path);
            }
            DebugMsg::BreakpointJump(path, line) => return self.open_at(&path, line, 0),
        }
        Task::none()
    }

    fn debug_notice(&mut self, n: Notice) -> Task {
        match n {
            Notice::Output { category, text } => {
                self.debug.print(&category, &text);
                Task::none()
            }
            Notice::Show { path, line, column } => {
                // Stepping shouldn't fill the back button's history, or take the keyboard from
                // the console when that's where the step came from.
                let t = self.open_path(&path);
                self.jump_to(line, column);
                t
            }
            Notice::Stopped => {
                self.hover = None;
                if let Some((path, line)) = self.debug.run_to_after_start.take() {
                    let debug = &mut self.debug;
                    if let Some(s) = debug.session.as_mut().filter(|s| s.is_stopped()) {
                        let list = debug.breakpoints.get(&path).cloned().unwrap_or_default();
                        s.run_to(&path, line, &list);
                    }
                }
                Task::none()
            }
            Notice::Continued => {
                self.hover = None;
                Task::none()
            }
            Notice::Ended { code } => {
                let text = match code {
                    Some(c) => format!("{} {c}", tr("Program exited with code")),
                    None => tr("Debug session ended").to_string(),
                };
                self.debug.print("console", &format!("{text}\n"));
                self.debug.session = None;
                self.debug.run_to_after_start = None;
                self.toast(text)
            }
            Notice::Failed(message) => {
                self.debug.session = None;
                self.debug.run_to_after_start = None;
                self.debug_problem(message)
            }
            Notice::Hover { expression, value, pos } => {
                self.hover = Some(HoverPopup { text: format!("{expression} = {value}"), pos });
                Task::none()
            }
        }
    }
}
