//! What the application asks of the toolkit after a message: a list of effects, run by the GTK
//! side once the update is done. The app's logic never touches a widget.

use crate::app::Message;
use std::path::PathBuf;

pub type Then<T> = Box<dyn FnOnce(T) -> Message>;

pub enum Effect {
    /// Another message, handled right after this one.
    Message(Message),
    /// Keyboard focus to a field, by the name the app gives it.
    Focus(&'static str),
    SelectAll(&'static str),
    /// Back to the text.
    FocusEditor,
    ClipboardWrite(String),
    ClipboardRead(Then<Option<String>>),
    OpenFiles(Then<Vec<PathBuf>>),
    OpenFolder(Then<PathBuf>),
    SaveAs { name: String, folder: Option<PathBuf>, then: Then<PathBuf> },
    Fullscreen(bool),
    Quit,
    /// A picture of the window, for the test scripts.
    Screenshot(PathBuf),
    /// Terminals live on the GTK side; these reach them by id.
    TermSpawn { id: u64, cwd: PathBuf },
    /// A terminal over a pseudo-terminal opened elsewhere, the debugged program's.
    TermAttach { id: u64, master: std::os::fd::OwnedFd },
    /// A terminal running `argv` instead of the shell: a program run from the editor, a REPL.
    TermRun { id: u64, cwd: PathBuf, argv: Vec<String> },
    /// Pasted, as the clipboard would be.
    TermInput { id: u64, text: String },
    /// Typed as it is: Enter, Ctrl+C.
    TermFeed { id: u64, text: String },
    TermCopy(u64),
    TermPaste(u64),
    TermKill(u64),
    TermFocus(u64),
}

/// Effects to run, in order. Empty most of the time.
#[must_use]
#[derive(Default)]
pub struct Task(pub Vec<Effect>);

impl Task {
    pub fn none() -> Self {
        Task(Vec::new())
    }

    pub fn effect(e: Effect) -> Self {
        Task(vec![e])
    }

    pub fn message(m: Message) -> Self {
        Task(vec![Effect::Message(m)])
    }

    pub fn batch(tasks: impl IntoIterator<Item = Task>) -> Self {
        Task(tasks.into_iter().flat_map(|t| t.0).collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for Task {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Task({} effects)", self.0.len())
    }
}

pub fn focus(id: &'static str) -> Task {
    Task::effect(Effect::Focus(id))
}

pub fn select_all(id: &'static str) -> Task {
    Task::effect(Effect::SelectAll(id))
}

pub fn focus_editor() -> Task {
    Task::effect(Effect::FocusEditor)
}

pub fn clipboard_write(text: String) -> Task {
    Task::effect(Effect::ClipboardWrite(text))
}

pub fn clipboard_read(then: impl FnOnce(Option<String>) -> Message + 'static) -> Task {
    Task::effect(Effect::ClipboardRead(Box::new(then)))
}
