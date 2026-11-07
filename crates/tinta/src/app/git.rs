//! The source control panel and the gutter's base text.

use super::{App, DialogState, Message};
use crate::bus;
use crate::i18n::tr;
use crate::state::DocId;
use crate::fx::Task;
use std::path::PathBuf;
use tinta_core::git::{self as core_git, StatusEntry};

#[derive(Default)]
pub struct GitPanel {
    pub root: Option<PathBuf>,
    pub branch: Option<String>,
    pub entries: Vec<StatusEntry>,
    pub message: String,
    pub ahead_behind: Option<(usize, usize)>,
    pub busy: bool,
    pub generation: u64,
}

#[derive(Debug, Clone)]
pub enum GitMsg {
    Status { generation: u64, root: Option<PathBuf>, branch: Option<String>, entries: Vec<StatusEntry>, ignored: Vec<PathBuf>, ahead_behind: Option<(usize, usize)> },
    Base(DocId, Option<(PathBuf, String)>, Option<String>),
    MessageInput(String),
    Commit,
    Stage(String),
    Unstage(String),
    StageAll,
    Discard(StatusEntry),
    Open(String, bool),
    Finished(String, Result<String, String>),
    Blame(DocId, usize, String),
}

fn ignored_paths(root: &std::path::Path) -> Vec<PathBuf> {
    let out = tinta_core::host::command("git", ["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"], Some(root), &[("GIT_OPTIONAL_LOCKS", "0")])
        .stdin(std::process::Stdio::null())
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| root.join(s.trim_end_matches('/')))
        .collect()
}

impl App {
    /// Status runs on a thread: on a big repository it can take a moment.
    pub fn refresh_git_panel(&mut self) {
        let Some(folder) = self.folder.clone().or_else(|| self.active().and_then(|(_, e)| e.doc.path.clone())) else { return };
        self.git.generation += 1;
        let generation = self.git.generation;
        std::thread::spawn(move || {
            let root = core_git::repo_root(&folder);
            let (branch, entries, ignored, ahead_behind) = match &root {
                Some(r) => (
                    core_git::branch(r),
                    core_git::status(r).unwrap_or_default(),
                    ignored_paths(r),
                    core_git::ahead_behind(r),
                ),
                None => (None, Vec::new(), Vec::new(), None),
            };
            bus::send(Message::Git(GitMsg::Status { generation, root, branch, entries, ignored, ahead_behind }));
        });
    }

    pub fn refresh_git_for_active(&mut self) {
        if let Some((t, _)) = self.active() {
            let d = t.doc;
            self.refresh_git_for(d);
        }
    }

    /// Fetches what HEAD has for this file, for the gutter marks.
    pub fn refresh_git_for(&mut self, doc: DocId) {
        if !self.settings.git_gutter {
            return;
        }
        let Some(e) = self.docs.get(&doc) else { return };
        let Some(path) = e.doc.path.clone() else { return };
        std::thread::spawn(move || {
            let root = core_git::repo_root(&path);
            let (rel, base) = match root {
                Some(r) => {
                    let canon_root = std::fs::canonicalize(&r).unwrap_or(r);
                    match path.strip_prefix(&canon_root).map(|p| p.to_string_lossy().into_owned()) {
                        Ok(rel) => {
                            let base = core_git::file_at_head(&canon_root, &rel);
                            (Some((canon_root, rel)), base)
                        }
                        Err(_) => (None, None),
                    }
                }
                None => (None, None),
            };
            bus::send(Message::Git(GitMsg::Base(doc, rel, base)));
        });
    }

    pub fn update_blame(&mut self) -> Task {
        if !self.settings.inline_blame {
            return Task::none();
        }
        let Some((t, e)) = self.active() else { return Task::none() };
        let doc_id = t.doc;
        let line = e.doc.char_to_line(t.cursors.head());
        if e.blame.as_ref().is_some_and(|(l, _)| *l == line) || e.doc.is_dirty() {
            return Task::none();
        }
        let Some((root, rel)) = e.git_rel.clone() else { return Task::none() };
        std::thread::spawn(move || {
            let text = match core_git::blame_line(&root, &rel, line) {
                Some(b) => format!("{}, {} · {}", b.author, b.date, b.summary),
                None => String::new(),
            };
            bus::send(Message::Git(GitMsg::Blame(doc_id, line, text)));
        });
        Task::none()
    }

    pub fn git_remote(&mut self, what: &'static str) -> Task {
        let Some(root) = self.git.root.clone() else { return self.toast(tr("Not a git repository")) };
        self.git.busy = true;
        std::thread::spawn(move || {
            let r = match what {
                "push" => core_git::push(&root),
                "pull" => core_git::pull(&root),
                _ => core_git::fetch(&root),
            };
            bus::send(Message::Git(GitMsg::Finished(what.to_string(), r)));
        });
        Task::none()
    }

    pub fn git_discard(&mut self, entry: StatusEntry) -> Task {
        let Some(root) = self.git.root.clone() else { return Task::none() };
        if let Err(e) = core_git::discard(&root, &entry) {
            return self.toast(e);
        }
        self.refresh_git_panel();
        self.reload_explorer();
        Task::none()
    }

    pub fn on_git(&mut self, m: GitMsg) -> Task {
        match m {
            GitMsg::Status { generation, root, branch, entries, ignored, ahead_behind } => {
                if generation != self.git.generation {
                    return Task::none();
                }
                // Colours for the tree: each file, and the folders on its way up.
                let mut status = std::collections::HashMap::new();
                if let Some(r) = &root {
                    for e in &entries {
                        let mark = if e.is_conflict() {
                            'U'
                        } else if e.index == '?' {
                            'A'
                        } else if e.worktree != ' ' {
                            e.worktree
                        } else {
                            e.index
                        };
                        let p = r.join(&e.path);
                        for a in p.ancestors().skip(1) {
                            if a == r.as_path() {
                                break;
                            }
                            status.entry(a.to_path_buf()).or_insert('M');
                        }
                        status.insert(p, mark);
                    }
                }
                self.explorer.status = status;
                self.explorer.ignored = ignored.into_iter().collect();
                self.git.root = root;
                self.git.branch = branch;
                self.git.entries = entries;
                self.git.ahead_behind = ahead_behind;
                self.reload_explorer();
            }
            GitMsg::Base(doc, rel, base) => {
                if let Some(e) = self.docs.get_mut(&doc) {
                    e.git_rel = rel;
                    e.git_diff = match &base {
                        Some(b) => core_git::line_diff(b, &e.doc.rope.to_string()),
                        None => Default::default(),
                    };
                    e.git_base = base;
                    e.git_stale = false;
                }
            }
            GitMsg::MessageInput(s) => self.git.message = s,
            GitMsg::Commit => {
                let Some(root) = self.git.root.clone() else { return Task::none() };
                let msg = self.git.message.trim().to_string();
                if msg.is_empty() {
                    return Task::none();
                }
                // Nothing staged means everything, as most editors do it.
                if !self.git.entries.iter().any(StatusEntry::is_staged) {
                    let _ = core_git::stage_all(&root);
                }
                match core_git::commit(&root, &msg) {
                    Ok(_) => {
                        self.git.message.clear();
                        self.refresh_git_panel();
                        let ids: Vec<DocId> = self.docs.keys().copied().collect();
                        for id in ids {
                            self.refresh_git_for(id);
                        }
                        return self.toast(tr("Committed"));
                    }
                    Err(e) => return self.toast(e),
                }
            }
            GitMsg::Stage(p) => {
                if let Some(r) = self.git.root.clone()
                    && let Err(e) = core_git::stage(&r, &p)
                {
                    return self.toast(e);
                }
                self.refresh_git_panel();
            }
            GitMsg::Unstage(p) => {
                if let Some(r) = self.git.root.clone()
                    && let Err(e) = core_git::unstage(&r, &p)
                {
                    return self.toast(e);
                }
                self.refresh_git_panel();
            }
            GitMsg::StageAll => {
                if let Some(r) = self.git.root.clone()
                    && let Err(e) = core_git::stage_all(&r)
                {
                    return self.toast(e);
                }
                self.refresh_git_panel();
            }
            GitMsg::Discard(entry) => self.dialog = Some(DialogState::ConfirmDiscard(entry)),
            GitMsg::Open(rel, staged) => {
                let Some(root) = self.git.root.clone() else { return Task::none() };
                return self.open_git_diff(root, rel, staged);
            }
            GitMsg::Finished(what, r) => {
                self.git.busy = false;
                self.refresh_git_panel();
                return match r {
                    Ok(_) => {
                        let label = match what.as_str() {
                            "push" => tr("Pushed"),
                            "pull" => tr("Pulled"),
                            _ => tr("Refresh"),
                        };
                        self.toast(label)
                    }
                    Err(e) => {
                        self.output.push(format!("git {what}: {e}"));
                        self.toast(e)
                    }
                };
            }
            GitMsg::Blame(doc, line, text) => {
                if let Some(e) = self.docs.get_mut(&doc) {
                    e.blame = Some((line, text));
                }
            }
        }
        Task::none()
    }
}
