//! The GTK side: the application, its window, and running what the app's logic asks for.

mod debug;
mod dialogs;
mod editor;
mod findbar;
mod keys;
mod markdown;
mod menu;
mod panel;
mod popups;
mod rows;
mod shell;
mod sidebar;
mod status;
mod window;

use adw::prelude::*;
use gtk::{gio, glib};

#[cfg(test)]
pub use editor::editor_view_for;
#[cfg(test)]
pub use sidebar::current_symbols;
pub use shell::Shell;

pub fn run() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // `--new-session` is an editor of its own: it doesn't join the one already running.
    let mut flags = gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::HANDLES_OPEN;
    if args.iter().any(|a| a == "--new-session") {
        flags |= gio::ApplicationFlags::NON_UNIQUE;
    }
    let app = adw::Application::builder().application_id(crate::app::APP_ID).flags(flags).build();
    let shell: std::rc::Rc<std::cell::RefCell<Option<std::rc::Rc<Shell>>>> = Default::default();
    {
        let shell = shell.clone();
        app.connect_command_line(move |app, cmd| {
            let args: Vec<String> = cmd.arguments().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
            let cwd = cmd.cwd().unwrap_or_default();
            let (targets, new_session) = match crate::parse_args(&args) {
                Ok(t) => t,
                // `--help` and `--version` were answered before GTK started.
                Err(_) => return 0.into(),
            };
            // Relative paths are the caller's, who may be in another folder than this process.
            let targets: Vec<crate::Target> = targets.into_iter().map(|(p, at)| (if p.is_relative() { cwd.join(p) } else { p }, at)).collect();
            let existing = shell.borrow().clone();
            match existing {
                Some(s) => {
                    s.dispatch(crate::app::Message::RemoteOpen(targets));
                    s.present();
                }
                None => {
                    let s = Shell::start(app, targets, new_session);
                    *shell.borrow_mut() = Some(s);
                }
            }
            0.into()
        });
    }
    app.run_with_args(&args)
}
