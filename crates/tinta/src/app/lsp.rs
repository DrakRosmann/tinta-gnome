//! Language servers, completion, hover and the rest of code intelligence.

use super::{App, DialogState, Focus, Message, PAction, PItem, PaletteMode};
use crate::bus;
use crate::i18n::tr;
use crate::state::{Diag, DocId, TabId};
use crate::fx::Task;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tinta_core::lsp::{self as core_lsp, Client, CompletionItem, Event, Reply, TextEdit, WorkspaceEdit};
use tinta_core::{Selection, commands, fuzzy, snippet};

pub struct LspEntry {
    pub client: Client,
    pub ready: bool,
    pub waiting_docs: Vec<DocId>,
    pub lang_id: String,
}

#[derive(Debug, Clone)]
pub enum Pending {
    Completion { tab: TabId, generation: u64 },
    Hover { pos: usize },
    Signature,
    Definition,
    References,
    Rename,
    Format { save_tab: Option<TabId> },
    Actions,
    ActionResolve,
    Symbols { doc: DocId },
}

#[derive(Debug, Clone)]
pub enum CompletionSource {
    Lsp(Box<CompletionItem>),
    Word,
    /// A keyword or built-in of the file's language: picked with Enter like the server's items.
    Language,
    Snippet(String),
}

#[derive(Debug, Clone)]
pub struct CompletionEntry {
    pub label: String,
    pub detail: String,
    pub kind: &'static str,
    pub filter: String,
    pub sort: String,
    pub source: CompletionSource,
    pub positions: Vec<u32>,
}

pub struct Completion {
    pub tab: TabId,
    /// Where the word being completed starts.
    pub anchor: usize,
    pub entries: Vec<CompletionEntry>,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub generation: u64,
    /// A language server has yet to answer, so an empty list may still fill up.
    pub waiting: bool,
    pub navigated: bool,
}

impl Completion {
    /// Suggestions from a server or a move through the list count; a guess from the file doesn't.
    pub fn enter_accepts(&self) -> bool {
        self.navigated
            || self.visible.get(self.selected).and_then(|i| self.entries.get(*i)).is_some_and(|e| !matches!(e.source, CompletionSource::Word))
    }

    pub fn visible_labels(&self) -> Vec<String> {
        self.visible.iter().map(|i| self.entries[*i].label.clone()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct HoverPopup {
    pub text: String,
    pub pos: usize,
}

#[derive(Debug, Clone)]
pub struct SignaturePopup {
    pub label: String,
    pub active: Option<(usize, usize)>,
    pub pos: usize,
}

fn sink() -> core_lsp::Sink {
    Arc::new(|e| bus::send(Message::Lsp(e)))
}

impl App {
    fn server_for(&self, doc: DocId) -> Option<&LspEntry> {
        let key = self.docs.get(&doc)?.lsp_key.as_ref()?;
        self.lsp.get(key).filter(|e| e.ready)
    }

    /// Starts the language's server if needed and tells it about the document.
    pub fn lsp_open_doc(&mut self, doc: DocId) -> Task {
        if !self.settings.lsp_enabled {
            return Task::none();
        }
        let Some(e) = self.docs.get(&doc) else { return Task::none() };
        let Some(path) = e.doc.path.clone() else { return Task::none() };
        let Some(lang) = e.doc.language.lsp_id else { return Task::none() };
        let Some((cmd, args)) = core_lsp::resolve_server(lang, &self.settings.lsp_servers) else { return Task::none() };
        let root = core_lsp::find_root(&path, self.folder.as_deref());
        let key = format!("{cmd}@{}", root.display());
        if let Some(entry) = self.lsp.get_mut(&key) {
            if let Some(e) = self.docs.get_mut(&doc) {
                e.lsp_key = Some(key.clone());
                e.doc.track_lsp = true;
                if entry.ready {
                    // The server gets the whole text now, so no earlier change matters.
                    let _ = e.doc.take_lsp_changes();
                    entry.client.did_open(&path, lang, &e.doc);
                } else {
                    entry.waiting_docs.push(doc);
                }
            }
            return Task::none();
        }
        match Client::start(&cmd, &args, &root, &key, sink()) {
            Ok(client) => {
                self.lsp.insert(key.clone(), LspEntry { client, ready: false, waiting_docs: vec![doc], lang_id: lang.to_string() });
                if let Some(e) = self.docs.get_mut(&doc) {
                    e.lsp_key = Some(key);
                    e.doc.track_lsp = true;
                }
                Task::none()
            }
            Err(err) => {
                self.output.push(format!("{cmd}: {err}"));
                Task::none()
            }
        }
    }

    pub fn lsp_close_doc(&mut self, path: &Path, key: Option<&str>) {
        if let Some(k) = key
            && let Some(entry) = self.lsp.get(k)
        {
            entry.client.did_close(path);
        }
        self.project_diags.remove(path);
    }

    pub fn lsp_did_save(&mut self, doc: DocId) {
        self.lsp_flush(doc);
        if let Some(e) = self.docs.get(&doc)
            && let (Some(p), Some(s)) = (&e.doc.path, self.server_for(doc))
        {
            s.client.did_save(p, &e.doc);
        }
    }

    /// Sends pending text to the server. Called before any request, and on a timer.
    pub fn lsp_flush(&mut self, doc: DocId) {
        let Some(e) = self.docs.get_mut(&doc) else { return };
        if !e.lsp_dirty {
            return;
        }
        e.lsp_dirty = false;
        let changes = e.doc.take_lsp_changes();
        let Some(e) = self.docs.get(&doc) else { return };
        if let (Some(p), Some(s)) = (&e.doc.path, self.server_for(doc)) {
            s.client.did_change(p, &e.doc, changes);
        }
    }

    pub fn lsp_flush_all(&mut self) {
        let ids: Vec<DocId> = self
            .docs
            .iter()
            .filter(|(_, e)| e.lsp_dirty && e.last_edit.is_none_or(|t| t.elapsed() > Duration::from_millis(250)))
            .map(|(k, _)| *k)
            .collect();
        for id in ids {
            self.lsp_flush(id);
        }
    }

    pub fn restart_lsp(&mut self) -> Task {
        let Some((_, e)) = self.active() else { return Task::none() };
        let Some(k) = e.lsp_key.clone() else { return self.toast(tr("No language server for this file")) };
        self.lsp.remove(&k);
        let docs: Vec<DocId> = self.docs.iter().filter(|(_, e)| e.lsp_key.as_ref() == Some(&k)).map(|(i, _)| *i).collect();
        let mut tasks = Vec::new();
        for d in docs {
            if let Some(e) = self.docs.get_mut(&d) {
                e.lsp_key = None;
                e.diags.clear();
            }
            tasks.push(self.lsp_open_doc(d));
        }
        Task::batch(tasks)
    }

    fn with_server(&mut self) -> Option<(DocId, PathBuf, usize, String)> {
        let (t, e) = self.active()?;
        let doc = t.doc;
        let path = e.doc.path.clone()?;
        let head = t.cursors.head();
        let key = e.lsp_key.clone()?;
        self.lsp_flush(doc);
        self.lsp.get(&key).filter(|s| s.ready)?;
        Some((doc, path, head, key))
    }

    fn register(&mut self, key: &str, id: i64, p: Pending) {
        self.lsp_pending.insert((key.to_string(), id), p);
    }

    // Requests -----------------------------------------------------------------------------

    pub fn request_hover(&mut self, tab: TabId, row: usize, vcol: usize) -> Task {
        let Some(t) = self.tabs.get(&tab) else { return Task::none() };
        let Some(e) = self.docs.get(&t.doc) else { return Task::none() };
        if self.palette.is_some() || self.completion.is_some() {
            return Task::none();
        }
        let r = t.map.row(&e.doc, row);
        let line = r.line as usize;
        // Past the end of the line there is nothing to explain.
        let col_max = e.doc.line_len(line);
        let pos = t.map.row_to_pos(&e.doc, row, vcol);
        let on_text = e.doc.line_col(pos).1 < col_max;
        // Problems under the pointer come first; they don't need a server.
        let diag_text: Vec<String> = e
            .diags
            .iter()
            .filter(|d| d.start <= pos && pos <= d.end)
            .map(|d| match &d.source {
                Some(s) => format!("{} ({s})", d.message),
                None => d.message.clone(),
            })
            .collect();
        let doc = t.doc;
        let key = e.lsp_key.clone();
        let path = e.doc.path.clone();
        if !diag_text.is_empty() {
            self.hover = Some(HoverPopup { text: diag_text.join("\n\n"), pos });
        }
        if !on_text {
            return Task::none();
        }
        // While the program is paused, what matters about a name is its value.
        if diag_text.is_empty() {
            let (col, text) = (self.docs[&doc].doc.line_col(pos).1, self.docs[&doc].doc.line_slice(line).to_string());
            if self.debug_hover(&text, col, pos) {
                return Task::none();
            }
        }
        let (Some(key), Some(path)) = (key, path) else { return Task::none() };
        self.lsp_flush(doc);
        let Some(s) = self.lsp.get(&key).filter(|s| s.ready) else { return Task::none() };
        let id = s.client.hover(&path, &self.docs[&doc].doc.rope, pos);
        self.register(&key, id, Pending::Hover { pos });
        Task::none()
    }

    pub fn hover_at_caret(&mut self) -> Task {
        let Some((doc, path, head, key)) = self.with_server() else { return Task::none() };
        let id = self.lsp[&key].client.hover(&path, &self.docs[&doc].doc.rope, head);
        self.register(&key, id, Pending::Hover { pos: head });
        Task::none()
    }

    pub fn request_signature(&mut self) -> Task {
        let Some((doc, path, head, key)) = self.with_server() else { return Task::none() };
        let id = self.lsp[&key].client.signature(&path, &self.docs[&doc].doc.rope, head);
        self.register(&key, id, Pending::Signature);
        Task::none()
    }

    pub fn go_to_location(&mut self, method: &str) -> Task {
        let Some((doc, path, head, key)) = self.with_server() else {
            return self.toast(tr("No language server for this file"));
        };
        let id = self.lsp[&key].client.locations(method, &path, &self.docs[&doc].doc.rope, head);
        self.register(&key, id, Pending::Definition);
        Task::none()
    }

    pub fn find_references(&mut self) -> Task {
        let Some((doc, path, head, key)) = self.with_server() else {
            return self.toast(tr("No language server for this file"));
        };
        let id = self.lsp[&key].client.references(&path, &self.docs[&doc].doc.rope, head);
        self.register(&key, id, Pending::References);
        Task::none()
    }

    pub fn start_rename(&mut self) -> Task {
        let Some((t, e)) = self.active() else { return Task::none() };
        if e.lsp_key.is_none() {
            return self.toast(tr("No language server for this file"));
        }
        let (a, b) = commands::word_at(&e.doc, t.cursors.head());
        if a == b {
            return self.toast(tr("Nothing to rename here"));
        }
        self.dialog_input = e.doc.slice_string(a, b);
        self.dialog = Some(DialogState::RenameSymbol);
        super::explorer::dialog_focus()
    }

    pub fn do_rename(&mut self, new_name: String) -> Task {
        if new_name.trim().is_empty() {
            return Task::none();
        }
        let Some((doc, path, head, key)) = self.with_server() else { return Task::none() };
        let id = self.lsp[&key].client.rename(&path, &self.docs[&doc].doc.rope, head, new_name.trim());
        self.register(&key, id, Pending::Rename);
        Task::none()
    }

    pub fn format_document(&mut self) -> Task {
        let Some((doc, path, _, key)) = self.with_server() else {
            return self.toast(tr("No language server for this file"));
        };
        let i = self.docs[&doc].doc.indent;
        let id = self.lsp[&key].client.format(&path, i.width, i.spaces);
        self.register(&key, id, Pending::Format { save_tab: None });
        Task::none()
    }

    /// True if a format request went out and the save will follow its answer.
    pub fn lsp_format_then_save(&mut self, tab: TabId) -> bool {
        let Some((doc, path, _, key)) = self.with_server() else { return false };
        if !self.lsp[&key].client.has_capability("documentFormattingProvider") {
            return false;
        }
        let i = self.docs[&doc].doc.indent;
        let id = self.lsp[&key].client.format(&path, i.width, i.spaces);
        self.register(&key, id, Pending::Format { save_tab: Some(tab) });
        true
    }

    pub fn request_code_actions(&mut self) -> Task {
        let Some((doc, path, _, key)) = self.with_server() else {
            return self.toast(tr("No language server for this file"));
        };
        let Some((t, e)) = self.active() else { return Task::none() };
        let p = t.cursors.primary();
        let (a, b) = (p.start(), p.end());
        let diags: Vec<serde_json::Value> = e.diags.iter().filter(|d| d.start <= b && a <= d.end).map(|d| d.raw.clone()).collect();
        let id = self.lsp[&key].client.code_actions(&path, &self.docs[&doc].doc.rope, a, b, &diags);
        self.register(&key, id, Pending::Actions);
        Task::none()
    }

    pub fn run_code_action(&mut self, action: core_lsp::CodeAction) -> Task {
        let key = self.active().and_then(|(_, e)| e.lsp_key.clone());
        if let Some(edit) = &action.edit {
            self.apply_workspace_edit(edit);
        } else if let Some(k) = &key
            && action.command.is_none()
            && let Some(s) = self.lsp.get(k)
        {
            let id = s.client.resolve_action(&action.raw);
            let k = k.clone();
            self.register(&k, id, Pending::ActionResolve);
        }
        if let (Some((c, args)), Some(k)) = (&action.command, &key)
            && let Some(s) = self.lsp.get(k)
        {
            s.client.execute_command(c, args);
        }
        Task::none()
    }

    pub fn request_symbols(&mut self) {
        let Some((doc, path, _, key)) = self.with_server() else { return };
        let id = self.lsp[&key].client.symbols(&path);
        self.register(&key, id, Pending::Symbols { doc });
    }

    // Completion ---------------------------------------------------------------------------

    /// Called after typing. Opens suggestions when the text calls for them.
    pub fn after_typing(&mut self, text: &str) -> Task {
        let Some(ch) = text.chars().last() else { return Task::none() };
        let doc = self.active().map(|(t, _)| t.doc);
        // Signature help follows parentheses.
        if matches!(ch, '(' | ',') {
            let t = self.request_signature();
            if ch == '(' {
                self.completion = None;
            }
            if self.completion.is_none() {
                return t;
            }
        }
        if ch == ')' {
            self.signature = None;
        }
        if !self.settings.auto_complete {
            return Task::none();
        }
        let is_trigger = doc
            .and_then(|d| self.server_for(d))
            .map(|s| s.client.trigger_characters("completionProvider").iter().any(|t| t.ends_with(ch)))
            .unwrap_or(false);
        if ch.is_ascii_digit() && self.completion.is_none() {
            // Numbers aren't words anyone wants finished, unless it's the end of an identifier.
            let starts_number = self.active().is_some_and(|(t, e)| {
                let head = t.cursors.head();
                let (a, _) = commands::word_at(&e.doc, head);
                e.doc.char_at(a).is_some_and(|c| c.is_ascii_digit())
            });
            if starts_number {
                return Task::none();
            }
        }
        if commands::is_word_char(ch) {
            if self.completion.is_some() {
                self.refilter_completion();
                if self.completion.is_some() {
                    return Task::none();
                }
            }
            return self.trigger_completion(None);
        }
        if is_trigger {
            return self.trigger_completion(Some(ch.to_string()));
        }
        self.completion = None;
        Task::none()
    }

    pub fn after_deleting(&mut self) -> Task {
        if self.completion.is_some() {
            self.refilter_completion();
        }
        Task::none()
    }

    pub fn trigger_completion(&mut self, trigger: Option<String>) -> Task {
        let settings_word = self.settings.word_completion;
        let settings_snip = self.settings.snippets;
        let Some(tab_id) = self.active_tab_id() else { return Task::none() };
        let snippets = if settings_snip { self.snippets_for_active() } else { Vec::new() };
        let Some(t) = self.tabs.get(&tab_id) else { return Task::none() };
        let head = t.cursors.head();
        // In a comment or a string, the language's words and snippets are only in the way. The
        // line just typed on is parsed first, so that's known now and not a frame later.
        let doc_id = t.doc;
        let in_text = match self.docs.get_mut(&doc_id) {
            Some(e) => {
                let anchor = commands::word_at(&e.doc, head).0.min(head);
                let (line, col) = e.doc.line_col(anchor);
                let rope = e.doc.rope.clone();
                e.doc.highlighter.as_mut().is_some_and(|h| {
                    h.advance(&rope, std::time::Duration::from_millis(3));
                    h.is_noncode(line, col)
                })
            }
            None => return Task::none(),
        };
        let snippets = if in_text { Vec::new() } else { snippets };
        let Some(e) = self.docs.get(&doc_id) else { return Task::none() };
        let anchor = commands::word_at(&e.doc, head).0.min(head);
        let prefix = e.doc.slice_string(anchor, head);
        let mut entries = Vec::new();
        // The language's own words, by what's typed so far: keywords, and what comes with it.
        let lower = prefix.to_lowercase();
        let language_words = if in_text { Vec::new() } else { tinta_core::keywords::for_language(&e.doc.language.name) };
        if !prefix.is_empty() {
            for (w, kind) in &language_words {
                if !w.to_lowercase().starts_with(&lower) || *w == prefix {
                    continue;
                }
                let (tag, detail, rank) = match kind {
                    tinta_core::keywords::Kind::Keyword => ("kw", tr("keyword"), 0),
                    tinta_core::keywords::Kind::Builtin => ("std", tr("built-in"), 1),
                };
                // `p` puts `pub` and `print!` before `PartialEq`, and keywords before the rest.
                let case = if w.starts_with(prefix.as_str()) { 0 } else { 1 };
                entries.push(CompletionEntry {
                    filter: w.to_string(),
                    sort: format!("x{case}{rank}{w}"),
                    label: w.to_string(),
                    detail: detail.to_string(),
                    kind: tag,
                    source: CompletionSource::Language,
                    positions: Vec::new(),
                });
            }
        }
        if settings_word && !prefix.is_empty() {
            for w in commands::words_with_prefix(&e.doc, &prefix, head, 30) {
                if language_words.iter().any(|(k, _)| *k == w) {
                    continue;
                }
                entries.push(CompletionEntry {
                    filter: w.clone(),
                    sort: format!("z{w}"),
                    label: w,
                    detail: String::new(),
                    kind: "abc",
                    source: CompletionSource::Word,
                    positions: Vec::new(),
                });
            }
        }
        for d in snippets {
            entries.push(CompletionEntry {
                filter: d.prefix.clone(),
                sort: format!("y{}", d.prefix),
                label: d.prefix.clone(),
                detail: d.description.clone(),
                kind: "snip",
                source: CompletionSource::Snippet(d.body),
                positions: Vec::new(),
            });
        }
        let generation = self.completion.as_ref().map_or(1, |c| c.generation + 1);
        let server = self.with_server();
        self.completion = Some(Completion { tab: tab_id, anchor, entries, visible: Vec::new(), selected: 0, generation, waiting: server.is_some(), navigated: false });
        self.refilter_completion();
        if let Some((doc, path, head, key)) = server {
            let id = self.lsp[&key].client.completion(&path, &self.docs[&doc].doc.rope, head, trigger.as_deref());
            self.register(&key, id, Pending::Completion { tab: tab_id, generation });
        }
        Task::none()
    }

    /// Narrows the list to what's been typed since it opened. Closes it if nothing's left.
    pub fn refilter_completion(&mut self) {
        let Some(c) = &self.completion else { return };
        let anchor = c.anchor;
        let waiting_on_server = c.waiting;
        let Some((t, e)) = self.active() else {
            self.completion = None;
            return;
        };
        let head = t.cursors.head();
        if head < anchor || e.doc.char_to_line(head) != e.doc.char_to_line(anchor) {
            self.completion = None;
            return;
        }
        let typed = e.doc.slice_string(anchor, head);
        // A word break after the anchor ends this completion.
        if typed.chars().any(|c| !commands::is_word_char(c)) {
            self.completion = None;
            return;
        }
        let Some(c) = &mut self.completion else { return };
        let hits = fuzzy::filter(c.entries.iter().map(|x| x.filter.as_str()), &typed, false, 100);
        let mut scored: Vec<(u32, usize, Vec<u32>)> = hits.into_iter().map(|h| (h.score, h.index, h.positions)).collect();
        if typed.is_empty() {
            scored.sort_by(|a, b| c.entries[a.1].sort.cmp(&c.entries[b.1].sort));
        } else {
            scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| c.entries[a.1].sort.cmp(&c.entries[b.1].sort)));
        }
        c.visible.clear();
        for (_, i, pos) in scored {
            c.entries[i].positions = pos;
            c.visible.push(i);
        }
        c.selected = c.selected.min(c.visible.len().saturating_sub(1));
        // Nothing to offer, or only exactly what's already there.
        let only_self = c.visible.len() == 1 && c.entries[c.visible[0]].label == typed;
        if (c.visible.is_empty() && !waiting_on_server) || only_self {
            self.completion = None;
        }
    }

    pub fn completion_move(&mut self, d: i32) -> Task {
        if let Some(c) = &mut self.completion {
            let n = c.visible.len() as i32;
            if n > 0 {
                c.selected = (c.selected as i32 + d).rem_euclid(n) as usize;
                c.navigated = true;
            }
        }
        Task::none()
    }

    pub fn accept_completion(&mut self) -> Task {
        let Some(c) = self.completion.take() else { return Task::none() };
        let Some(entry) = c.visible.get(c.selected).and_then(|i| c.entries.get(*i)).cloned() else { return Task::none() };
        match entry.source {
            CompletionSource::Word | CompletionSource::Language => {
                let label = entry.label;
                self.replace_range_with(c.anchor, &label);
            }
            CompletionSource::Snippet(body) => self.insert_snippet(&body, Some(c.anchor)),
            CompletionSource::Lsp(item) => {
                let rope = self.active().map(|(_, e)| e.doc.rope.clone());
                let Some(rope) = rope else { return Task::none() };
                let (start, text) = match &item.edit {
                    Some(ed) => (core_lsp::from_lsp(&rope, ed.range.start), ed.new_text.clone()),
                    None => (c.anchor, item.insert_text.clone()),
                };
                if item.is_snippet {
                    self.insert_snippet(&text, Some(start));
                } else {
                    self.replace_range_with(start, &text);
                }
                if !item.additional.is_empty() {
                    self.apply_text_edits_active(&item.additional);
                }
            }
        }
        self.after_caret_move();
        Task::none()
    }

    fn replace_range_with(&mut self, start: usize, text: &str) {
        let text = text.to_string();
        self.edit(|d, c, _| {
            let e = c.head();
            d.apply(vec![tinta_core::Edit::new(start, e, text)], c, tinta_core::EditKind::Other, |m, c| {
                c.set_single(Selection::cursor(m.after_edit(0)));
            });
        });
    }

    /// Puts a snippet at the cursor, replacing from `start` when given, and enters its stops.
    pub fn insert_snippet(&mut self, body: &str, start: Option<usize>) {
        let (name, dir) = match self.active() {
            Some((_, e)) => (
                e.doc.file_name(),
                e.doc.path.as_ref().and_then(|p| p.parent()).map(|p| p.display().to_string()).unwrap_or_default(),
            ),
            None => return,
        };
        let vars = |v: &str| -> Option<String> {
            match v {
                "TM_FILENAME" => Some(name.clone()),
                "TM_DIRECTORY" => Some(dir.clone()),
                "TM_FILENAME_BASE" => Some(name.rsplit_once('.').map_or(name.clone(), |(a, _)| a.to_string())),
                _ => None,
            }
        };
        let parsed = snippet::parse(body, &vars);
        // Continuation lines line up with where the snippet starts.
        let (indent, unit) = {
            let Some((t, e)) = self.active() else { return };
            let l = e.doc.char_to_line(t.cursors.head());
            (e.doc.line_indent(l), e.doc.indent.unit())
        };
        let parsed = reindent_stops(&parsed, &indent, &unit);
        let text = parsed.text.clone();
        let mut base = 0;
        self.edit(|d, c, _| {
            let p = c.primary();
            let a = start.unwrap_or(p.start());
            let b = p.end();
            d.apply(vec![tinta_core::Edit::new(a, b, text)], c, tinta_core::EditKind::Other, |m, c| {
                base = m.before_edit(0);
                c.set_single(Selection::cursor(m.after_edit(0)));
            });
        });
        let session = snippet::Session::new(&parsed, base);
        if let Some((t, _)) = self.active_mut() {
            match session {
                Some(s) => {
                    set_cursors_to_stop(&mut t.cursors, s.current());
                    t.snippet = Some(s);
                }
                None => {
                    if let Some(z) = parsed.stops.get(&0).and_then(|r| r.first()) {
                        t.cursors.set_single(Selection::cursor(base + z.0));
                    }
                }
            }
        }
    }

    /// Tab inside a snippet. True when it was used.
    pub fn snippet_step(&mut self, forward: bool) -> bool {
        let Some((t, e)) = self.active_mut() else { return false };
        let Some(s) = &mut t.snippet else { return false };
        let stop = if forward { s.next().map(<[_]>::to_vec) } else { s.prev().map(<[_]>::to_vec) };
        match stop {
            Some(r) => {
                let last = s.is_last();
                set_cursors_to_stop(&mut t.cursors, &r);
                if last {
                    t.snippet = None;
                }
                e.doc.break_undo();
                self.want_reveal = true;
                true
            }
            None => {
                t.snippet = None;
                false
            }
        }
    }

    /// `fn` followed by Tab expands when there's a snippet with that exact prefix.
    pub fn expand_snippet_prefix(&mut self) -> bool {
        if !self.settings.snippets || self.completion.is_some() {
            return false;
        }
        let Some((t, e)) = self.active() else { return false };
        if t.cursors.is_multi() || !t.cursors.primary().is_empty() {
            return false;
        }
        let head = t.cursors.head();
        let (a, b) = commands::word_at(&e.doc, head);
        if b != head || a == b {
            return false;
        }
        let word = e.doc.slice_string(a, b);
        let Some(def) = self.snippets_for_active().into_iter().find(|d| d.prefix == word) else { return false };
        self.insert_snippet(&def.body, Some(a));
        true
    }

    // Edits from servers -------------------------------------------------------------------

    fn apply_text_edits_active(&mut self, edits: &[TextEdit]) {
        let Some((_, e)) = self.active() else { return };
        let list = core_lsp::to_edits(&e.doc.rope, edits);
        self.edit(|d, c, _| {
            d.apply(list, c, tinta_core::EditKind::Other, |m, c| {
                c.map(|s| Selection {
                    anchor: m.map(s.anchor, tinta_core::buffer::Assoc::After),
                    head: m.map(s.head, tinta_core::buffer::Assoc::After),
                    goal: None,
                });
            });
        });
    }

    pub fn apply_workspace_edit(&mut self, edit: &WorkspaceEdit) {
        for (path, edits) in &edit.changes {
            if edits.is_empty() {
                continue;
            }
            if let Some(id) = self.doc_by_path(path) {
                let Some(tab) = self.tabs.values_mut().find(|t| t.doc == id) else { continue };
                let Some(e) = self.docs.get_mut(&id) else { continue };
                let list = core_lsp::to_edits(&e.doc.rope, edits);
                let tid = tab.id;
                e.doc.apply(list, &mut tab.cursors, tinta_core::EditKind::Other, |m, c| {
                    c.map(|s| Selection {
                        anchor: m.map(s.anchor, tinta_core::buffer::Assoc::Before),
                        head: m.map(s.head, tinta_core::buffer::Assoc::Before),
                        goal: None,
                    });
                });
                self.edit_tab = Some(tid);
            } else if let Ok(mut doc) = tinta_core::Document::open(path, self.default_indent()) {
                // Files not open change on disk.
                let list = core_lsp::to_edits(&doc.rope, edits);
                let mut c = tinta_core::Cursors::default();
                doc.apply(list, &mut c, tinta_core::EditKind::Other, |_, _| {});
                if let Err(err) = doc.save_to(path) {
                    self.output.push(format!("{}: {err}", path.display()));
                }
            }
        }
    }

    // What comes back ----------------------------------------------------------------------

    pub fn on_lsp_event(&mut self, ev: Event) -> Task {
        match ev {
            Event::Ready { server, capabilities } => {
                let mut opened = Vec::new();
                if let Some(entry) = self.lsp.get_mut(&server) {
                    entry.ready = true;
                    entry.client.capabilities = *capabilities;
                    opened = std::mem::take(&mut entry.waiting_docs);
                }
                for d in opened {
                    let Some(e) = self.docs.get_mut(&d) else { continue };
                    let Some(p) = e.doc.path.clone() else { continue };
                    let _ = e.doc.take_lsp_changes();
                    e.lsp_dirty = false;
                    if let Some(entry) = self.lsp.get(&server) {
                        let id = e.doc.language.lsp_id.unwrap_or(&entry.lang_id);
                        entry.client.did_open(&p, id, &e.doc);
                    }
                }
                self.output.push(format!("[{server}] ready"));
                Task::none()
            }
            Event::Diagnostics { path, items } => {
                let target = std::fs::canonicalize(&path).unwrap_or(path);
                if let Some(id) = self.doc_by_path(&target)
                    && let Some(e) = self.docs.get_mut(&id)
                {
                    let rope = e.doc.rope.clone();
                    // What the last build found stays until the next build.
                    let from_build: Vec<Diag> = e.diags.iter().filter(|d| d.source.as_deref() == Some(super::tasks::TASK_SOURCE)).cloned().collect();
                    e.diags = items
                        .iter()
                        .map(|d| {
                            let start = core_lsp::from_lsp(&rope, d.range.start);
                            let end = core_lsp::from_lsp(&rope, d.range.end);
                            Diag { start, end, severity: d.severity, message: d.message.clone(), source: d.source.clone(), raw: d.raw.clone() }
                        })
                        .collect();
                    e.diags.extend(from_build);
                    e.diags.sort_by_key(|d| d.start);
                }
                if items.is_empty() {
                    self.project_diags.remove(&target);
                } else {
                    self.project_diags.insert(target, items);
                }
                Task::none()
            }
            Event::Reply { server, id, reply } => {
                let Some(p) = self.lsp_pending.remove(&(server.clone(), id)) else { return Task::none() };
                self.on_reply(p, reply)
            }
            Event::ApplyEdit { edit, .. } => {
                self.apply_workspace_edit(&edit);
                Task::none()
            }
            Event::Progress { message, .. } => {
                self.lsp_progress = message;
                Task::none()
            }
            Event::Message { server, severity, text } => {
                self.output.push(format!("[{server}] {text}"));
                if severity <= 2 {
                    return self.toast(text);
                }
                Task::none()
            }
            Event::Log { server, text } => {
                self.output.push(format!("[{server}] {text}"));
                if self.output.len() > 5000 {
                    self.output.drain(..1000);
                }
                Task::none()
            }
            Event::Exited { server } => {
                let was_running = self.lsp.remove(&server).is_some();
                self.lsp_pending.retain(|(s, _), _| *s != server);
                for e in self.docs.values_mut() {
                    if e.lsp_key.as_ref() == Some(&server) {
                        e.lsp_key = None;
                    }
                }
                if was_running {
                    return self.toast(format!("{} {server} {}", tr("Language server"), tr("stopped")));
                }
                Task::none()
            }
        }
    }

    fn on_reply(&mut self, pending: Pending, reply: Reply) -> Task {
        if let Reply::Error(e) = &reply {
            self.output.push(format!("LSP: {e}"));
            // A save waiting on the formatter still has to happen.
            if let Pending::Format { save_tab: Some(id) } = pending
                && let Some(p) = self.tabs.get(&id).and_then(|t| self.docs.get(&t.doc)).and_then(|e| e.doc.path.clone())
            {
                return self.finish_save(id, p);
            }
            if !matches!(pending, Pending::Hover { .. } | Pending::Completion { .. } | Pending::Signature | Pending::Symbols { .. }) {
                return self.toast(e.clone());
            }
            return Task::none();
        }
        match (pending, reply) {
            (Pending::Completion { tab, generation }, Reply::Completion { items, .. }) => {
                let Some(c) = &mut self.completion else { return Task::none() };
                if c.tab != tab || c.generation != generation {
                    return Task::none();
                }
                c.waiting = false;
                c.entries.retain(|e| !matches!(e.source, CompletionSource::Lsp(_)));
                let names: std::collections::HashSet<String> = items.iter().map(|i| i.label.trim().to_string()).collect();
                // Words the server already offers aren't worth listing twice.
                c.entries.retain(|e| !(matches!(e.source, CompletionSource::Word | CompletionSource::Language) && names.contains(&e.label)));
                for i in items {
                    c.entries.push(CompletionEntry {
                        label: i.label.trim().to_string(),
                        detail: i.detail.clone().unwrap_or_default(),
                        kind: i.kind_label(),
                        filter: i.filter_text.trim().to_string(),
                        sort: format!("a{}", i.sort_text),
                        source: CompletionSource::Lsp(Box::new(i)),
                        positions: Vec::new(),
                    });
                }
                self.refilter_completion();
            }
            (Pending::Hover { pos }, Reply::Hover(text)) => {
                if self.palette.is_none() && !text.trim().is_empty() {
                    // Keep a problem's text already on show, with the docs below it.
                    let text = match &self.hover {
                        Some(h) if h.pos == pos && !h.text.contains(&text) => format!("{}\n\n{text}", h.text),
                        _ => text,
                    };
                    self.hover = Some(HoverPopup { text, pos });
                }
            }
            (Pending::Signature, Reply::Signature { label, active, .. }) => {
                let pos = self.active().map(|(t, _)| t.cursors.head()).unwrap_or(0);
                self.signature = Some(SignaturePopup { label, active, pos });
            }
            (Pending::Definition, Reply::Locations(list)) => {
                if list.is_empty() {
                    return self.toast(tr("No definition found"));
                }
                if list.len() == 1 {
                    let l = &list[0];
                    return self.open_location(l.path.clone(), l.range.start);
                }
                return self.show_locations(list);
            }
            (Pending::References, Reply::Locations(list)) => {
                if list.is_empty() {
                    return self.toast(tr("No references found"));
                }
                return self.show_locations(list);
            }
            (Pending::Rename, Reply::Workspace(edit)) => {
                self.apply_workspace_edit(&edit);
            }
            (Pending::Format { save_tab }, Reply::Edits(edits)) => {
                if !edits.is_empty() {
                    self.apply_text_edits_active(&edits);
                }
                if let Some(id) = save_tab
                    && let Some(p) = self.tabs.get(&id).and_then(|t| self.docs.get(&t.doc)).and_then(|e| e.doc.path.clone())
                {
                    return self.finish_save(id, p);
                }
            }
            (Pending::Actions, Reply::Actions(mut actions)) => {
                if actions.is_empty() {
                    return self.toast(tr("No quick fixes available"));
                }
                actions.sort_by_key(|a| !a.preferred);
                let items = actions
                    .iter()
                    .enumerate()
                    .map(|(i, a)| PItem { label: a.title.clone(), detail: String::new(), hint: String::new(), positions: Vec::new(), action: PAction::Action(i) })
                    .collect();
                return self.open_palette_with(PaletteMode::Actions, items, actions);
            }
            (Pending::ActionResolve, Reply::Action(a)) => {
                if let Some(edit) = &a.edit {
                    self.apply_workspace_edit(edit);
                }
            }
            (Pending::Symbols { doc }, Reply::Symbols(list)) => {
                if let Some(e) = self.docs.get_mut(&doc) {
                    e.lsp_symbols = Some((e.doc.revision, list));
                }
                if self.palette.as_ref().is_some_and(|p| p.mode == PaletteMode::Symbols) {
                    self.refilter_palette();
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn open_location(&mut self, path: PathBuf, at: core_lsp::Pos) -> Task {
        self.push_nav();
        let t = self.open_path(&path);
        if let Some((tab, e)) = self.active_mut() {
            let p = core_lsp::from_lsp(&e.doc.rope, at);
            tab.cursors.set_single(Selection::cursor(p));
        }
        self.center_active();
        self.focus = Focus::Editor;
        self.after_caret_move();
        t
    }

    fn show_locations(&mut self, list: Vec<core_lsp::Location>) -> Task {
        let items = list
            .iter()
            .map(|l| {
                let rel = self.folder.as_ref().and_then(|f| l.path.strip_prefix(f).ok()).unwrap_or(&l.path);
                // The line's text, so the list says what each place is.
                let preview = self
                    .doc_by_path(&l.path)
                    .and_then(|d| self.docs.get(&d))
                    .filter(|e| l.range.start.line < e.doc.len_lines())
                    .map(|e| e.doc.line_text(l.range.start.line).trim().to_string())
                    .or_else(|| std::fs::read_to_string(&l.path).ok().and_then(|s| s.lines().nth(l.range.start.line).map(|x| x.trim().to_string())))
                    .unwrap_or_default();
                PItem {
                    label: format!("{}:{}", rel.display(), l.range.start.line + 1),
                    detail: preview,
                    hint: String::new(),
                    positions: Vec::new(),
                    action: PAction::Location(l.path.clone(), l.range.start.line, l.range.start.character),
                }
            })
            .collect();
        self.open_palette_with(PaletteMode::Locations, items, Vec::new())
    }

    pub fn problems(&self) -> Vec<(PathBuf, usize, usize, tinta_core::lsp::Severity, String)> {
        let mut out = Vec::new();
        for (path, list) in &self.project_diags {
            for d in list {
                out.push((path.clone(), d.range.start.line, d.range.start.character, d.severity, d.message.clone()));
            }
        }
        for (path, p) in &self.tasks.problems {
            out.push((path.clone(), p.location.line, p.location.col, p.severity, p.message.clone()));
        }
        out.sort_by(|a, b| (a.3 as u8, &a.0, a.1).cmp(&(b.3 as u8, &b.0, b.1)));
        out
    }

    pub fn diag_step(&mut self, forward: bool) {
        let Some((t, e)) = self.active() else { return };
        if e.diags.is_empty() {
            return;
        }
        let head = t.cursors.head();
        let i = if forward {
            e.diags.iter().position(|d| d.start > head).unwrap_or(0)
        } else {
            e.diags.iter().rposition(|d| d.start < head).unwrap_or(e.diags.len() - 1)
        };
        let d = &e.diags[i];
        let (l, c) = e.doc.line_col(d.start);
        let hover = HoverPopup { text: d.message.clone(), pos: d.start };
        self.jump_to(l, c);
        self.hover = Some(hover);
    }
}

fn set_cursors_to_stop(c: &mut tinta_core::Cursors, ranges: &[(usize, usize)]) {
    let sels: Vec<Selection> = ranges.iter().map(|(a, b)| Selection::new(*a, *b)).collect();
    if !sels.is_empty() {
        *c = tinta_core::Cursors::from_vec(sels, 0);
    }
}

fn expand_leading(line: &str, unit: &str) -> String {
    // Bodies are written with four spaces or tabs per level.
    let spaces = line.chars().take_while(|c| *c == ' ').count();
    let tabs = line.chars().take_while(|c| *c == '\t').count();
    if tabs > 0 {
        return format!("{}{}", unit.repeat(tabs), &line[tabs..]);
    }
    let levels = spaces / 4;
    format!("{}{}", unit.repeat(levels), &line[levels * 4..])
}

/// Later lines of a snippet get the current line's indentation, tabs in the body become the
/// file's own indent unit, and the tab stops move along with the text.
fn reindent_stops(p: &snippet::Parsed, indent: &str, unit: &str) -> snippet::Parsed {
    let map_pos = |pos: usize| -> usize {
        let mut chars_seen = 0usize; // chars of the original consumed
        let mut out = 0usize;
        for (i, line) in p.text.split('\n').enumerate() {
            let n = line.chars().count();
            let new_line = expand_leading(line, unit);
            let extra = if i > 0 { indent.chars().count() + 1 } else { 0 };
            if pos <= chars_seen + n {
                let in_line = pos - chars_seen;
                let delta = new_line.chars().count() as isize - n as isize;
                // Past the leading whitespace the whole shift applies.
                let lead = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
                let shifted = if in_line >= lead { (in_line as isize + delta) as usize } else { in_line };
                return out + extra + shifted;
            }
            chars_seen += n + 1;
            out += extra + new_line.chars().count();
        }
        out
    };
    let mut stops = std::collections::BTreeMap::new();
    for (k, v) in &p.stops {
        stops.insert(*k, v.iter().map(|(a, b)| (map_pos(*a), map_pos(*b))).collect());
    }
    let mut text = String::with_capacity(p.text.len());
    for (i, line) in p.text.split('\n').enumerate() {
        if i > 0 {
            text.push('\n');
            text.push_str(indent);
        }
        text.push_str(&expand_leading(line, unit));
    }
    snippet::Parsed { text, stops }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_reindent_moves_stops_with_the_text() {
        let p = snippet::parse("fn ${1:a}() {\n    $0\n}", &|_| None);
        let r = reindent_stops(&p, "  ", "\t");
        assert_eq!(r.text, "fn a() {\n  \t\n  }");
        assert_eq!(r.stops[&1], vec![(3, 4)]);
        let zero = r.stops[&0][0].0;
        assert_eq!(r.text.chars().take(zero).collect::<String>(), "fn a() {\n  \t");
    }
}
