//! Tinta: a code editor for GNOME, in GTK 4 and libadwaita.

mod app;
mod bus;
mod fx;
mod gfx;
mod gui;
mod i18n;
mod state;
mod ui;

use std::path::PathBuf;

/// A file to open, with the line and column to go to.
pub type Target = (PathBuf, Option<(usize, usize)>);

fn main() -> gtk::glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    // Answered here, before GTK: a running editor would print them in its own terminal.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(text) = parse_args(&args) {
        println!("{text}");
        return gtk::glib::ExitCode::SUCCESS;
    }
    gui::run()
}

/// What the command line asks for. Errors and `--help` print and leave.
pub fn parse_args(args: &[String]) -> Result<(Vec<Target>, bool), String> {
    let mut targets = Vec::new();
    let mut new_session = false;
    for arg in args {
        match arg.as_str() {
            "--version" | "-V" => return Err(format!("tinta {}", env!("CARGO_PKG_VERSION"))),
            "--help" | "-h" => return Err(USAGE.to_string()),
            "--new-session" => new_session = true,
            other => targets.push(parse_target(other)),
        }
    }
    Ok((targets, new_session))
}

const USAGE: &str = "tinta: code editor for GNOME

Usage: tinta-gnome [--new-session] [FILE[:LINE[:COL]] | FOLDER]...

  --new-session   a separate editor: no saved session, no backups, doesn't join the running one
  --version       print the version
";

/// `src/main.rs:10:4` opens at that spot, as compilers print it.
fn parse_target(arg: &str) -> Target {
    let path = PathBuf::from(arg);
    if path.exists() {
        return (path, None);
    }
    let mut parts = arg.rsplitn(3, ':');
    let last = parts.next().and_then(|s| s.parse::<usize>().ok());
    let mid = parts.next();
    let rest = parts.next();
    match (last, mid, rest) {
        (Some(col), Some(line), Some(file)) if line.parse::<usize>().is_ok() => {
            (PathBuf::from(file), Some((line.parse::<usize>().unwrap().saturating_sub(1), col.saturating_sub(1))))
        }
        (Some(line), Some(file), _) => (PathBuf::from(file), Some((line.saturating_sub(1), 0))),
        _ => (path, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_with_positions() {
        assert_eq!(parse_target("nope/a.rs:10:4"), (PathBuf::from("nope/a.rs"), Some((9, 3))));
        assert_eq!(parse_target("nope/a.rs:10"), (PathBuf::from("nope/a.rs"), Some((9, 0))));
        assert_eq!(parse_target("nope/a.rs"), (PathBuf::from("nope/a.rs"), None));
    }
}
