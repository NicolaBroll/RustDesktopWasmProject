//! The system's file manager and terminal, opened on a folder (`Platform::show_folder` and
//! `Platform::open_terminal`).

use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Shows `folder` in the file manager: `open` on macOS, `explorer` on Windows, `xdg-open`
/// elsewhere.
pub fn show_folder(folder: &Path) -> io::Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let mut command = Command::new(program);
    command.arg(folder);
    start(command)
}

/// Opens a terminal in `folder`: the first of [`terminals`] that exists.
pub fn open_terminal(folder: &Path) -> io::Result<()> {
    let mut last = io::Error::from(io::ErrorKind::NotFound);
    for argv in terminals(std::env::var("TERMINAL").ok().as_deref(), folder) {
        let Some((program, args)) = argv.split_first() else {
            continue;
        };
        let mut command = Command::new(program);
        command.args(args).current_dir(folder);
        match start(command) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => last = e,
            Err(e) => return Err(e),
        }
    }
    Err(last)
}

/// The terminals to try, in order, started in `folder`. Linux has no standard one: `$TERMINAL`
/// if set, then Debian's `x-terminal-emulator` and the terminals of the common desktops, which
/// all start in their working directory. macOS and Windows get the folder as an argument.
fn terminals(env_terminal: Option<&str>, folder: &Path) -> Vec<Vec<OsString>> {
    let argv = |args: &[&str]| args.iter().map(OsString::from).collect::<Vec<_>>();
    let with_folder = |args: &[&str]| {
        let mut argv = argv(args);
        argv.push(folder.into());
        argv
    };
    if cfg!(target_os = "macos") {
        return vec![with_folder(&["open", "-a", "Terminal"])];
    }
    if cfg!(windows) {
        // Windows Terminal, otherwise a new console with `cmd` in the working directory.
        return vec![
            with_folder(&["wt", "-d"]),
            argv(&["cmd", "/C", "start", "cmd"]),
        ];
    }
    let mut list: Vec<Vec<OsString>> = env_terminal
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| argv(&[t]))
        .into_iter()
        .collect();
    for name in [
        "x-terminal-emulator",
        "gnome-terminal",
        "konsole",
        "xfce4-terminal",
        "mate-terminal",
        "tilix",
        "kitty",
        "alacritty",
        "foot",
        "xterm",
    ] {
        list.push(argv(&[name]));
    }
    list
}

/// Starts `command` detached from the app's standard streams; a thread waits for it, so it does
/// not stay a zombie on Unix.
fn start(mut command: Command) -> io::Result<()> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::Builder::new()
        .name("project-launch".to_owned())
        .spawn(move || {
            if let Err(e) = child.wait() {
                log::warn!("waiting for a launched program: {e}");
            }
        })?;
    Ok(())
}

#[cfg(all(test, not(windows), not(target_os = "macos")))]
mod tests {
    use super::terminals;
    use std::path::Path;

    #[test]
    fn terminal_comes_first_when_set() {
        let folder = Path::new("/repo");
        let list = terminals(Some(" wezterm "), folder);
        assert_eq!(list[0], ["wezterm"]);
        assert_eq!(list[1], ["x-terminal-emulator"]);
        assert_eq!(terminals(Some(""), folder), terminals(None, folder));
        assert_eq!(terminals(None, folder)[0], ["x-terminal-emulator"]);
    }
}
