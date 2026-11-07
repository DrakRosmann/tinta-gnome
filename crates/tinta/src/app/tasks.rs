//! Running build tasks: output, errors in the file and on the Problems panel.

use super::{App, Message, PAction, PItem, PaletteMode, PanelView};
use crate::bus;
use crate::i18n::tr;
use crate::state::Diag;
use crate::fx::Task as AppTask;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;
use tinta_core::lsp::Severity;
use tinta_core::tasks::{self, Problem, ProblemParser, Task};

/// A line of output, with any `file:line` in it already found.
#[derive(Debug, Clone)]
pub struct OutputLine {
    pub text: String,
    pub location: Option<tasks::Location>,
    pub severity: Option<Severity>,
}

#[derive(Default)]
pub struct TaskPanel {
    pub last: Option<Task>,
    pub running: Option<u32>,
    pub generation: u64,
    pub lines: Vec<OutputLine>,
    pub problems: Vec<(PathBuf, Problem)>,
    pub current: Option<usize>,
    pub exit: Option<Result<(), Option<i32>>>,
    pub parser: ProblemParser,
    pub root: PathBuf,
}

#[derive(Debug, Clone)]
pub enum TaskMsg {
    Lines(u64, Vec<String>),
    Finished(u64, Option<i32>),
    Jump(usize),
}

/// Keeps a noisy build from flooding the app: output goes out in batches, a few times a second.
fn pump(generation: u64, rx: Receiver<String>) {
    std::thread::spawn(move || {
        let mut batch = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_millis(80)) {
                Ok(line) => {
                    batch.push(line);
                    if batch.len() < 2000 {
                        continue;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if !batch.is_empty() {
                        bus::send(Message::Task(TaskMsg::Lines(generation, batch)));
                    }
                    return;
                }
            }
            if !batch.is_empty() {
                bus::send(Message::Task(TaskMsg::Lines(generation, std::mem::take(&mut batch))));
            }
        }
    });
}

fn read_lines(from: impl Read + Send + 'static, tx: std::sync::mpsc::Sender<String>) {
    std::thread::spawn(move || {
        for line in BufReader::new(from).split(b'\n').map_while(Result::ok) {
            let text = String::from_utf8_lossy(&line).into_owned();
            if tx.send(text).is_err() {
                break;
            }
        }
    });
}

impl App {
    pub fn task_root(&self) -> Option<PathBuf> {
        self.folder.clone().or_else(|| self.active().and_then(|(_, e)| e.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf)))
    }

    pub fn run_task_picker(&mut self) -> AppTask {
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        let (list, problem) = tasks::detect(&root);
        if let Some(p) = problem {
            self.output.push(p);
        }
        if list.is_empty() {
            return self.toast(tr("No tasks found in this folder"));
        }
        let items = list
            .into_iter()
            .map(|t| PItem {
                label: t.name.clone(),
                detail: t.command.clone(),
                hint: String::new(),
                positions: Vec::new(),
                action: PAction::Task(Box::new(t)),
            })
            .collect();
        self.open_palette_with(PaletteMode::Locations, items, Vec::new())
    }

    pub fn run_default_build(&mut self) -> AppTask {
        let Some(root) = self.task_root() else { return self.toast(tr("No folder open")) };
        let (list, _) = tasks::detect(&root);
        match tasks::default_build(&list).cloned() {
            Some(t) => self.run_task(t),
            None => self.toast(tr("No tasks found in this folder")),
        }
    }

    pub fn rerun_task(&mut self) -> AppTask {
        match self.tasks.last.clone() {
            Some(t) => self.run_task(t),
            None => self.run_default_build(),
        }
    }

    pub fn stop_task(&mut self) {
        if let Some(pid) = self.tasks.running.take() {
            // The whole group: `cargo run` has children of its own.
            let _ = Command::new("kill").arg("--").arg(format!("-{pid}")).stderr(Stdio::null()).status();
        }
    }

    pub fn run_task(&mut self, task: Task) -> AppTask {
        self.stop_task();
        let Some(root) = self.task_root() else { return AppTask::none() };
        let save = self.save_all_dirty_named();
        self.clear_task_problems();
        let cwd = task.cwd.as_ref().map(|c| root.join(c)).unwrap_or_else(|| root.clone());
        self.tasks.generation += 1;
        let generation = self.tasks.generation;
        self.tasks.lines.clear();
        self.tasks.exit = None;
        self.tasks.current = None;
        self.tasks.parser = ProblemParser::default();
        self.tasks.root = cwd.clone();
        self.tasks.last = Some(task.clone());
        self.panel = Some(PanelView::Tasks);

        let child = tinta_core::host::command("sh", ["-c", task.command.as_str()], Some(&cwd), &[("TERM", "dumb")])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return AppTask::batch([save, self.toast(format!("{}: {e}", task.name))]),
        };
        self.tasks.running = Some(child.id());
        let (tx, rx) = channel();
        if let Some(out) = child.stdout.take() {
            read_lines(out, tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            read_lines(err, tx.clone());
        }
        drop(tx);
        pump(generation, rx);
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|s| s.code());
            bus::send(Message::Task(TaskMsg::Finished(generation, code)));
        });
        save
    }

    fn clear_task_problems(&mut self) {
        self.tasks.problems.clear();
        for e in self.docs.values_mut() {
            e.diags.retain(|d| d.source.as_deref() != Some(TASK_SOURCE));
        }
    }

    /// Finds the file a compiler named, relative to where it ran.
    fn resolve_output_path(&self, path: &str) -> Option<PathBuf> {
        let p = Path::new(path);
        let candidates = [Some(p.to_path_buf()), Some(self.tasks.root.join(p)), self.folder.as_ref().map(|f| f.join(p))];
        candidates.into_iter().flatten().find(|c| c.is_file()).map(|c| std::fs::canonicalize(&c).unwrap_or(c))
    }

    /// A path from a terminal: its working directory isn't known, so also try the open files'
    /// folders and, failing that, any file in the project ending the same way.
    pub fn resolve_any_path(&self, path: &str) -> Option<PathBuf> {
        if let Some(p) = self.resolve_output_path(path) {
            return Some(p);
        }
        if let Some(dir) = self.active().and_then(|(_, e)| e.doc.path.as_deref().and_then(Path::parent).map(Path::to_path_buf))
            && dir.join(path).is_file()
        {
            return Some(dir.join(path));
        }
        let wanted = path.trim_start_matches("./");
        let found = self.file_index.iter().find(|f| f.as_str() == wanted || f.ends_with(&format!("/{wanted}")))?;
        self.folder.as_ref().map(|r| r.join(found))
    }

    fn attach_problem(&mut self, path: &Path, p: &Problem) {
        let Some(id) = self.doc_by_path(path) else { return };
        let Some(e) = self.docs.get_mut(&id) else { return };
        let start = e.doc.pos(p.location.line, p.location.col);
        let (a, b) = tinta_core::commands::word_at(&e.doc, start);
        let end = if a == b { (start + 1).min(e.doc.line_end(p.location.line)) } else { b };
        e.diags.push(Diag {
            start: if a == b { start } else { a },
            end,
            severity: p.severity,
            message: p.message.clone(),
            source: Some(TASK_SOURCE.to_string()),
            raw: serde_json::Value::Null,
        });
        e.diags.sort_by_key(|d| d.start);
    }

    /// Files opened after the build still get its marks.
    pub fn attach_task_problems_to(&mut self, doc_path: &Path) {
        let list: Vec<(PathBuf, Problem)> = self.tasks.problems.iter().filter(|(p, _)| p == doc_path).cloned().collect();
        for (p, problem) in list {
            self.attach_problem(&p, &problem);
        }
    }

    pub fn on_task(&mut self, m: TaskMsg) -> AppTask {
        match m {
            TaskMsg::Lines(g, lines) => {
                if g != self.tasks.generation {
                    return AppTask::none();
                }
                for raw in lines {
                    let text = tasks::clean_line(&raw);
                    let problem = self.tasks.parser.feed(&text);
                    let mut severity = None;
                    if let Some(p) = problem {
                        severity = Some(p.severity);
                        if let Some(path) = self.resolve_output_path(&p.location.path) {
                            self.attach_problem(&path, &p);
                            self.tasks.problems.push((path, p));
                        }
                    }
                    let location = tasks::find_locations(&text).into_iter().find(|l| self.resolve_output_path(&l.path).is_some());
                    self.tasks.lines.push(OutputLine { text, location, severity });
                }
                let n = self.tasks.lines.len();
                if n > 20_000 {
                    self.tasks.lines.drain(..n - 20_000);
                }
                AppTask::none()
            }
            TaskMsg::Finished(g, code) => {
                if g != self.tasks.generation {
                    return AppTask::none();
                }
                self.tasks.running = None;
                self.tasks.exit = Some(if code == Some(0) { Ok(()) } else { Err(code) });
                self.debug_build_finished(g, code == Some(0));
                let name = self.tasks.last.as_ref().map(|t| t.name.clone()).unwrap_or_default();
                let errors = self.tasks.problems.iter().filter(|(_, p)| p.severity == Severity::Error).count();
                let warnings = self.tasks.problems.len() - errors;
                let text = if code == Some(0) && errors == 0 {
                    format!("{name}: {} ({warnings} {})", tr("finished"), tr("warnings"))
                } else {
                    format!("{name}: {errors} {}, {warnings} {}", tr("errors"), tr("warnings"))
                };
                self.toast(text)
            }
            TaskMsg::Jump(i) => {
                let Some(loc) = self.tasks.lines.get(i).and_then(|l| l.location.clone()) else { return AppTask::none() };
                let Some(path) = self.resolve_output_path(&loc.path) else { return AppTask::none() };
                self.open_at(&path, loc.line, loc.col)
            }
        }
    }

    pub fn open_at(&mut self, path: &Path, line: usize, col: usize) -> AppTask {
        self.push_nav();
        let t = self.open_path(path);
        self.jump_to(line, col);
        self.focus = super::Focus::Editor;
        t
    }

    pub fn task_problem_step(&mut self, forward: bool) -> AppTask {
        let n = self.tasks.problems.len();
        if n == 0 {
            return AppTask::none();
        }
        let i = match self.tasks.current {
            Some(c) if forward => (c + 1) % n,
            Some(c) => (c + n - 1) % n,
            None if forward => 0,
            None => n - 1,
        };
        self.tasks.current = Some(i);
        let (path, p) = self.tasks.problems[i].clone();
        let t = self.open_at(&path, p.location.line, p.location.col);
        self.hover = self.active().map(|(tab, _)| super::HoverPopup { text: p.message.clone(), pos: tab.cursors.head() });
        t
    }

    /// Ctrl+click in the terminal.
    pub fn on_term_link(&mut self, text: String, col: usize) -> AppTask {
        if let Some(url) = tasks::url_at(&text, col) {
            let _ = open::that_detached(url);
            return AppTask::none();
        }
        if let Some(loc) = tasks::location_at(&text, col)
            && let Some(path) = self.resolve_any_path(&loc.path)
        {
            return self.open_at(&path, loc.line, loc.col);
        }
        // A bare path with no line number is worth opening too.
        if let Some(word) = word_at_col(&text, col)
            && let Some(path) = self.resolve_any_path(&word)
        {
            return self.open_path(&path);
        }
        AppTask::none()
    }
}

pub const TASK_SOURCE: &str = "tinta";

fn word_at_col(text: &str, col: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if col >= chars.len() {
        return None;
    }
    let ok = |c: char| c.is_alphanumeric() || "._-/~+@".contains(c);
    if !ok(chars[col]) {
        return None;
    }
    let mut a = col;
    while a > 0 && ok(chars[a - 1]) {
        a -= 1;
    }
    let mut b = col;
    while b < chars.len() && ok(chars[b]) {
        b += 1;
    }
    Some(chars[a..b].iter().collect())
}

#[cfg(test)]
mod tests {
    use super::word_at_col;

    #[test]
    fn words_for_paths() {
        assert_eq!(word_at_col("ls src/a.rs agora", 5).as_deref(), Some("src/a.rs"));
        assert_eq!(word_at_col("a b", 1), None);
        assert_eq!(word_at_col("a", 9), None);
    }
}
