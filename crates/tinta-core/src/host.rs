//! Programs of the user's system: debuggers, language servers, compilers, git, the shell. In a
//! Flatpak they live outside the sandbox, so they run through `flatpak-spawn --host`; anywhere
//! else they run as they are.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// Running inside a Flatpak.
pub fn sandboxed() -> bool {
    static FLATPAK: OnceLock<bool> = OnceLock::new();
    *FLATPAK.get_or_init(|| Path::new("/.flatpak-info").exists())
}

/// `program` with `args`, run in `cwd` with `env` added, on the user's system. The caller adds
/// the pipes. Set the folder and the environment here, not on the result: in a Flatpak the
/// result is `flatpak-spawn`, and they have to reach the program on the other side.
pub fn command<S: AsRef<OsStr>>(program: impl AsRef<OsStr>, args: impl IntoIterator<Item = S>, cwd: Option<&Path>, env: &[(&str, &str)]) -> Command {
    if !sandboxed() {
        let mut c = Command::new(program);
        c.args(args);
        if let Some(d) = cwd {
            c.current_dir(d);
        }
        c.envs(env.iter().copied());
        return c;
    }
    let mut c = Command::new("flatpak-spawn");
    // --watch-bus: the program ends with the editor instead of lingering on the host.
    c.args(["--host", "--watch-bus"]);
    if let Some(d) = cwd {
        c.arg(format!("--directory={}", d.display()));
    }
    for (k, v) in env {
        c.arg(format!("--env={k}={v}"));
    }
    c.arg(program).args(args);
    c
}

/// Like `command`, with `fd` of this process handed to the program as its descriptor 3: in a
/// Flatpak that's how something open in the sandbox, like a terminal, reaches the other side.
pub fn command_with_fd<S: AsRef<OsStr>>(
    program: impl AsRef<OsStr>,
    args: impl IntoIterator<Item = S>,
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    fd: std::os::fd::RawFd,
) -> Command {
    use std::os::unix::process::CommandExt;
    let mut c = if sandboxed() {
        let mut c = Command::new("flatpak-spawn");
        c.args(["--host", "--watch-bus", "--forward-fd=3"]);
        if let Some(d) = cwd {
            c.arg(format!("--directory={}", d.display()));
        }
        for (k, v) in env {
            c.arg(format!("--env={k}={v}"));
        }
        c.arg(program).args(args);
        c
    } else {
        command(program, args, cwd, env)
    };
    // SAFETY: dup2 is async-signal-safe, and the copy is what the program gets as 3.
    unsafe {
        c.pre_exec(move || {
            if libc::dup2(fd, 3) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    c
}

/// The arguments that start `argv` on the user's system, for whoever spawns it in their own
/// way, like the terminal.
pub fn argv(argv: &[String], cwd: Option<&Path>, env: &[(&str, &str)]) -> Vec<String> {
    if !sandboxed() {
        return argv.to_vec();
    }
    let mut out = vec!["flatpak-spawn".to_string(), "--host".to_string(), "--watch-bus".to_string()];
    if let Some(d) = cwd {
        out.push(format!("--directory={}", d.display()));
    }
    out.extend(env.iter().map(|(k, v)| format!("--env={k}={v}")));
    out.extend(argv.iter().cloned());
    out
}

/// A folder for files the user's programs make for the editor, like a program built to debug.
/// In a Flatpak `/tmp` is the sandbox's own, which the programs outside don't see: the app's
/// cache folder, under the home folder, is the same path on both sides.
pub fn temp_dir() -> std::path::PathBuf {
    if sandboxed() && let Some(cache) = dirs::cache_dir() {
        return cache.join("tmp");
    }
    std::env::temp_dir()
}

/// Whether the user's system has `cmd`: a name on its PATH, or a path to a program.
pub fn find(cmd: &str) -> bool {
    if !sandboxed() {
        if cmd.contains('/') {
            return Path::new(cmd).is_file();
        }
        return std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|p| p.join(cmd).is_file()));
    }
    // Asking the host takes a moment, and the answer doesn't change while the editor runs.
    static SEEN: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(found) = seen.lock().ok().and_then(|m| m.get(cmd).copied()) {
        return found;
    }
    let found = command("sh", ["-c", "command -v \"$1\" >/dev/null", "sh", cmd], None, &[])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if let Ok(mut m) = seen.lock() {
        m.insert(cmd.to_string(), found);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_sandbox_programs_run_as_they_are() {
        if sandboxed() {
            return;
        }
        let c = command("git", ["status"], Some(Path::new("/tmp")), &[("A", "1")]);
        assert_eq!(c.get_program(), "git");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["status"]);
        assert_eq!(c.get_current_dir(), Some(Path::new("/tmp")));
        assert_eq!(argv(&["sh".into()], None, &[]), ["sh"]);
        assert!(find("sh"));
        assert!(!find("nao-existe-de-jeito-nenhum"));
    }
}
