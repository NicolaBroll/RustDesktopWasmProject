//! The system's file manager and terminal, opened on a folder (`Platform::show_folder` and
//! `Platform::open_terminal`), and a file opened in the external editor (`Platform::open_file`).

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
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

/// Opens `file` in `editor` (see [`editor_argv`]) or, if `editor` is blank, in the system's
/// default application: `open` on macOS, `explorer` on Windows, `xdg-open` elsewhere. The path is
/// made absolute, so it never starts with `-`, and its separators are the system's.
pub fn open_file(file: &Path, editor: &str) -> io::Result<()> {
    let file: PathBuf = std::path::absolute(file)?.components().collect();
    if !file.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{}: no such file", file.display()),
        ));
    }
    let argv = if editor.trim().is_empty() {
        let program = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(windows) {
            "explorer"
        } else {
            "xdg-open"
        };
        vec![OsString::from(program), file.into_os_string()]
    } else {
        editor_argv(editor, file.as_os_str())?
    };
    let Some((program, args)) = argv.split_first() else {
        return Err(io::ErrorKind::InvalidInput.into());
    };
    let mut command = Command::new(program);
    command.args(args);
    start(command)
}

/// The arguments of the editor command line `editor` for `file`, split by [`split_command`]:
/// every `{file}` is replaced by the path, and without one the path is the last argument.
fn editor_argv(editor: &str, file: &OsStr) -> io::Result<Vec<OsString>> {
    let words = split_command(editor).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("external editor: unclosed quote in {editor}"),
        )
    })?;
    if words.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "external editor: no program",
        ));
    }
    const FILE: &str = "{file}";
    let mut found = false;
    let mut argv: Vec<OsString> = words
        .iter()
        .enumerate()
        .map(|(index, word)| {
            // The program itself is never the file.
            if index == 0 || !word.contains(FILE) {
                return OsString::from(word);
            }
            found = true;
            let mut arg = OsString::new();
            for (n, piece) in word.split(FILE).enumerate() {
                if n > 0 {
                    arg.push(file);
                }
                arg.push(piece);
            }
            arg
        })
        .collect();
    if !found {
        argv.push(file.to_owned());
    }
    Ok(argv)
}

/// Splits a command line into words, without a shell: blanks separate the words, `"…"` and `'…'`
/// keep blanks and the other quote inside a word, and nothing else is special (a backslash stays,
/// so Windows paths need no escaping). `None`: a quote is not closed.
fn split_command(line: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_word = true;
            }
            None if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            None => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    if in_word {
        words.push(word);
    }
    Some(words)
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

#[cfg(test)]
mod split_tests {
    use super::{editor_argv, split_command};
    use std::ffi::{OsStr, OsString};

    fn words(line: &str) -> Vec<String> {
        split_command(line).unwrap()
    }

    #[test]
    fn blanks_separate_and_quotes_group() {
        assert_eq!(words("code --wait {file}"), ["code", "--wait", "{file}"]);
        assert_eq!(words("  a \t b  "), ["a", "b"]);
        assert!(words("").is_empty() && words("   ").is_empty());
        assert_eq!(
            words(r#""C:\Program Files\Code\code.exe" -g {file}"#),
            [r"C:\Program Files\Code\code.exe", "-g", "{file}"]
        );
        assert_eq!(words("say 'it''s' \"a 'b'\""), ["say", "its", "a 'b'"]);
        assert_eq!(words("x\"y z\"w"), ["xy zw"]);
        assert_eq!(
            words("a '' b"),
            ["a", "", "b"],
            "an empty quoted word stays"
        );
        assert_eq!(words(r"a\ b"), [r"a\", "b"], "a backslash is not an escape");
        assert_eq!(split_command("vim 'file"), None);
        assert_eq!(split_command("vim \"file"), None);
    }

    #[test]
    fn the_file_replaces_its_placeholder_or_is_appended() {
        let file = OsStr::new("/repo/a b.txt");
        let argv = |line| editor_argv(line, file).unwrap();
        let os = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(argv("code -g {file}"), os(&["code", "-g", "/repo/a b.txt"]));
        assert_eq!(argv("gedit"), os(&["gedit", "/repo/a b.txt"]));
        assert_eq!(
            argv("ed --x={file}:1 {file}"),
            os(&["ed", "--x=/repo/a b.txt:1", "/repo/a b.txt"])
        );
        assert_eq!(argv("{file}"), os(&["{file}", "/repo/a b.txt"]));
        assert!(editor_argv("  ", file).is_err());
        assert!(editor_argv("vim 'x", file).is_err());
    }
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
