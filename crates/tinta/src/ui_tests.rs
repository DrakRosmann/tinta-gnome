//! The app driven the way the window drives it, without a window: keys and commands go to
//! `update`, and the mouse goes through the text area's own event handling, at the same grid
//! positions it paints. Clicking menus and the like is left to the end-to-end tests in GNOME.

use crate::app::{App, Flags, Focus, Message, SidebarView};
use crate::bus;
use crate::fx::{Effect, Task};
use crate::gfx::{self, Event, Modifiers, Point, Rectangle, Size, keyboard, mouse, window};
use crate::gui::editor_view_for;
use crate::state::TabKind;
use crate::ui::diff_view::{self, DiffView};
use crate::ui::editor_view::{self, Geometry};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::Instant;
use tinta_core::keymap::Command;

/// The bus is global and settings live in the environment: one test at a time.
static LOCK: Mutex<()> = Mutex::new(());
static INIT: Once = Once::new();

const SIZE: Size = Size { width: 1000.0, height: 700.0 };

struct Ui {
    app: App,
    state: editor_view::State,
    diff_state: diff_view::State,
    cursor: Point,
    mods: Modifiers,
    clipboard: Option<String>,
    _guard: MutexGuard<'static, ()>,
}

fn temp_dir(name: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir().join(format!("tinta-ui-{}-{}-{name}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

const SOURCE: &str = "fn main() {\n    let alpha = 1;\n    let beta = alpha + 2;\n    if beta > 2 {\n        println!(\"{beta}\");\n    }\n}\n\nfn other() {\n    let gamma = 3;\n}\n";

impl Ui {
    fn lock() -> MutexGuard<'static, ()> {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        INIT.call_once(|| {
            let base = temp_dir("env");
            // SAFETY: done once, before this test's threads read the environment.
            unsafe {
                std::env::set_var("XDG_CONFIG_HOME", base.join("config"));
                std::env::set_var("XDG_STATE_HOME", base.join("state"));
                std::env::remove_var("TINTA_SCRIPT");
            }
        });
        guard
    }

    fn new(targets: Vec<PathBuf>) -> Self {
        let guard = Self::lock();
        // Earlier tests may have saved settings; each starts from the defaults.
        let _ = std::fs::remove_file(tinta_core::settings::settings_path());
        let _ = bus::drain();
        let flags = Flags { targets: targets.into_iter().map(|p| (p, None)).collect(), new_session: true, dark: true, system_mono: None, font_names: Vec::new() };
        let (app, first) = App::new(flags);
        let mut ui = Ui { app, state: Default::default(), diff_state: Default::default(), cursor: Point::ORIGIN, mods: Modifiers::empty(), clipboard: None, _guard: guard };
        ui.run(first);
        ui.frame();
        ui.frame();
        ui
    }

    /// What the window would do with the effects: messages go back in, the clipboard is a string.
    fn run(&mut self, task: Task) {
        let mut queue: Vec<Message> = Vec::new();
        for e in task.0 {
            match e {
                Effect::Message(m) => queue.push(m),
                Effect::ClipboardWrite(t) => self.clipboard = Some(t),
                Effect::ClipboardRead(then) => queue.push(then(self.clipboard.clone())),
                _ => {}
            }
        }
        for m in queue {
            let t = self.app.update(m);
            self.run(t);
        }
    }

    fn msg(&mut self, m: Message) {
        let t = self.app.update(m);
        self.run(t);
        self.frame();
    }

    /// One event through the active tab's text area, then whatever it and the background
    /// threads produced, until things settle.
    fn send(&mut self, events: Vec<Event>) {
        let mut pending = events;
        for _ in 0..12 {
            let mut messages: Vec<Message> = Vec::new();
            let bounds = Rectangle::with_size(SIZE);
            let cursor = mouse::Cursor::Available(self.cursor);
            if let Some(tab) = self.app.active_tab_id() {
                self.state.cell_w = self.app.metrics.cell_w;
                self.diff_state.cell_w = self.app.metrics.cell_w;
                for ev in &pending {
                    let mut out = gfx::Shell::<Message>::default();
                    let is_diff = self.app.tabs.get(&tab).is_some_and(|t| t.kind == TabKind::Diff);
                    if is_diff {
                        let Some(data) = self.app.tabs.get(&tab).and_then(|t| t.diff.as_deref()) else { continue };
                        let view = DiffView { data, metrics: self.app.metrics, on_event: Box::new(move |e| Message::Diff(tab, e)) };
                        view.update(&mut self.diff_state, ev, bounds, cursor, &mut out);
                    } else if let Some(view) = editor_view_for(&self.app, tab, true) {
                        view.update(&mut self.state, ev, bounds, cursor, &mut out);
                    }
                    messages.extend(out.messages);
                }
            }
            pending = Vec::new();
            messages.extend(bus::drain());
            if messages.is_empty() {
                return;
            }
            for m in messages {
                let t = self.app.update(m);
                self.run(t);
            }
        }
    }

    fn frame(&mut self) {
        self.send(vec![Event::Window(window::Event::RedrawRequested(Instant::now()))]);
    }

    fn set_mods(&mut self, m: Modifiers) {
        self.mods = m;
        self.send(vec![Event::Keyboard(keyboard::Event::ModifiersChanged(m))]);
    }

    fn move_to(&mut self, p: Point) {
        self.cursor = p;
        self.send(vec![Event::Mouse(mouse::Event::CursorMoved { position: p })]);
    }

    fn press(&mut self, b: mouse::Button) {
        self.send(vec![Event::Mouse(mouse::Event::ButtonPressed(b))]);
    }

    fn release(&mut self, b: mouse::Button) {
        self.send(vec![Event::Mouse(mouse::Event::ButtonReleased(b))]);
    }

    fn click_at(&mut self, p: Point) {
        self.move_to(p);
        self.press(mouse::Button::Left);
        self.release(mouse::Button::Left);
    }

    /// The text area starts at its own corner.
    fn origin_from_view(&self) -> Point {
        Point::ORIGIN
    }

    /// Where a document cell is drawn, using the same geometry the widget paints with.
    fn cell(&self, line: usize, col: usize) -> Point {
        let (t, e) = self.app.active().unwrap();
        let g = Geometry::new(e.doc.len_lines(), t.view_w, &self.app.metrics, &self.app.settings);
        Point::new(g.text_x + (col as f32 + 0.2) * self.app.metrics.cell_w, (line as f32 + 0.5) * self.app.metrics.line_h - t.scroll_y)
    }

    fn head(&self) -> (usize, usize) {
        let (t, e) = self.app.active().unwrap();
        e.doc.line_col(t.cursors.head())
    }

    fn selection_text(&self) -> String {
        let (t, e) = self.app.active().unwrap();
        let s = t.cursors.primary();
        e.doc.slice_string(s.start(), s.end())
    }

    fn text(&self) -> String {
        self.app.active().unwrap().1.doc.rope.to_string()
    }

    /// The editor closes, and the test keeps the lock: no other test touches the saved session
    /// before this one opens it again.
    fn close(self) -> MutexGuard<'static, ()> {
        let Ui { _guard, .. } = self;
        _guard
    }

    fn cmd(&mut self, c: Command) {
        self.msg(Message::Cmd(c));
    }

    fn with_file() -> (Self, PathBuf) {
        let dir = temp_dir("file");
        let p = dir.join("a.rs");
        std::fs::write(&p, SOURCE).unwrap();
        let ui = Ui::new(vec![p.clone()]);
        (ui, p)
    }

    fn key(&mut self, name: &str, mods: Modifiers, text: Option<&str>) {
        let ev = crate::app::KeyEv { name: name.into(), shifted: None, mods, text: text.map(str::to_string), captured: false };
        self.msg(Message::Key(ev));
    }

    fn typed(&mut self, s: &str) {
        for c in s.chars() {
            self.key(&c.to_lowercase().to_string(), Modifiers::empty(), Some(&c.to_string()));
        }
    }
}

fn wait_for(ui: &mut Ui, mut done: impl FnMut(&App) -> bool) {
    for _ in 0..400 {
        ui.frame();
        if done(&ui.app) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting");
}

#[test]
fn the_editor_measures_itself_and_reports_its_size() {
    let (ui, _) = Ui::with_file();
    let (t, _) = ui.app.active().unwrap();
    assert!(t.view_w > 300.0 && t.view_h > 300.0, "{} x {}", t.view_w, t.view_h);
    assert!(ui.app.metrics.cell_w > 3.0, "{}", ui.app.metrics.cell_w);
}

#[test]
fn clicking_puts_the_caret_on_the_cell() {
    let (mut ui, _) = Ui::with_file();
    let p = ui.cell(2, 8);
    ui.click_at(p);
    assert_eq!(ui.head(), (2, 8));
    assert_eq!(ui.app.focus, Focus::Editor);
    // Past the end of a line lands at its end.
    let p = ui.cell(1, 60);
    ui.click_at(p);
    assert_eq!(ui.head(), (1, 18));
}

#[test]
fn double_and_triple_click_select_word_and_line() {
    let (mut ui, _) = Ui::with_file();
    let p = ui.cell(1, 9);
    ui.click_at(p);
    ui.click_at(p);
    assert_eq!(ui.selection_text(), "alpha");
    ui.click_at(p);
    assert_eq!(ui.selection_text(), "    let alpha = 1;\n");
}

#[test]
fn dragging_selects() {
    let (mut ui, _) = Ui::with_file();
    let a = ui.cell(1, 8);
    let b = ui.cell(2, 7);
    ui.move_to(a);
    ui.press(mouse::Button::Left);
    ui.move_to(b);
    ui.release(mouse::Button::Left);
    assert_eq!(ui.selection_text(), "alpha = 1;\n    let");
    // Moving afterwards doesn't keep selecting.
    let c = ui.cell(4, 3);
    ui.move_to(c);
    assert_eq!(ui.selection_text(), "alpha = 1;\n    let");
}

/// The click that closes a popup reaches the text, but its release goes to the popup. Found with
/// a real pointer: the drag never ended, and moving the mouse went on selecting.
#[test]
fn a_lost_release_does_not_leave_a_drag_running() {
    let (mut ui, _) = Ui::with_file();
    let a = ui.cell(1, 8);
    ui.move_to(a);
    ui.press(mouse::Button::Left);
    // No release: the pointer comes back to the window as the popup goes away.
    ui.send(vec![Event::Mouse(mouse::Event::CursorEntered)]);
    let b = ui.cell(4, 3);
    ui.move_to(b);
    assert_eq!(ui.selection_text(), "");
    assert_eq!(ui.head(), (1, 8));
}

#[test]
fn shift_click_extends_and_alt_click_adds_a_cursor() {
    let (mut ui, _) = Ui::with_file();
    let a = ui.cell(1, 8);
    ui.click_at(a);
    ui.set_mods(Modifiers::SHIFT);
    let b = ui.cell(1, 13);
    ui.click_at(b);
    assert_eq!(ui.selection_text(), "alpha");
    ui.set_mods(Modifiers::ALT);
    let c = ui.cell(2, 4);
    ui.click_at(c);
    assert_eq!(ui.app.active().unwrap().0.cursors.len(), 2);
    // Alt+click on a cursor takes it away (after long enough not to be a double click).
    std::thread::sleep(std::time::Duration::from_millis(420));
    ui.click_at(c);
    assert_eq!(ui.app.active().unwrap().0.cursors.len(), 1);
    ui.set_mods(Modifiers::empty());
}

#[test]
fn shift_alt_drag_makes_a_column_selection() {
    let (mut ui, _) = Ui::with_file();
    let a = ui.cell(1, 4);
    let b = ui.cell(2, 7);
    ui.set_mods(Modifiers::SHIFT | Modifiers::ALT);
    ui.move_to(a);
    ui.press(mouse::Button::Left);
    ui.move_to(b);
    ui.release(mouse::Button::Left);
    ui.set_mods(Modifiers::empty());
    let (t, e) = ui.app.active().unwrap();
    let parts: Vec<String> = t.cursors.iter().map(|s| e.doc.slice_string(s.start(), s.end())).collect();
    assert_eq!(parts, vec!["let", "let"]);
}

#[test]
fn wheel_scrolls_and_ctrl_wheel_zooms() {
    let dir = temp_dir("wheel");
    let p = dir.join("long.txt");
    std::fs::write(&p, "linha\n".repeat(500)).unwrap();
    let mut ui = Ui::new(vec![p]);
    let c = ui.cell(3, 1);
    ui.move_to(c);
    ui.send(vec![Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 } })]);
    let y = ui.app.active().unwrap().0.scroll_y;
    assert!(y > 0.0, "{y}");
    let size = ui.app.settings.font_size;
    ui.set_mods(Modifiers::CTRL);
    ui.send(vec![Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 } })]);
    ui.set_mods(Modifiers::empty());
    assert_eq!(ui.app.settings.font_size, size + 1.0);
    assert_eq!(ui.app.active().unwrap().0.scroll_y, y, "ctrl+wheel doesn't scroll");
}

#[test]
fn fold_arrow_in_the_gutter_folds_and_unfolds() {
    let (mut ui, _) = Ui::with_file();
    let rows = |ui: &Ui| {
        let (t, e) = ui.app.active().unwrap();
        t.map.row_count(&e.doc)
    };
    let before = rows(&ui);
    let (t, e) = ui.app.active().unwrap();
    let g = Geometry::new(e.doc.len_lines(), t.view_w, &ui.app.metrics, &ui.app.settings);
    let o = ui.origin_from_view();
    let arrow = Point::new(o.x + g.gutter_w - 8.0, o.y + ui.app.metrics.line_h / 2.0);
    ui.click_at(arrow);
    assert!(rows(&ui) < before, "folded");
    ui.frame();
    // A fold doesn't count as a press on the text.
    assert_eq!(ui.head(), (0, 0));
    ui.click_at(arrow);
    assert_eq!(rows(&ui), before, "unfolded");
}

#[test]
fn line_number_click_selects_the_line() {
    let (mut ui, _) = Ui::with_file();
    let o = ui.origin_from_view();
    let c = ui.cell(1, 0);
    let g = Geometry::new(SOURCE.lines().count() + 1, ui.app.active().unwrap().0.view_w, &ui.app.metrics, &ui.app.settings);
    let p = Point::new(o.x + g.gutter_w - 24.0, c.y);
    ui.click_at(p);
    assert_eq!(ui.selection_text(), "    let alpha = 1;\n");
}

#[test]
fn minimap_and_scrollbar_scroll() {
    let dir = temp_dir("mini");
    let p = dir.join("long.txt");
    std::fs::write(&p, "linha\n".repeat(2000)).unwrap();
    let mut ui = Ui::new(vec![p]);
    let (t, e) = ui.app.active().unwrap();
    let g = Geometry::new(e.doc.len_lines(), t.view_w, &ui.app.metrics, &ui.app.settings);
    let (view_h, o) = (t.view_h, ui.origin_from_view());
    assert!(g.minimap_w > 0.0);
    let on_minimap = Point::new(o.x + g.minimap_x + 20.0, o.y + view_h - 10.0);
    ui.click_at(on_minimap);
    let y1 = ui.app.active().unwrap().0.scroll_y;
    assert!(y1 > 0.0, "{y1}");
    let on_bar = Point::new(o.x + g.scrollbar_x + 6.0, o.y + view_h / 2.0);
    ui.click_at(on_bar);
    let y2 = ui.app.active().unwrap().0.scroll_y;
    assert!(y2 > 0.0 && y2 != y1, "{y1} {y2}");
    // Dragging the thumb keeps following the pointer.
    ui.move_to(on_bar);
    ui.press(mouse::Button::Left);
    ui.move_to(Point::new(on_bar.x, on_bar.y + 100.0));
    ui.release(mouse::Button::Left);
    assert!(ui.app.active().unwrap().0.scroll_y > y2);
}

#[test]
fn typing_goes_through_keys_with_auto_pairs_and_undo() {
    let (mut ui, _) = Ui::with_file();
    ui.cmd(Command::SelectAll);
    ui.typed("f(");
    assert_eq!(ui.text(), "f()");
    ui.typed(")");
    assert_eq!(ui.text(), "f()");
    assert_eq!(ui.head(), (0, 3));
    ui.cmd(Command::Undo);
    ui.cmd(Command::Undo);
    assert_eq!(ui.text(), SOURCE);
}

#[test]
fn keyboard_shortcuts_reach_commands() {
    let (mut ui, _) = Ui::with_file();
    ui.key("d", Modifiers::CTRL | Modifiers::SHIFT, Some("D"));
    assert!(ui.text().starts_with("fn main() {\nfn main() {\n"), "{}", ui.text());
    ui.key("k", Modifiers::CTRL, None);
    assert!(ui.app.keymap.is_pending());
    ui.key("0", Modifiers::CTRL, None);
    assert!(!ui.app.active().unwrap().0.folds.ranges.is_empty(), "ctrl+k ctrl+0 folds everything");
    ui.key("k", Modifiers::CTRL, None);
    ui.key("j", Modifiers::CTRL, None);
    assert!(ui.app.active().unwrap().0.folds.ranges.is_empty());
}

#[test]
fn escape_collapses_cursors_and_arrows_move() {
    let (mut ui, _) = Ui::with_file();
    ui.cmd(Command::AddCursorBelow);
    assert_eq!(ui.app.active().unwrap().0.cursors.len(), 2);
    ui.key("escape", Modifiers::empty(), None);
    assert_eq!(ui.app.active().unwrap().0.cursors.len(), 1);
    // The newest cursor is the one that stays: it was on the second line.
    assert_eq!(ui.head().0, 1);
    ui.key("down", Modifiers::empty(), None);
    ui.key("right", Modifiers::CTRL, None);
    assert_eq!(ui.head().0, 2);
    assert!(ui.head().1 > 0);
}

#[test]
fn explorer_click_opens_files_and_folders_expand() {
    let dir = temp_dir("tree");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/inner.txt"), "dentro\n").unwrap();
    std::fs::write(dir.join("top.txt"), "fora\n").unwrap();
    let mut ui = Ui::new(vec![dir]);
    assert_eq!(ui.app.sidebar, Some(SidebarView::Explorer));
    assert_eq!(ui.app.explorer.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["sub", "top.txt"]);
    ui.msg(Message::Explorer(crate::app::ExplorerMsg::Click(0)));
    assert_eq!(ui.app.explorer.rows.len(), 3, "{:?}", ui.app.explorer.rows.iter().map(|r| &r.name).collect::<Vec<_>>());
    ui.msg(Message::Explorer(crate::app::ExplorerMsg::Click(1)));
    assert_eq!(ui.app.active().unwrap().1.doc.file_name(), "inner.txt");
}

#[test]
fn explorer_keyboard_moves_opens_and_folds() {
    let dir = temp_dir("keys");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("sub/inner.txt"), "dentro\n").unwrap();
    std::fs::write(dir.join("alpha.txt"), "a\n").unwrap();
    std::fs::write(dir.join("beta.txt"), "b\n").unwrap();
    let mut ui = Ui::new(vec![dir.clone()]);
    let none = Modifiers::empty();
    let cursor = |ui: &Ui| ui.app.explorer.selected.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
    ui.cmd(Command::ShowExplorer);
    assert_eq!(ui.app.focus, Focus::Explorer);
    // Starts on the first row; the arrows walk the list and stop at its ends.
    ui.key("down", none, None);
    assert_eq!(cursor(&ui).as_deref(), Some("alpha.txt"));
    ui.key("end", none, None);
    ui.key("down", none, None);
    assert_eq!(cursor(&ui).as_deref(), Some("beta.txt"));
    ui.key("home", none, None);
    assert_eq!(cursor(&ui).as_deref(), Some("sub"));
    // Right opens the folder, then goes into it; left goes back out, then closes it.
    ui.key("right", none, None);
    assert_eq!(ui.app.explorer.rows.len(), 4);
    ui.key("right", none, None);
    assert_eq!(cursor(&ui).as_deref(), Some("inner.txt"));
    ui.key("left", none, None);
    assert_eq!(cursor(&ui).as_deref(), Some("sub"));
    ui.key("left", none, None);
    assert_eq!(ui.app.explorer.rows.len(), 3);
    // A letter jumps to the next name starting with it.
    ui.key("b", none, Some("b"));
    assert_eq!(cursor(&ui).as_deref(), Some("beta.txt"));
    // Space opens and stays in the tree; Enter opens and goes to the text.
    ui.key("space", none, None);
    assert_eq!(ui.app.active().unwrap().1.doc.file_name(), "beta.txt");
    assert_eq!(ui.app.focus, Focus::Explorer);
    ui.key("up", none, None);
    ui.key("enter", none, None);
    assert_eq!(ui.app.active().unwrap().1.doc.file_name(), "alpha.txt");
    assert_eq!(ui.app.focus, Focus::Editor);
    // Typing in the text doesn't move the tree.
    ui.typed("b");
    assert_eq!(cursor(&ui).as_deref(), Some("alpha.txt"));
    // F2 asks for a new name, Escape leaves the tree.
    ui.cmd(Command::ShowExplorer);
    ui.key("f2", none, None);
    assert!(matches!(ui.app.dialog, Some(crate::app::DialogState::RenamePath(ref p)) if p.ends_with("alpha.txt")));
    ui.key("escape", none, None);
    ui.cmd(Command::ShowExplorer);
    ui.key("escape", none, None);
    assert_eq!(ui.app.focus, Focus::Editor);
}

#[test]
fn narrow_window_floats_the_sidebar_and_puts_it_back() {
    let dir = temp_dir("narrow");
    std::fs::write(dir.join("one.txt"), "1\n").unwrap();
    let mut ui = Ui::new(vec![dir]);
    assert_eq!(ui.app.sidebar, Some(SidebarView::Explorer));
    // Narrow: the sidebar steps aside, comes back on demand with the keyboard on the first row,
    // and goes again once a file opens from it.
    ui.msg(Message::Narrow(true));
    assert_eq!(ui.app.sidebar, None);
    ui.cmd(Command::ShowExplorer);
    assert_eq!(ui.app.sidebar, Some(SidebarView::Explorer));
    assert_eq!(ui.app.explorer.selected.as_ref().and_then(|p| p.file_name()).and_then(|n| n.to_str()), Some("one.txt"));
    ui.key("enter", Modifiers::empty(), None);
    assert_eq!(ui.app.active().unwrap().1.doc.file_name(), "one.txt");
    assert_eq!(ui.app.sidebar, None);
    // Wide again: back as it was before the window shrank.
    ui.msg(Message::Narrow(false));
    assert_eq!(ui.app.sidebar, Some(SidebarView::Explorer));
}

#[test]
fn completion_offers_the_language_words_as_you_type() {
    let dir = temp_dir("kw");
    let src = dir.join("main.rs");
    std::fs::write(&src, "fn main() {\n    \n}\n").unwrap();
    let mut ui = Ui::new(vec![src]);
    let labels = |ui: &Ui| -> Vec<String> {
        let Some(c) = &ui.app.completion else { return Vec::new() };
        c.visible.iter().map(|i| c.entries[*i].label.clone()).collect()
    };
    ui.app.jump_to(1, 4);
    ui.typed("pri");
    let shown = labels(&ui);
    assert!(shown.contains(&"println!".to_string()) && shown.contains(&"print!".to_string()), "{shown:?}");
    assert!(!shown.contains(&"impl".to_string()), "only words starting with what's typed: {shown:?}");
    // Narrows as you go on, and Enter puts the word in.
    ui.typed("ntl");
    assert_eq!(labels(&ui).first().map(String::as_str), Some("println!"));
    ui.key("enter", Modifiers::empty(), None);
    assert_eq!(ui.app.active().unwrap().1.doc.line_text(1).trim_end(), "    println!");

    // Not in a comment or a string, where they'd only be in the way.
    ui.typed("(\"pri");
    assert!(!labels(&ui).contains(&"println!".to_string()), "in a string: {:?}", labels(&ui));
    ui.key("escape", Modifiers::empty(), None);
    ui.key("end", Modifiers::empty(), None);
    ui.key("enter", Modifiers::empty(), None);
    ui.typed("// pri");
    assert!(!labels(&ui).contains(&"println!".to_string()), "in a comment: {:?}", labels(&ui));
    ui.key("escape", Modifiers::empty(), None);
    ui.key("enter", Modifiers::empty(), None);
    ui.typed("pri");
    assert!(labels(&ui).contains(&"println!".to_string()), "back in code: {:?}", labels(&ui));
    ui.key("escape", Modifiers::empty(), None);

    // Each language its own: C has printf and its keywords, Rust's macros are not there.
    let c = dir.join("prog.c");
    std::fs::write(&c, "int main(void) {\n    \n}\n").unwrap();
    let t = ui.app.open_path(&c);
    ui.run(t);
    ui.app.jump_to(1, 4);
    ui.typed("pr");
    let shown = labels(&ui);
    assert!(shown.contains(&"printf".to_string()), "{shown:?}");
    assert!(!shown.contains(&"println!".to_string()), "{shown:?}");
    ui.key("escape", Modifiers::empty(), None);
    ui.typed(" w");
    assert!(labels(&ui).contains(&"while".to_string()), "{:?}", labels(&ui));
}

#[test]
fn a_language_without_its_server_says_how_to_get_one() {
    let dir = temp_dir("nolsp");
    let lua = dir.join("a.lua");
    std::fs::write(&lua, "print(1)\n").unwrap();
    let mut ui = Ui::new(Vec::new());
    // A server that can't be there, so the test doesn't depend on what this machine has.
    ui.app.settings.lsp_servers.insert("lua".into(), tinta_core::settings::LspServer { command: "nao-existe-servidor".into(), args: Vec::new() });
    let t = ui.app.open_path(&lua);
    ui.run(t);
    let (lang, text, has_command) = ui.app.lsp_hint().expect("a hint for Lua");
    assert_eq!(lang, "lua");
    assert!(text.contains("Lua") && text.contains("nao-existe-servidor"), "{text}");
    assert!(!has_command, "nothing to install for a command the user picked");
    // Closed: it stays closed for the session.
    ui.msg(Message::LspHintDone(lang));
    assert!(ui.app.lsp_hint().is_none());
    // Turned off on purpose: no hint at all.
    let py = dir.join("a.py");
    std::fs::write(&py, "print(1)\n").unwrap();
    ui.app.settings.lsp_servers.insert("python".into(), tinta_core::settings::LspServer { command: String::new(), args: Vec::new() });
    let t = ui.app.open_path(&py);
    ui.run(t);
    assert!(ui.app.lsp_hint().is_none());
}

#[test]
fn palette_filters_and_runs_a_clicked_command() {
    let (mut ui, _) = Ui::with_file();
    ui.cmd(Command::CommandPalette);
    assert!(ui.app.palette.is_some());
    assert_eq!(ui.app.focus, Focus::Input);
    let title = Command::ToggleWordWrap.title(ui.app.pt()).to_string();
    let _ = ui.app.update(Message::Palette(crate::app::PaletteMsg::Input(title)));
    ui.frame();
    let items: Vec<String> = ui.app.palette.as_ref().unwrap().items.iter().map(|i| i.label.clone()).collect();
    assert!(!items.is_empty());
    let before = ui.app.settings.word_wrap;
    let target = ui.app.palette.as_ref().unwrap().items[0].clone();
    assert!(matches!(target.action, crate::app::PAction::Cmd(Command::ToggleWordWrap)), "{}", target.label);
    ui.msg(Message::Palette(crate::app::PaletteMsg::Click(0)));
    assert!(ui.app.palette.is_none(), "a click on a row closes it");
    assert_ne!(ui.app.settings.word_wrap, before);
    ui.cmd(Command::ToggleWordWrap);
}

#[test]
fn every_command_runs_without_panicking_in_every_state() {
    let dir = temp_dir("all");
    std::fs::write(dir.join("a.rs"), SOURCE).unwrap();
    std::fs::write(dir.join("b.md"), "# t\n\ntexto\n").unwrap();
    let mut ui = Ui::new(vec![dir.clone()]);
    let skip = |c: Command| matches!(c, Command::Quit | Command::NewTerminal | Command::ToggleTerminal | Command::TogglePanel | Command::OpenFolderInFiles | Command::CloseFolder);
    // With nothing open.
    for c in Command::ALL.iter().copied().filter(|c| !skip(*c)) {
        eprintln!("-> {} on nothing", c.id());
        ui.cmd(c);
        ui.app.palette = None;
        ui.app.dialog = None;
        ui.app.focus = Focus::Editor;
    }
    // With a file open, then an empty file, each command from a fresh state.
    for name in ["a.rs", "b.md"] {
        for c in Command::ALL.iter().copied().filter(|c| !skip(*c)) {
            let _ = ui.app.close_tabs_where(|_, _| false);
            for t in ui.app.tabs.values_mut() {
                t.pinned = false;
            }
            ui.app.dialog = None;
            let ids: Vec<u64> = ui.app.tabs.keys().copied().collect();
            for id in ids {
                let _ = ui.app.close_tab(id);
            }
            let _ = ui.app.open_path(&dir.join(name));
            ui.frame();
            eprintln!("-> {} on {name}", c.id());
            ui.cmd(c);
            ui.app.palette = None;
            ui.app.dialog = None;
            ui.app.focus = Focus::Editor;
            ui.frame();
        }
    }
    assert!(Path::new(&dir).exists());
}

#[test]
fn split_views_of_one_document_stay_in_step() {
    let (mut ui, _) = Ui::with_file();
    let p = ui.cell(2, 0);
    ui.click_at(p);
    ui.cmd(Command::SplitRight);
    assert_eq!(ui.app.groups.len(), 2);
    assert_eq!(ui.app.docs.len(), 1);
    // Edit at the top in the new view: the first view's caret moves along.
    let first = ui.app.groups[0].active_tab().unwrap();
    let before = ui.app.tabs[&first].cursors.head();
    let _ = ui.app.update(Message::Key(crate::app::KeyEv { name: "home".into(), shifted: None, mods: Modifiers::CTRL, text: None, captured: false }));
    ui.typed("xyz");
    assert_eq!(ui.app.tabs[&first].cursors.head(), before + 3);
    ui.cmd(Command::CloseTab);
    assert_eq!(ui.app.groups.len(), 1);
    assert_eq!(ui.app.docs.len(), 1);
}

#[test]
fn closing_a_dirty_tab_asks_first() {
    let (mut ui, _) = Ui::with_file();
    ui.typed("x");
    ui.cmd(Command::CloseTab);
    assert!(matches!(ui.app.dialog, Some(crate::app::DialogState::CloseUnsaved(_))));
    assert_eq!(ui.app.tabs.len(), 1);
    let _ = ui.app.update(Message::Dialog(crate::app::DialogMsg::Secondary));
    ui.frame();
    assert_eq!(ui.app.tabs.len(), 0);
}

#[test]
fn saving_writes_the_file_and_clears_dirty() {
    let (mut ui, p) = Ui::with_file();
    ui.typed("//");
    assert!(ui.app.active().unwrap().1.doc.is_dirty());
    ui.cmd(Command::Save);
    assert!(!ui.app.active().unwrap().1.doc.is_dirty());
    assert!(std::fs::read_to_string(&p).unwrap().starts_with("//fn main"));
}

#[test]
fn find_highlights_and_replace_all_works() {
    let (mut ui, _) = Ui::with_file();
    ui.cmd(Command::Find);
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::Query("alpha".into())));
    ui.frame();
    assert_eq!(ui.app.active().unwrap().1.matches.len(), 2);
    assert_eq!(ui.selection_text(), "alpha");
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::Replacement("omega".into())));
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::ReplaceAll));
    ui.frame();
    assert_eq!(ui.text().matches("omega").count(), 2);
    assert_eq!(ui.app.active().unwrap().1.matches.len(), 0);
    ui.cmd(Command::Undo);
    assert_eq!(ui.text(), SOURCE);
}

/// More matches than get highlighted: all of them still change.
#[test]
fn replace_all_goes_past_the_highlight_limit() {
    let dir = temp_dir("limit");
    let p = dir.join("many.txt");
    let n = crate::app::MAX_SHOWN_MATCHES + 500;
    std::fs::write(&p, "x\n".repeat(n)).unwrap();
    let mut ui = Ui::new(vec![p]);
    ui.cmd(Command::Find);
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::Query("x".into())));
    ui.frame();
    assert_eq!(ui.app.active().unwrap().1.matches.len(), crate::app::MAX_SHOWN_MATCHES);
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::Replacement("y".into())));
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::ReplaceAll));
    ui.frame();
    assert_eq!(ui.text().matches('y').count(), n);
    assert_eq!(ui.text().matches('x').count(), 0);
}

/// Replacing in the folder used to act on the list on screen, which stops at a limit.
#[test]
fn replace_in_files_reaches_files_past_the_list_limit() {
    let dir = temp_dir("limit2");
    // One file fills the list; the next, alphabetically or not, would be left out of it.
    std::fs::write(dir.join("a.txt"), "x\n".repeat(60_000)).unwrap();
    std::fs::write(dir.join("b.txt"), "x\n").unwrap();
    std::fs::write(dir.join("c.txt"), "x\n").unwrap();
    let mut ui = Ui::new(vec![dir.clone()]);
    let _ = ui.app.update(Message::Search(crate::app::SearchMsg::Query("x".into())));
    wait_for(&mut ui, |a| !a.search.running);
    assert!(ui.app.search.truncated, "the list should say it stopped early");
    let _ = ui.app.update(Message::Search(crate::app::SearchMsg::Replacement("y".into())));
    let _ = ui.app.update(Message::Search(crate::app::SearchMsg::ReplaceAll));
    wait_for(&mut ui, |a| !a.search.replacing);
    for f in ["a.txt", "b.txt", "c.txt"] {
        assert!(!std::fs::read_to_string(dir.join(f)).unwrap().contains('x'), "{f}");
    }
}

#[test]
fn a_separate_instance_leaves_session_and_backups_alone() {
    let (mut ui, _) = Ui::with_file();
    assert!(ui.app.new_session);
    tinta_core::session::write_backup("of-the-main-instance", "texto").unwrap();
    ui.typed("x");
    ui.app.persist_on_quit();
    assert_eq!(tinta_core::session::read_backup("of-the-main-instance").as_deref(), Some("texto"));
    tinta_core::session::remove_backup("of-the-main-instance");
    // And since nothing is kept for it, quitting with unsaved work asks.
    let _ = ui.app.request_quit();
    assert!(matches!(ui.app.dialog, Some(crate::app::DialogState::QuitUnsaved)));
}

#[test]
fn opening_a_file_does_not_lose_the_session_it_would_have_restored() {
    let (mut ui, p) = Ui::with_file();
    ui.typed("rascunho ");
    ui.app.write_backups(false);
    crate::app::save_session(&ui.app);
    let text = ui.text();
    let guard = ui.close();
    let other = temp_dir("other").join("b.txt");
    std::fs::write(&other, "b").unwrap();
    let flags = Flags { targets: vec![(other, None)], new_session: false, dark: true, system_mono: None, font_names: Vec::new() };
    let (app, _) = App::new(flags);
    drop(guard);
    assert_eq!(app.active().unwrap().1.doc.file_name(), "b.txt", "the requested file is in front");
    let id = app.doc_by_path(&p).expect("the old tab came back");
    assert_eq!(app.docs[&id].doc.rope.to_string(), text);
    assert!(app.docs[&id].doc.is_dirty());
}

#[test]
fn word_wrap_keeps_clicks_on_the_right_cell() {
    let dir = temp_dir("wrap");
    let p = dir.join("w.txt");
    std::fs::write(&p, "palavra ".repeat(200)).unwrap();
    let mut ui = Ui::new(vec![p]);
    ui.cmd(Command::ToggleWordWrap);
    ui.frame();
    let (t, e) = ui.app.active().unwrap();
    assert!(t.map.row_count(&e.doc) > 3, "{}", t.map.row_count(&e.doc));
    // Row 2 starts at a word: its first cell is the start of that word.
    let row = t.map.row(&e.doc, 2);
    let start = row.start as usize;
    let p = ui.cell(2, 0);
    ui.click_at(p);
    assert_eq!(ui.head(), (0, start));
    ui.cmd(Command::ToggleWordWrap);
}

#[test]
fn hot_exit_brings_unsaved_text_back() {
    let (mut ui, p) = Ui::with_file();
    ui.typed("rascunho ");
    ui.app.write_backups(false);
    ui.app.settings.restore_session = true;
    crate::app::save_session(&ui.app);
    let doc_text = ui.text();
    // A new run, restoring: same text, still dirty, nothing written to disk.
    let guard = ui.close();
    let flags = Flags { targets: Vec::new(), new_session: false, dark: true, system_mono: None, font_names: Vec::new() };
    let (app, _) = App::new(flags);
    drop(guard);
    let (_, e) = app.active().expect("the tab came back");
    assert_eq!(e.doc.rope.to_string(), doc_text);
    assert!(e.doc.is_dirty());
    assert_eq!(std::fs::read_to_string(&p).unwrap(), SOURCE);
}

/// Not an assertion so much as a measurement: `cargo test --release large_file -- --ignored --nocapture`.
#[test]
#[ignore]
fn large_file_stays_responsive() {
    let dir = temp_dir("large");
    let p = dir.join("big.rs");
    let line = "    let value = compute(alpha, beta) + other_thing(\"texto\", 42); // comentário\n";
    let mut src = String::from("fn main() {\n");
    for _ in 0..300_000 {
        src.push_str(line);
    }
    src.push_str("}\n");
    std::fs::write(&p, &src).unwrap();
    let t = Instant::now();
    let mut ui = Ui::new(vec![p]);
    eprintln!("open + first frames: {:?} ({} MB)", t.elapsed(), src.len() / 1_000_000);
    let measure = |ui: &mut Ui, label: &str, n: usize, f: &mut dyn FnMut(&mut Ui)| {
        let mut worst = std::time::Duration::ZERO;
        let t = Instant::now();
        for _ in 0..n {
            let one = Instant::now();
            f(ui);
            worst = worst.max(one.elapsed());
        }
        eprintln!("{label}: mean {:?}, worst {:?}", t.elapsed() / n as u32, worst);
    };
    measure(&mut ui, "idle frame", 30, &mut |ui| ui.frame());
    measure(&mut ui, "highlight tick + frame", 30, &mut |ui| {
        let _ = ui.app.update(Message::TickFrame);
        ui.frame();
    });
    let _ = ui.app.update(Message::Cmd(Command::GoToLine));
    let _ = ui.app.update(Message::Palette(crate::app::PaletteMsg::Close));
    ui.app.jump_to(150_000, 10);
    ui.frame();
    measure(&mut ui, "typing mid-file", 30, &mut |ui| ui.typed("x"));
    measure(&mut ui, "newline mid-file", 10, &mut |ui| ui.key("enter", Modifiers::empty(), None));
    measure(&mut ui, "arrow down", 30, &mut |ui| ui.key("down", Modifiers::empty(), None));
    measure(&mut ui, "undo", 10, &mut |ui| ui.cmd(Command::Undo));
    let _ = ui.app.update(Message::Cmd(Command::Find));
    let t = Instant::now();
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::Query("alpha".into())));
    ui.frame();
    eprintln!("find 300k matches: {:?} ({} found)", t.elapsed(), ui.app.active().unwrap().1.matches.len());
    measure(&mut ui, "frame with matches", 10, &mut |ui| ui.frame());
    measure(&mut ui, "typing with find open", 5, &mut |ui| {
        ui.app.focus = Focus::Editor;
        ui.typed("y");
    });
    let _ = ui.app.update(Message::Find(crate::app::FindMsg::Close));
    let t = Instant::now();
    ui.cmd(Command::SelectAll);
    eprintln!("select all: {:?}", t.elapsed());
    ui.key("escape", Modifiers::empty(), None);
    let t = Instant::now();
    ui.cmd(Command::ToggleWordWrap);
    eprintln!("turn on word wrap: {:?}", t.elapsed());
    measure(&mut ui, "typing with wrap", 5, &mut |ui| ui.typed("z"));
}

#[test]
fn folds_outline_and_breadcrumbs_come_from_the_syntax_tree() {
    let dir = temp_dir("tree-sitter");
    let p = dir.join("a.rs");
    // Indentation here is deliberately no help.
    std::fs::write(&p, "fn um() {\nlet a = 1;\nlet b = 2;\n}\nstruct S {\nx: u8,\ny: u8,\n}\n").unwrap();
    let mut ui = Ui::new(vec![p]);
    ui.app.refresh_symbols();
    ui.cmd(Command::FoldAll);
    let (t, e) = ui.app.active().unwrap();
    assert_eq!(t.folds.ranges, vec![(0, 2), (4, 6)]);
    let names: Vec<_> = e.symbols.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["um", "S"]);
    ui.cmd(Command::UnfoldAll);
    ui.app.jump_to(5, 0);
    let (t, e) = ui.app.active().unwrap();
    assert_eq!(crate::gui::current_symbols(e, t), vec!["S"]);
    ui.app.jump_to(3, 0);
    let (t, e) = ui.app.active().unwrap();
    assert_eq!(crate::gui::current_symbols(e, t), vec!["um"]);
    // Typing keeps it current.
    ui.app.jump_to(8, 0);
    ui.typed("fn dois() {}");
    ui.app.refresh_symbols();
    std::thread::sleep(std::time::Duration::from_millis(450));
    ui.app.refresh_symbols();
    let names: Vec<_> = ui.app.active().unwrap().1.symbols.iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, vec!["um", "S", "dois"]);
}

#[test]
fn vim_mode_edits_through_the_real_key_path() {
    use tinta_core::vim::Mode;
    let (mut ui, p) = Ui::with_file();
    assert!(!ui.app.settings.vim_mode);
    ui.cmd(Command::ToggleVim);
    // Normal mode: letters are commands, and don't get typed.
    ui.typed("jwQ");
    assert_eq!(ui.text(), SOURCE);
    assert_eq!(ui.head(), (1, 4));
    ui.typed("dw");
    assert!(ui.text().contains("\n    alpha = 1;"), "{}", ui.text());
    ui.typed("u");
    assert_eq!(ui.text(), SOURCE);
    // Insert goes through the editor's own typing.
    ui.typed("ifoo(");
    assert_eq!(ui.app.vim.mode, Mode::Insert);
    assert!(ui.text().contains("    foo(let"), "{}", ui.text());
    ui.key("escape", Modifiers::empty(), None);
    assert_eq!(ui.app.vim.mode, Mode::Normal);
    ui.typed("u");
    assert_eq!(ui.text(), SOURCE, "all that typing undoes as one");
    // Visual selection shows on screen as the document's selection.
    ui.typed("viw");
    assert_eq!(ui.selection_text(), "let");
    ui.key("escape", Modifiers::empty(), None);
    assert_eq!(ui.selection_text(), "");
    // Shortcuts it doesn't claim still work.
    ui.key("p", Modifiers::CTRL, None);
    assert!(ui.app.palette.is_some());
    ui.key("escape", Modifiers::empty(), None);
    assert!(ui.app.palette.is_none());
    // The command line writes the file.
    ui.typed("ix");
    ui.key("escape", Modifiers::empty(), None);
    ui.typed(":");
    assert!(ui.app.vim_line.is_some());
    assert_eq!(ui.app.focus, Focus::Input);
    let _ = ui.app.update(Message::Vim(crate::app::VimMsg::Input("w".into())));
    let _ = ui.app.update(Message::Vim(crate::app::VimMsg::Submit));
    ui.frame();
    assert_eq!(ui.app.focus, Focus::Editor);
    assert!(std::fs::read_to_string(&p).unwrap().contains("lext alpha"), "the caret was on the t of `let`");
    // Searching lights up its matches, and :noh puts them out.
    ui.typed("/");
    let _ = ui.app.update(Message::Vim(crate::app::VimMsg::Input("alpha".into())));
    let _ = ui.app.update(Message::Vim(crate::app::VimMsg::Submit));
    ui.frame();
    assert_eq!(ui.app.active().unwrap().1.matches.len(), 2);
    ui.typed(":");
    let _ = ui.app.update(Message::Vim(crate::app::VimMsg::Input("noh".into())));
    let _ = ui.app.update(Message::Vim(crate::app::VimMsg::Submit));
    ui.frame();
    assert_eq!(ui.app.active().unwrap().1.matches.len(), 0);
    // A mouse selection is a visual selection.
    let a = ui.cell(0, 0);
    let b = ui.cell(0, 2);
    ui.move_to(a);
    ui.press(mouse::Button::Left);
    ui.move_to(b);
    ui.release(mouse::Button::Left);
    assert_eq!(ui.app.vim.mode, Mode::Visual);
    ui.typed("d");
    assert!(ui.text().starts_with(" main()"), "{}", ui.text());
    ui.cmd(Command::ToggleVim);
}

#[test]
fn vim_on_a_read_only_buffer_moves_but_does_not_edit() {
    let (mut ui, _) = Ui::with_file();
    ui.cmd(Command::ToggleVim);
    ui.app.active_mut().unwrap().1.doc.readonly = true;
    ui.typed("jddxix");
    assert_eq!(ui.text(), SOURCE);
    assert_eq!(ui.head().0, 1, "j moved");
    assert_eq!(ui.app.vim.mode, tinta_core::vim::Mode::Normal);
    ui.cmd(Command::ToggleVim);
}

#[test]
fn side_by_side_diff_opens_scrolls_and_jumps() {
    let dir = temp_dir("diff");
    let p = dir.join("a.rs");
    let old: String = (0..200).map(|i| format!("linha {i}\n")).collect();
    std::fs::write(&p, &old).unwrap();
    let mut ui = Ui::new(vec![p.clone()]);
    ui.app.jump_to(150, 0);
    ui.typed("mudou ");
    ui.app.jump_to(5, 0);
    ui.typed("cedo ");
    ui.cmd(Command::CompareSaved);
    let tab = ui.app.active_tab_id().unwrap();
    assert_eq!(ui.app.tabs[&tab].kind, crate::state::TabKind::Diff);
    let hunks = ui.app.tabs[&tab].diff.as_ref().unwrap().hunks.clone();
    assert_eq!(hunks, vec![5, 150]);
    ui.frame();
    // The second change, then wrapping around to the first.
    ui.cmd(Command::NextChange);
    let y = ui.app.tabs[&tab].diff.as_ref().unwrap().scroll_y;
    assert!(y > 100.0 * ui.app.metrics.line_h, "{y}");
    ui.cmd(Command::NextChange);
    assert_eq!(ui.app.tabs[&tab].diff.as_ref().unwrap().current, Some(0));
    // Keys scroll, and don't try to edit anything.
    let before = ui.app.tabs[&tab].diff.as_ref().unwrap().scroll_y;
    ui.key("down", Modifiers::empty(), None);
    assert_eq!(ui.app.tabs[&tab].diff.as_ref().unwrap().scroll_y, before + ui.app.metrics.line_h);
    ui.typed("x");
    ui.key("end", Modifiers::empty(), None);
    ui.frame();
    // Wheel, through the real widget.
    ui.key("home", Modifiers::empty(), None);
    ui.move_to(Point::new(SIZE.width - 200.0, 300.0));
    ui.send(vec![Event::Mouse(mouse::Event::WheelScrolled { delta: mouse::ScrollDelta::Lines { x: 0.0, y: -2.0 } })]);
    assert!(ui.app.tabs[&tab].diff.as_ref().unwrap().scroll_y > 0.0);
    // Double click on the new side goes to the file, at that line.
    let row_y = 300.0;
    let p2 = Point::new(SIZE.width - 200.0, row_y);
    ui.click_at(p2);
    ui.click_at(p2);
    let (t, e) = ui.app.active().unwrap();
    assert_eq!(t.kind, crate::state::TabKind::Text);
    assert_eq!(e.doc.file_name(), "a.rs");
    assert!(e.doc.char_to_line(t.cursors.head()) > 0);
    // Comparing again replaces the view instead of piling up tabs.
    let n = ui.app.tabs.len();
    ui.cmd(Command::CompareSaved);
    ui.frame();
    assert_eq!(ui.app.tabs.len(), n);
}

#[test]
fn source_control_opens_diffs_against_the_right_versions() {
    use tinta_core::git as g;
    let dir = temp_dir("gitdiff");
    let run = |args: &[&str]| {
        std::process::Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap();
    };
    run(&["init", "-q", "-b", "main"]);
    run(&["config", "user.email", "t@example.com"]);
    run(&["config", "user.name", "T"]);
    std::fs::write(dir.join("a.txt"), "um\ndois\n").unwrap();
    g::stage(&dir, "a.txt").unwrap();
    g::commit(&dir, "x").unwrap();
    std::fs::write(dir.join("a.txt"), "um\nDOIS\n").unwrap();
    g::stage(&dir, "a.txt").unwrap();
    std::fs::write(dir.join("a.txt"), "um\nDOIS\ntres\n").unwrap();
    let mut ui = Ui::new(vec![dir.clone()]);
    let _ = ui.app.open_git_diff(dir.clone(), "a.txt".into(), true);
    ui.frame();
    let d = ui.app.active().unwrap().0.diff.as_ref().unwrap();
    assert_eq!(d.left.rope.to_string(), "um\ndois\n");
    assert_eq!(d.right.rope.to_string(), "um\nDOIS\n");
    let _ = ui.app.open_git_diff(dir.clone(), "a.txt".into(), false);
    ui.frame();
    let d = ui.app.active().unwrap().0.diff.as_ref().unwrap();
    assert_eq!(d.left.rope.to_string(), "um\nDOIS\n");
    assert_eq!(d.right.rope.to_string(), "um\nDOIS\ntres\n");
    assert_eq!((d.added, d.removed), (1, 0));
}

/// Compiling and starting a debugger can take a while on a slow machine.
fn wait_long(ui: &mut Ui, mut done: impl FnMut(&App) -> bool) {
    for _ in 0..3000 {
        ui.frame();
        if done(&ui.app) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("timed out waiting");
}

fn have_gdb_dap() -> bool {
    tinta_core::dap::resolve_adapter("gdb").is_ok() && std::process::Command::new("cc").arg("--version").output().is_ok()
}

const DEBUG_PROGRAM: &str = "#include <stdio.h>\n\nstruct ponto { int x; int y; };\n\nint dobro(int n) {\n    int r = n * 2;\n    return r;\n}\n\nint main(void) {\n    struct ponto p = { 3, 4 };\n    int total = dobro(p.x);\n    total += p.y;\n    printf(\"total %d\\n\", total);\n    return 7;\n}\n";

/// A click in the margin to the left of the line numbers, on the text's row.
fn margin(ui: &Ui, line: usize) -> Point {
    let c = ui.cell(line, 0);
    let (t, e) = ui.app.active().unwrap();
    let g = Geometry::new(e.doc.len_lines(), t.view_w, &ui.app.metrics, &ui.app.settings);
    Point::new(c.x - g.text_x + 12.0, c.y)
}

#[test]
fn debugging_a_program_through_the_interface() {
    if !have_gdb_dap() {
        eprintln!("skipped: no gdb with DAP");
        return;
    }
    let dir = temp_dir("debug");
    let src = dir.join("prog.c");
    std::fs::write(&src, DEBUG_PROGRAM).unwrap();
    let mut ui = Ui::new(vec![src.clone()]);
    ui.frame();

    // A click in the margin sets a breakpoint, and doesn't touch the selection.
    let before = ui.head();
    let at = margin(&ui, 5);
    ui.click_at(at);
    assert_eq!(ui.head(), before);
    assert_eq!(ui.app.debug.breakpoints[&src][0].line, 5);
    // F9 on the printf line.
    ui.app.jump_to(13, 0);
    ui.key("f9", Modifiers::empty(), None);
    assert_eq!(ui.app.debug.breakpoints[&src].iter().map(|b| b.line).collect::<Vec<_>>(), vec![5, 13]);
    // Clicking again takes it off.
    let at = margin(&ui, 13);
    ui.click_at(at);
    assert_eq!(ui.app.debug.breakpoints[&src].len(), 1);

    // Lines added above move it along with its code.
    ui.app.jump_to(0, 0);
    ui.key("enter", Modifiers::empty(), None);
    assert_eq!(ui.app.debug.breakpoints[&src][0].line, 6);
    ui.key("backspace", Modifiers::empty(), None);
    assert_eq!(ui.app.debug.breakpoints[&src][0].line, 5);

    // F5 compiles it through the task panel, and stops at the breakpoint.
    ui.key("f5", Modifiers::empty(), None);
    assert_eq!(ui.app.sidebar, Some(crate::app::SidebarView::Debug));
    wait_long(&mut ui, |a| a.debug.is_stopped() && a.debug.session.as_ref().is_some_and(|s| !s.variable_rows().is_empty()));
    // The program has a terminal of its own in the panel; what it prints comes out there.
    assert_eq!(ui.app.panel, Some(crate::app::PanelView::Terminal));
    let (term, tty) = ui.app.debug.tty.as_ref().expect("the program's terminal");
    assert!(ui.app.terms.iter().any(|t| t.id == *term && t.title.ends_with("prog.c")), "{:?}", ui.app.terms.iter().map(|t| &t.title).collect::<Vec<_>>());
    let shown = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        use std::io::Read;
        let mut out = std::fs::File::from(tty.master.try_clone().unwrap());
        let shown = shown.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 512];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                shown.lock().unwrap().push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
    }
    let loc = |ui: &Ui| ui.app.debug.session.as_ref().unwrap().exec_location().map(|(_, l, top)| (l, top));
    assert_eq!(loc(&ui), Some((5, true)));
    assert_eq!(ui.head().0, 5, "the caret goes to where it stopped");
    assert_eq!(ui.app.debug.breakpoints[&src][0].state, tinta_core::dap::BpState::Verified);

    // F10 steps, and the panels draw whatever comes.
    ui.key("f10", Modifiers::empty(), None);
    let value = |a: &App, path: &str| a.debug.session.as_ref()?.variable_rows().into_iter().find(|r| r.path == path).map(|r| r.value);
    wait_long(&mut ui, |a| a.debug.is_stopped() && a.debug.session.as_ref().unwrap().exec_location().is_some_and(|l| l.1 == 6) && value(a, "Locals/r").is_some());
    assert_eq!(value(&ui.app, "Locals/r").as_deref(), Some("6"));

    // A new value for r, through the dialog: dobro now returns 20.
    let row = ui.app.debug.session.as_ref().unwrap().variable_rows().into_iter().find(|r| r.path == "Locals/r").unwrap();
    let _ = ui.app.update(Message::Debug(crate::app::DebugMsg::EditValue(row)));
    assert!(matches!(ui.app.dialog, Some(crate::app::DialogState::SetValue(_))));
    assert_eq!(ui.app.dialog_input, "6");
    let _ = ui.app.update(Message::Dialog(crate::app::DialogMsg::Input("20".into())));
    let _ = ui.app.update(Message::Dialog(crate::app::DialogMsg::Primary));
    wait_long(&mut ui, |a| value(a, "Locals/r").as_deref() == Some("20"));

    // The call stack is clickable: the frame's variables and line come up.
    let _ = ui.app.update(Message::Debug(crate::app::DebugMsg::SelectFrame(1)));
    wait_long(&mut ui, |a| value(a, "Locals/total").is_some());
    assert_eq!(loc(&ui), Some((11, false)));
    assert_eq!(ui.head().0, 11);

    // Shift+F11 leaves dobro, F11 is a step in rather than full screen.
    ui.key("f11", Modifiers::empty(), None);
    assert!(!ui.app.fullscreen, "F11 steps while debugging");
    wait_long(&mut ui, |a| a.debug.is_stopped() && a.debug.session.as_ref().is_some_and(|s| !s.variable_rows().is_empty()));

    // Hovering shows the value, from the debugger and not the language server. The pointer
    // leaves the text first, or its own hover timer would ask about where it rests.
    ui.move_to(Point::new(-50.0, -50.0));
    let tab = ui.app.active_tab_id().unwrap();
    ui.app.hover = None;
    let _ = ui.app.update(Message::Editor(tab, crate::ui::editor_view::EditorEvent::Hover { row: 6, vcol: 11 }));
    wait_long(&mut ui, |a| a.hover.as_ref().is_some_and(|h| h.text == "r = 20"));
    // Even over the right half of the letter, where the position rounds to the char after it.
    ui.app.hover = None;
    let _ = ui.app.update(Message::Editor(tab, crate::ui::editor_view::EditorEvent::Hover { row: 6, vcol: 12 }));
    wait_long(&mut ui, |a| a.hover.as_ref().is_some_and(|h| h.text == "r = 20"));

    // The console evaluates.
    let _ = ui.app.update(Message::Debug(crate::app::DebugMsg::Input("2 + 3".into())));
    let _ = ui.app.update(Message::Debug(crate::app::DebugMsg::Submit));
    wait_long(&mut ui, |a| a.debug.console.iter().any(|l| l.text == "5"));

    // Ctrl+F10 runs on to the line with the caret, the printf, without a breakpoint there.
    ui.app.jump_to(13, 0);
    ui.key("f10", Modifiers::CTRL, None);
    wait_long(&mut ui, |a| a.debug.is_stopped() && a.debug.session.as_ref().unwrap().exec_location().is_some_and(|l| l.1 == 13) && value(a, "Locals/total").is_some());
    assert_eq!(value(&ui.app, "Locals/total").as_deref(), Some("24"), "20 from dobro, plus 4");
    assert_eq!(ui.app.debug.breakpoints[&src].iter().map(|b| b.line).collect::<Vec<_>>(), vec![5]);

    // Off to the end: the output in the program's terminal, the exit code in the console.
    ui.key("f5", Modifiers::empty(), None);
    wait_long(&mut ui, |a| a.debug.session.is_none());
    let end = Instant::now() + std::time::Duration::from_secs(5);
    while !shown.lock().unwrap().contains("total 24") && Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(shown.lock().unwrap().contains("total 24"), "{:?}", shown.lock().unwrap());
    let text: Vec<_> = ui.app.debug.console.iter().map(|l| l.text.as_str()).collect();
    assert!(text.iter().any(|l| l.ends_with(" 7")), "{text:?}");
    assert!(ui.app.debug.breakpoints[&src][0].line == 5, "the breakpoint stays for next time");

    // Without a session, F11 is full screen again.
    assert!(!ui.app.debug.is_live());
    ui.frame();
}

#[test]
fn run_to_cursor_starts_a_session_and_goes_there() {
    if !have_gdb_dap() {
        eprintln!("skipped: no gdb with DAP");
        return;
    }
    let dir = temp_dir("runto");
    let src = dir.join("prog.c");
    std::fs::write(&src, DEBUG_PROGRAM).unwrap();
    let mut ui = Ui::new(vec![src.clone()]);
    ui.frame();
    // No breakpoints and nothing running: Ctrl+F10 on the printf starts and stops there.
    ui.app.jump_to(13, 0);
    ui.key("f10", Modifiers::CTRL, None);
    let at = |a: &App| a.debug.session.as_ref().and_then(|s| s.exec_location()).map(|(_, l, _)| l);
    wait_long(&mut ui, |a| a.debug.is_stopped() && at(a) == Some(13));
    assert!(ui.app.debug.breakpoints.get(&src).is_none_or(|l| l.is_empty()), "no breakpoint left behind");
    ui.cmd(Command::DebugStop);
    wait_long(&mut ui, |a| !a.debug.is_live());
}

#[test]
fn a_failed_build_shows_its_errors_and_does_not_start_anything() {
    if !have_gdb_dap() {
        return;
    }
    let dir = temp_dir("debugbad");
    let src = dir.join("ruim.c");
    std::fs::write(&src, "int main(void) {\n    return nada;\n}\n").unwrap();
    let mut ui = Ui::new(vec![src.clone()]);
    ui.key("f5", Modifiers::empty(), None);
    wait_long(&mut ui, |a| a.tasks.running.is_none() && a.tasks.exit.is_some());
    assert_eq!(ui.app.panel, Some(crate::app::PanelView::Tasks));
    assert!(ui.app.debug.session.is_none());
    assert!(ui.app.tasks.problems.iter().any(|(p, _)| p.ends_with("ruim.c")), "{:?}", ui.app.tasks.problems);
}

#[test]
fn stopping_a_session_lets_the_next_one_run_and_breakpoints_are_remembered() {
    if !have_gdb_dap() {
        return;
    }
    let dir = temp_dir("debugstop");
    let src = dir.join("prog.c");
    std::fs::write(&src, DEBUG_PROGRAM).unwrap();
    let mut ui = Ui::new(vec![src.clone()]);
    ui.app.jump_to(5, 0);
    ui.key("f9", Modifiers::empty(), None);
    ui.key("f5", Modifiers::empty(), None);
    wait_long(&mut ui, |a| a.debug.is_stopped());
    ui.key("f5", Modifiers::SHIFT, None);
    assert!(ui.app.debug.session.is_none());
    // A breakpoint set while nothing runs goes in with the next session.
    ui.app.jump_to(13, 0);
    ui.key("f9", Modifiers::empty(), None);
    let saved = ui.app.build_session();
    assert_eq!(saved.breakpoints[&src].len(), 2);
    let back: tinta_core::session::Session = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    assert_eq!(back.breakpoints[&src].iter().map(|b| b.line).collect::<Vec<_>>(), vec![5, 13]);
    // An old session file, from before there were breakpoints, still loads.
    let mut v = serde_json::to_value(&saved).unwrap();
    v.as_object_mut().unwrap().remove("breakpoints");
    v.as_object_mut().unwrap().remove("watches");
    assert!(serde_json::from_value::<tinta_core::session::Session>(v).is_ok());
    // Restart goes through a fresh session.
    ui.key("f5", Modifiers::empty(), None);
    wait_long(&mut ui, |a| a.debug.is_stopped());
    let first = ui.app.debug.generation;
    ui.key("f5", Modifiers::CTRL | Modifiers::SHIFT, None);
    wait_long(&mut ui, |a| a.debug.generation > first && a.debug.is_stopped());
    ui.key("f5", Modifiers::SHIFT, None);
}

#[test]
fn a_launch_json_pointing_at_nothing_says_so() {
    if !have_gdb_dap() {
        return;
    }
    let dir = temp_dir("debugmissing");
    std::fs::create_dir_all(dir.join(".tinta")).unwrap();
    std::fs::write(dir.join(".tinta/launch.json"), r#"[{ "name": "x", "type": "gdb", "program": "build/nao_existe" }]"#).unwrap();
    let src = dir.join("a.c");
    std::fs::write(&src, "int main(void) { return 0; }\n").unwrap();
    let mut ui = Ui::new(vec![dir.clone(), src]);
    ui.key("f5", Modifiers::empty(), None);
    assert!(ui.app.debug.session.is_none(), "it doesn't sit there looking like it's running");
    assert_eq!(ui.app.panel, Some(crate::app::PanelView::Debug));
    assert!(ui.app.debug.console.iter().any(|l| l.text.contains("nao_existe")), "{:?}", ui.app.debug.console);
    assert!(ui.app.toast.is_some());
}

#[test]
fn saying_what_is_missing_when_a_language_has_no_debugger() {
    let dir = temp_dir("debugnone");
    let p = dir.join("notas.md");
    std::fs::write(&p, "# oi\n").unwrap();
    let mut ui = Ui::new(vec![p]);
    ui.key("f5", Modifiers::empty(), None);
    assert!(ui.app.debug.session.is_none());
    assert_eq!(ui.app.panel, Some(crate::app::PanelView::Debug));
    assert!(ui.app.debug.console.iter().any(|l| l.text.contains("launch.json")), "{:?}", ui.app.debug.console);
    // A file with no name can't hold a breakpoint, and says why.
    ui.cmd(Command::NewFile);
    ui.key("f9", Modifiers::empty(), None);
    assert!(ui.app.debug.breakpoints.is_empty());
    assert!(ui.app.toast.is_some());
}


#[test]
fn a_build_task_runs_and_its_errors_mark_the_file() {
    let dir = temp_dir("task");
    std::fs::write(dir.join("a.c"), "int main(void) {\n  return nada;\n}\n").unwrap();
    std::fs::write(dir.join("Makefile"), "all:\n\t@echo 'a.c:2:10: error: nada nao existe' >&2; exit 3\n").unwrap();
    let mut ui = Ui::new(vec![dir.clone()]);
    // These marks come from the build; clangd, if installed, would add its own.
    ui.app.settings.lsp_enabled = false;
    let _ = ui.app.open_path(&dir.join("a.c"));
    ui.frame();
    ui.cmd(Command::RunBuild);
    assert_eq!(ui.app.panel, Some(crate::app::PanelView::Tasks));
    wait_for(&mut ui, |a| a.tasks.running.is_none() && a.tasks.exit.is_some());
    assert_eq!(ui.app.tasks.exit, Some(Err(Some(2))), "what make says when a recipe fails");
    assert_eq!(ui.app.tasks.problems.len(), 1);
    let (_, e) = ui.app.active().unwrap();
    assert_eq!(e.diags.len(), 1, "the open file got the mark");
    assert_eq!(e.diags[0].message, "nada nao existe");
    assert_eq!(e.doc.slice_string(e.diags[0].start, e.diags[0].end), "nada");
    assert!(ui.app.problems().iter().any(|p| p.4 == "nada nao existe"));
    // F6 goes there.
    ui.app.jump_to(0, 0);
    ui.cmd(Command::NextTaskProblem);
    assert_eq!(ui.head(), (1, 9));
    // A line of output with a place in it can be clicked.
    let i = ui.app.tasks.lines.iter().position(|l| l.location.is_some()).unwrap();
    ui.app.jump_to(0, 0);
    let _ = ui.app.update(Message::Task(crate::app::TaskMsg::Jump(i)));
    ui.frame();
    assert_eq!(ui.head(), (1, 9));
    // The next run starts clean.
    std::fs::write(dir.join("Makefile"), "all:\n\t@echo ok\n").unwrap();
    ui.cmd(Command::RerunTask);
    wait_for(&mut ui, |a| a.tasks.running.is_none() && a.tasks.exit.is_some());
    assert_eq!(ui.app.tasks.exit, Some(Ok(())));
    assert!(ui.app.active().unwrap().1.diags.is_empty());
    assert!(ui.app.tasks.lines.iter().any(|l| l.text == "ok"));
}

#[test]
fn a_running_task_can_be_stopped() {
    let dir = temp_dir("stop");
    std::fs::write(dir.join("Makefile"), "all:\n\t@echo inicio; sleep 60; echo fim\n").unwrap();
    let mut ui = Ui::new(vec![dir]);
    ui.cmd(Command::RunBuild);
    wait_for(&mut ui, |a| a.tasks.lines.iter().any(|l| l.text == "inicio"));
    let started = Instant::now();
    ui.cmd(Command::StopTask);
    wait_for(&mut ui, |a| a.tasks.exit.is_some());
    assert!(started.elapsed().as_secs() < 5, "the sleep underneath went too");
    assert!(!ui.app.tasks.lines.iter().any(|l| l.text == "fim"));
}

#[test]
fn ctrl_click_in_the_terminal_follows_a_location() {
    let dir = temp_dir("link");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/a.rs"), "um\ndois\ntres\n").unwrap();
    let mut ui = Ui::new(vec![dir]);
    let _ = ui.app.update(Message::TermLink("erro em src/a.rs:3:2 agora".into(), 12));
    ui.frame();
    assert_eq!(ui.app.active().unwrap().1.doc.file_name(), "a.rs");
    assert_eq!(ui.head(), (2, 1));
    // Nothing there: nothing happens, and it doesn't matter where the shell has wandered off to.
    let _ = ui.app.update(Message::TermLink("sem nada aqui".into(), 3));
    ui.frame();
    assert_eq!(ui.app.tabs.len(), 1);
}


#[test]
fn tabs_move_between_splits_and_an_emptied_split_goes_away() {
    let dir = temp_dir("splits");
    let (a, b) = (dir.join("a.txt"), dir.join("b.txt"));
    std::fs::write(&a, "a\n").unwrap();
    std::fs::write(&b, "b\n").unwrap();
    let mut ui = Ui::new(vec![a.clone(), b.clone()]);
    ui.cmd(Command::SplitRight);
    assert_eq!(ui.app.groups.len(), 2);
    let tab_b = ui.app.groups[0].tabs().into_iter().find(|t| ui.app.docs[&ui.app.tabs[t].doc].doc.file_name() == "b.txt").unwrap();
    // b.txt dropped onto the second split, at its start.
    ui.msg(Message::TabToGroup { tab: tab_b, group: 1, pos: 0 });
    assert_eq!(ui.app.groups[1].tabs()[0], tab_b);
    assert_eq!(ui.app.active_group, 1);
    assert_eq!(ui.app.active().unwrap().1.doc.file_name(), "b.txt");
    assert!(!ui.app.groups[0].tabs().contains(&tab_b));
    // Within the same split, it just moves along.
    let last = ui.app.groups[1].tabs().len() - 1;
    ui.msg(Message::TabToGroup { tab: tab_b, group: 1, pos: last });
    assert_eq!(*ui.app.groups[1].tabs().last().unwrap(), tab_b);
    // Widths from dragging the divider, kept for the session.
    ui.msg(Message::GroupWeights(vec![1.5, 0.5]));
    assert_eq!(ui.app.groups.iter().map(|g| g.weight).collect::<Vec<_>>(), vec![1.5, 0.5]);
    assert!(ui.app.build_session().groups.iter().any(|g| (g.weight - 1.5).abs() < 1e-6));
    // Moving everything out of the first split removes it.
    for t in ui.app.groups[0].tabs() {
        ui.msg(Message::TabToGroup { tab: t, group: 1, pos: 0 });
    }
    assert_eq!(ui.app.groups.len(), 1);
    assert_eq!(ui.app.groups[0].tabs().len(), ui.app.tabs.len());
}
