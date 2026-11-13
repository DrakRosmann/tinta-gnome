//! Running files without the debugger, sending code to a REPL, and asking before a project's
//! own `.tinta/` files run commands.

use super::{App, DialogState, Focus, PanelView};
use crate::fx::{self, Task};
use crate::i18n::tr;
use crate::state::TabId;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tinta_core::keymap::Command;
use tinta_core::run::{self, Action, RunError, RunPlan, trust};
use tinta_core::tasks;

/// What "Run again" runs: a file, or an untitled tab written aside to be run.
#[derive(Debug, Clone)]
pub struct RunTarget {
    pub path: PathBuf,
    pub untitled: Option<TabId>,
}

#[derive(Default)]
pub struct RunPanel {
    pub last: Option<RunTarget>,
    /// A build under way, and what to run when it succeeds.
    pub after_build: Option<(u64, RunPlan)>,
    /// The terminal the last program ran in, kept with its output after it ends.
    pub term: Option<u64>,
    pub running: bool,
    /// A terminal for each REPL started, by its command.
    pub repls: HashMap<String, u64>,
    /// What "Run File with Arguments…" was given, by file.
    pub args: HashMap<PathBuf, String>,
    /// A language that can't run for want of a program: its name, what the banner says and the
    /// command that installs it.
    pub missing: Option<(String, String, Option<String>)>,
    pub hint_closed: HashSet<String>,
    /// Folders whose own files the user turned down, this session.
    pub untrusted: HashSet<PathBuf>,
}

/// Runs the program in the terminal, and says how it ended and how long it took.
const RUN_SCRIPT: &str = r#"trap : INT
printf '\033[2m$ %s\033[0m\n' "$1"
s=$(date +%s%N)
eval "$1"
c=$?
e=$(date +%s%N)
ms=$(( (e - s) / 1000000 ))
printf '\n\033[2m%s %d · %d%s%03d s\033[0m\n' "$2" "$c" $((ms / 1000)) "$3" $((ms % 1000))
exit $c"#;

/// Where programs built to run, and untitled text being run, go. Gone with the editor.
pub fn out_dir() -> PathBuf {
    tinta_core::host::temp_dir().join(format!("tinta-run-{}", std::process::id()))
}

fn find(tool: &str) -> bool {
    tinta_core::host::find(tool)
}

/// The exit code in a wait status, as the terminal reports it.
fn exit_code(status: i32) -> Option<i32> {
    (status & 0x7f == 0).then_some((status >> 8) & 0xff)
}

impl App {
    /// Ctrl+F5: the file in front.
    pub fn run_file(&mut self) -> Task {
        match self.run_target() {
            Ok(t) => self.run_target_now(t, Command::RunFile),
            Err(t) => t,
        }
    }

    pub fn run_rerun(&mut self) -> Task {
        match self.runner.last.clone() {
            Some(t) => self.run_target_now(t, Command::RunRerun),
            None => self.run_file(),
        }
    }

    /// The file in front, or where its untitled text will be written to run.
    fn run_target(&mut self) -> Result<RunTarget, Task> {
        let Some(tab) = self.active_tab_id() else { return Err(self.toast(tr("Nothing to run"))) };
        let Some((_, e)) = self.active() else { return Err(self.toast(tr("Nothing to run"))) };
        if let Some(p) = &e.doc.path {
            return Ok(RunTarget { path: p.clone(), untitled: None });
        }
        let language = e.doc.language.name.clone();
        let Some(ext) = run::extension_for(&language) else { return Err(self.run_unknown(&language)) };
        let path = out_dir().join(format!("tab-{tab}")).join(format!("untitled.{ext}"));
        Ok(RunTarget { path, untitled: Some(tab) })
    }

    fn run_unknown(&mut self, language: &str) -> Task {
        let text = if self.pt() {
            format!("Não sei como executar arquivos {language}: diga como em \"runCommands\" nas configurações ou no {}", run::RUN_PATH)
        } else {
            format!("Don't know how to run {language} files: tell me in \"runCommands\" in the settings, or in {}", run::RUN_PATH)
        };
        self.output.push(text.clone());
        self.toast(text)
    }

    /// The banner about a program missing to run `language`, named `label` in what it says.
    pub(super) fn run_missing(&mut self, language: &str, label: &str, tool: &str, command: Option<String>) -> Task {
        let mut text = if self.pt() { format!("Para executar arquivos {label}, instale o {tool}") } else { format!("To run {label} files, install {tool}") };
        if let Some(c) = &command {
            text.push_str(&format!(": {c}"));
        }
        self.runner.hint_closed.remove(language);
        self.runner.missing = Some((language.to_string(), text.clone(), command));
        self.output.push(text.clone());
        self.toast(text)
    }

    fn run_target_now(&mut self, target: RunTarget, then: Command) -> Task {
        // The text to run: the file's, or the untitled tab's, written where it can be run from.
        let (language, first_line) = match target.untitled {
            Some(tab) => {
                let Some(e) = self.tabs.get(&tab).and_then(|t| self.docs.get(&t.doc)) else { return self.toast(tr("Nothing to run")) };
                let text = e.doc.rope.to_string();
                let written = target.path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&target.path, &text));
                if let Err(err) = written {
                    return self.toast(format!("{}: {err}", target.path.display()));
                }
                (e.doc.language.name.clone(), text.lines().next().unwrap_or("").to_string())
            }
            None => match self.doc_by_path(&target.path).and_then(|id| self.docs.get(&id)) {
                Some(e) => (e.doc.language.name.clone(), e.doc.line_text(0).trim_end().to_string()),
                None => {
                    let first = std::fs::read_to_string(&target.path).ok().and_then(|s| s.lines().next().map(str::to_string)).unwrap_or_default();
                    (tinta_core::language::detect(&target.path, &first).name.clone(), first)
                }
            },
        };
        let root = match target.untitled {
            Some(_) => self.folder.clone(),
            None => self.folder.clone().filter(|f| target.path.starts_with(f)),
        }
        .unwrap_or_else(|| target.path.parent().map(Path::to_path_buf).unwrap_or_default());
        let use_project = match self.project_files_ok(&root, run::RUN_PATH, then) {
            Ok(b) => b,
            Err(t) => return t,
        };
        let project = if use_project {
            match run::load_project(&root) {
                Ok(p) => p,
                Err(e) => {
                    self.output.push(e.clone());
                    return self.toast(e);
                }
            }
        } else {
            None
        };
        // A firmware project: running means flashing the board.
        let embedded = match target.untitled {
            Some(_) => None,
            None => match self.embedded_project(&root, then) {
                Ok(p) => p,
                Err(t) => return t,
            },
        };
        let args = self.runner.args.get(&target.path).cloned().unwrap_or_default();
        let out = out_dir();
        let user = self.settings.run_commands.clone();
        let req = run::Request {
            file: &target.path,
            language: &language,
            root: &root,
            user: &user,
            project: project.as_ref(),
            args: &args,
            out_dir: &out,
            first_line: &first_line,
            embedded: embedded.as_ref(),
            portuguese: self.pt(),
        };
        let plan = match run::plan(&req, &find) {
            Ok(p) => p,
            Err(RunError::Unknown(l)) => return self.run_unknown(&l),
            Err(RunError::Missing { language, tool, hint }) => {
                // A file with no grammar is named by its extension.
                let label = match target.path.extension() {
                    Some(ext) if language == tinta_core::language::plain().name => format!(".{}", ext.to_string_lossy()),
                    _ => language.clone(),
                };
                return self.run_missing(&language, &label, &tool, hint.command);
            }
            // An Arduino sketch with no board yet: which one is it?
            Err(RunError::Config(_)) if embedded.as_ref().is_some_and(|p| p.kind == tinta_core::embedded::Kind::Arduino && p.board.is_none()) => {
                return self.embedded_pick_board();
            }
            Err(RunError::Config(e)) => {
                self.output.push(e.clone());
                return self.toast(e);
            }
        };
        // A board: built and uploaded in the task panel, then watched in the serial monitor.
        if embedded.as_ref().is_some_and(|p| plan.source == run::Source::Environment && p.uses_serial_monitor()) {
            self.runner.last = Some(target);
            return self.embedded_upload(true);
        }
        // The project's own tool watches the board (espflash, idf.py monitor): it needs the port.
        if embedded.is_some() && plan.source == run::Source::Environment {
            self.serial_disconnect();
        }
        if self.runner.missing.as_ref().is_some_and(|(l, _, _)| *l == language) {
            self.runner.missing = None;
        }
        self.runner.last = Some(target);
        let save = self.save_all_dirty_named();
        let started = self.run_plan(plan, &out);
        Task::batch([save, started])
    }

    fn run_plan(&mut self, plan: RunPlan, out: &Path) -> Task {
        match plan.action.clone() {
            Action::Open(p) => match open::that_detached(&p) {
                Ok(()) => Task::none(),
                Err(e) => self.toast(format!("{}: {e}", p.display())),
            },
            Action::Preview => self.run(Command::MarkdownPreview),
            Action::Shell { build, .. } => {
                if let Err(e) = std::fs::create_dir_all(out) {
                    return self.toast(format!("{}: {e}", out.display()));
                }
                match build {
                    // Through the task panel: a failed build shows its errors in the code.
                    Some(command) => {
                        let task = tasks::Task { name: format!("{}: {}", tr("Build"), plan.title), command, cwd: Some(plan.cwd.to_string_lossy().into_owned()), group: String::new() };
                        let t = self.run_task(task);
                        self.runner.after_build = Some((self.tasks.generation, plan));
                        t
                    }
                    None => self.run_in_terminal(plan),
                }
            }
        }
    }

    /// A build finished. Called with its task's generation, so old builds don't start anything.
    pub fn run_build_finished(&mut self, generation: u64, success: bool) -> Task {
        let Some((g, plan)) = self.runner.after_build.take() else { return Task::none() };
        if g != generation {
            self.runner.after_build = Some((g, plan));
            return Task::none();
        }
        if !success {
            return Task::none();
        }
        match run::finish(plan) {
            Ok(p) => self.run_in_terminal(p),
            Err(e) => {
                self.output.push(e.clone());
                self.toast(e)
            }
        }
    }

    /// The program in its terminal, in place of the last run's.
    fn run_in_terminal(&mut self, plan: RunPlan) -> Task {
        let Action::Shell { run: command, .. } = &plan.action else { return Task::none() };
        let mut tasks = Vec::new();
        if let Some(old) = self.runner.term.take() {
            if let Some(i) = self.terms.iter().position(|t| t.id == old) {
                self.terms.remove(i);
            }
            tasks.push(Task::effect(fx::Effect::TermKill(old)));
        }
        let id = self.new_id();
        self.terms.push(super::TermInfo { id, title: format!("▶ {}", plan.title) });
        self.active_term = self.terms.len() - 1;
        self.panel = Some(PanelView::Terminal);
        self.runner.term = Some(id);
        self.runner.running = true;
        let (ended, sep) = if self.pt() { ("Terminou com o código", ",") } else { ("Exited with code", ".") };
        let argv = ["sh", "-c", RUN_SCRIPT, "tinta-run", command, ended, sep].map(str::to_string).to_vec();
        tasks.push(Task::effect(fx::Effect::TermRun { id, cwd: plan.cwd.clone(), argv }));
        Task::batch(tasks)
    }

    /// The terminal's program ended. True if it was the run's, which stays with its output.
    pub fn run_term_exited(&mut self, id: u64, status: i32) -> bool {
        self.runner.repls.retain(|_, t| *t != id);
        if self.runner.term != Some(id) {
            return false;
        }
        self.runner.running = false;
        if let Some(t) = self.terms.iter_mut().find(|t| t.id == id) {
            let name = t.title.trim_start_matches("▶ ").to_string();
            t.title = match exit_code(status) {
                Some(0) => format!("✓ {name}"),
                Some(c) => format!("✗ {name} ({c})"),
                None => format!("✗ {name}"),
            };
        }
        true
    }

    /// A terminal closed by the user: forget it if it was the run's or a REPL's.
    pub fn run_term_closed(&mut self, id: u64) {
        if self.runner.term == Some(id) {
            self.runner.term = None;
            self.runner.running = false;
        }
        self.runner.repls.retain(|_, t| *t != id);
    }

    pub fn run_stop(&mut self) -> Task {
        if self.runner.after_build.take().is_some() {
            self.stop_task();
            return self.toast(tr("Stopped"));
        }
        match self.runner.term.filter(|_| self.runner.running) {
            // As Ctrl+C in the terminal would: the program gets to clean up.
            Some(id) => Task::effect(fx::Effect::TermFeed { id, text: "\u{3}".into() }),
            None => self.toast(tr("Nothing is running")),
        }
    }

    pub fn run_with_args(&mut self) -> Task {
        let target = match self.run_target() {
            Ok(t) => t,
            Err(t) => return t,
        };
        self.dialog_input = self.runner.args.get(&target.path).cloned().unwrap_or_default();
        self.dialog = Some(DialogState::RunArgs(target.path));
        self.focus = Focus::Input;
        fx::select_all(super::DIALOG_INPUT)
    }

    pub fn set_run_args(&mut self, path: PathBuf, args: String) -> Task {
        let args = args.trim().to_string();
        if args.is_empty() {
            self.runner.args.remove(&path);
        } else {
            self.runner.args.insert(path, args);
        }
        self.mark_session_dirty();
        self.run_file()
    }

    pub fn run_configure(&mut self) -> Task {
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        let path = root.join(run::RUN_PATH);
        if !path.exists() {
            let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, run::RUN_TEMPLATE));
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

    // REPLs ------------------------------------------------------------------------------

    /// Whether the file in front has a REPL to send code to, installed or not. Without one,
    /// Shift+Enter is just a newline.
    pub fn has_repl(&self) -> bool {
        self.active().is_some_and(|(_, e)| run::repl_for(e.doc.path.as_deref(), &e.doc.language.name, &|_| true).is_some())
    }

    /// Shift+Enter: the selection, or the line, to the language's REPL.
    pub fn run_selection(&mut self) -> Task {
        let Some((t, e)) = self.active() else { return Task::none() };
        let language = e.doc.language.name.clone();
        let path = e.doc.path.clone();
        let sels: Vec<String> = t.cursors.iter().filter(|s| !s.is_empty()).map(|s| e.doc.slice_string(s.start(), s.end())).collect();
        let (line, _) = e.doc.line_col(t.cursors.head());
        let (line_text, lines) = (e.doc.line_text(line).into_owned(), e.doc.len_lines());
        let repl = match run::repl_for(path.as_deref(), &language, &find) {
            None => {
                let text = if self.pt() { format!("Nenhum REPL conhecido para {language}") } else { format!("No REPL known for {language}") };
                return self.toast(text);
            }
            Some(Err((tool, hint))) => return self.run_missing(&language, &language, &tool, hint.command),
            Some(Ok(r)) => r,
        };
        let text = if sels.is_empty() {
            // A line sent, the cursor goes on to the next, to send that one too.
            if line + 1 < lines
                && let Some((t, e)) = self.active_mut()
            {
                let pos = e.doc.line_start(line + 1);
                t.cursors.set_single(tinta_core::Selection::cursor(pos));
                e.doc.break_undo();
                self.after_caret_move();
            }
            line_text
        } else {
            sels.join("\n")
        };
        if text.trim().is_empty() {
            return Task::none();
        }
        let (input, enters) = run::repl_input(repl.kind, &text);
        let cwd = path.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(|| self.folder.clone()).unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        let (id, spawn) = self.ensure_repl(&repl, &language, &cwd);
        Task::batch([
            spawn,
            Task::effect(fx::Effect::TermInput { id, text: input }),
            Task::effect(fx::Effect::TermFeed { id, text: "\r".repeat(enters) }),
        ])
    }

    /// The REPL's terminal, started if it isn't running.
    fn ensure_repl(&mut self, repl: &run::Repl, language: &str, cwd: &Path) -> (u64, Task) {
        if let Some(&id) = self.runner.repls.get(&repl.command)
            && let Some(i) = self.terms.iter().position(|t| t.id == id)
        {
            self.active_term = i;
            self.panel = Some(PanelView::Terminal);
            return (id, Task::none());
        }
        let id = self.new_id();
        self.terms.push(super::TermInfo { id, title: format!("REPL: {language}") });
        self.active_term = self.terms.len() - 1;
        self.panel = Some(PanelView::Terminal);
        self.runner.repls.insert(repl.command.clone(), id);
        let argv = vec!["sh".to_string(), "-c".to_string(), format!("exec {}", repl.command)];
        (id, Task::effect(fx::Effect::TermRun { id, cwd: cwd.to_path_buf(), argv }))
    }

    pub fn open_repl(&mut self) -> Task {
        let Some((_, e)) = self.active() else { return self.toast(tr("Nothing to run")) };
        let language = e.doc.language.name.clone();
        let path = e.doc.path.clone();
        let repl = match run::repl_for(path.as_deref(), &language, &find) {
            None => {
                let text = if self.pt() { format!("Nenhum REPL conhecido para {language}") } else { format!("No REPL known for {language}") };
                return self.toast(text);
            }
            Some(Err((tool, hint))) => return self.run_missing(&language, &language, &tool, hint.command),
            Some(Ok(r)) => r,
        };
        let cwd = path.as_deref().and_then(Path::parent).map(Path::to_path_buf).or_else(|| self.folder.clone()).unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
        let (id, spawn) = self.ensure_repl(&repl, &language, &cwd);
        self.focus = Focus::Terminal;
        Task::batch([spawn, Task::effect(fx::Effect::TermFocus(id))])
    }

    // The banner -----------------------------------------------------------------------------

    /// The banner for the file in front: a program missing to run it, or a language server.
    /// Its key, what it says, and whether there's a command to copy.
    pub fn banner_hint(&self) -> Option<(String, String, bool)> {
        if let Some((lang, text, command)) = &self.runner.missing
            && !self.runner.hint_closed.contains(lang)
            && self.active().is_some_and(|(_, e)| e.doc.language.name == *lang)
        {
            return Some((format!("run:{lang}"), text.clone(), command.is_some()));
        }
        self.lsp_hint()
    }

    /// The run banner closed: its command, if it has one, to copy.
    pub fn run_hint_done(&mut self, lang: &str) -> Option<String> {
        self.runner.hint_closed.insert(lang.to_string());
        self.runner.missing.as_ref().filter(|(l, _, _)| l == lang).and_then(|(_, _, c)| c.clone())
    }

    // Trust ----------------------------------------------------------------------------------

    /// Whether the project's `file` may be used to run commands. A folder not trusted yet is
    /// asked about first, and `then` runs again once the user answers.
    pub fn project_files_ok(&mut self, root: &Path, file: &'static str, then: Command) -> Result<bool, Task> {
        if !root.join(file).exists() || trust::is_trusted(root) {
            return Ok(true);
        }
        if self.runner.untrusted.contains(root) {
            return Ok(false);
        }
        self.dialog = Some(DialogState::TrustFolder(root.to_path_buf(), file, then));
        Err(Task::none())
    }

    /// The answer to the question above.
    pub fn answer_trust(&mut self, root: PathBuf, trusted: bool, then: Command) -> Task {
        if trusted {
            if let Err(e) = trust::trust(&root) {
                return self.toast(e);
            }
            self.runner.untrusted.remove(&root);
        } else {
            self.runner.untrusted.insert(root);
        }
        self.run(then)
    }

    pub fn trust_folder(&mut self, trusted: bool) -> Task {
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        let done = if trusted { trust::trust(&root) } else { trust::untrust(&root) };
        if let Err(e) = done {
            return self.toast(e);
        }
        if trusted {
            self.runner.untrusted.remove(&root);
            self.toast(tr("Folder trusted: its .tinta files can run commands"))
        } else {
            self.runner.untrusted.insert(root);
            self.toast(tr("Folder no longer trusted: its .tinta files are ignored"))
        }
    }
}
