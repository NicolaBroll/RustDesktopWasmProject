//! Running `git` per the rules of §5: never a shell, separate arguments, fixed options and
//! environment.

use crate::askpass::{ASKPASS_ENV, Askpass, Bridge};
use project_engine::{CredentialPrompter, EngineError};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Variables that tell git which repo to use: inherited (e.g. from a git hook that starts the app)
/// they would override `-C` and make git read another repo. Always removed.
const REPO_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// Configuration passed through the environment (`git -c` of a git process that starts the app, or
/// `GIT_CONFIG_COUNT` with `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>`): it counts like the
/// command line, so it overrides the repo config, and a `core.worktree` injected this way
/// would move writes to another directory. Always removed, together with every inherited
/// `GIT_CONFIG_KEY_*`/`GIT_CONFIG_VALUE_*` ([`inherited_config_vars`]). Variables
/// set on purpose with [`Git::set_env`] (e.g. `GIT_CONFIG_GLOBAL` in tests) stay.
const CONFIG_ENV: &[&str] = &["GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT"];

/// The `GIT_CONFIG_KEY_*` and `GIT_CONFIG_VALUE_*` variables among `vars` (the names of the
/// process environment variables).
fn inherited_config_vars(vars: impl Iterator<Item = OsString>) -> Vec<OsString> {
    vars.filter(|key| {
        key.to_str()
            .is_some_and(|k| k.starts_with("GIT_CONFIG_KEY_") || k.starts_with("GIT_CONFIG_VALUE_"))
    })
    .collect()
}

/// Inherited author and committer dates (e.g. an app started by a git hook or by a script that
/// sets them): they would give every commit, and the backup commits, that old date instead of
/// the current one. Always removed. Name and email (`GIT_AUTHOR_NAME`, `GIT_COMMITTER_EMAIL`,
/// ...) stay instead, as for git from a terminal: they are the identity chosen by the user.
const DATE_ENV: &[&str] = &["GIT_AUTHOR_DATE", "GIT_COMMITTER_DATE"];

/// Variables that change the meaning of pathspecs (glob, case, literal): the paths
/// passed to git must mean what they are. Always removed; where literal paths are needed
/// `--literal-pathspecs` is passed.
const PATHSPEC_ENV: &[&str] = &[
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
];

/// Variables that point git and ssh to an askpass program, or tell the app to act as one: set
/// only by [`Git::run_network`] with a prompter, otherwise removed so an inherited value
/// (e.g. the app started from a git hook of another askpass session) does not reach git.
const ASKPASS_VARS: &[&str] = &[ASKPASS_ENV];

/// How to start `git`: the program, the environment variables added by whoever configures it
/// and, if set, how to ask the user for credentials in the network commands.
#[derive(Debug, Clone)]
pub(crate) struct Git {
    program: OsString,
    env: Vec<(OsString, OsString)>,
    askpass: Option<Askpass>,
}

impl Git {
    pub(crate) fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            env: Vec::new(),
            askpass: None,
        }
    }

    pub(crate) fn set_env(&mut self, key: OsString, value: OsString) {
        self.env.push((key, value));
    }

    /// The network commands ([`Git::run_network`]) ask `prompter` for credentials, with
    /// `program` as git's and ssh's askpass.
    pub(crate) fn set_prompter(
        &mut self,
        prompter: Arc<dyn CredentialPrompter>,
        program: OsString,
    ) {
        self.askpass = Some(Askpass { program, prompter });
    }

    /// `git [-C <dir>] -c core.quotePath=false -c color.ui=false <args> <paths>`, with empty
    /// stdin, `GIT_TERMINAL_PROMPT=0` and `LC_ALL=C`. The rest of the environment is inherited (SSH
    /// agent, credential helper). `paths` go after a `--` that the caller puts at the end of
    /// `args`.
    fn command(&self, dir: Option<&Path>, args: &[&str], paths: &[&OsStr]) -> Command {
        let mut cmd = Command::new(&self.program);
        if let Some(dir) = dir {
            cmd.arg("-C").arg(dir);
        }
        cmd.args(["-c", "core.quotePath=false", "-c", "color.ui=false"])
            .args(args)
            .args(paths)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        no_console_window(&mut cmd);
        for key in REPO_ENV
            .iter()
            .chain(PATHSPEC_ENV)
            .chain(CONFIG_ENV)
            .chain(DATE_ENV)
            .chain(ASKPASS_VARS)
        {
            cmd.env_remove(key);
        }
        for key in inherited_config_vars(std::env::vars_os().map(|(k, _)| k)) {
            cmd.env_remove(key);
        }
        for (key, value) in &self.env {
            cmd.env(key, value);
        }
        cmd
    }

    /// Runs git and returns the output whatever the exit code. Error only if the
    /// program does not start.
    pub(crate) fn output(&self, dir: Option<&Path>, args: &[&str]) -> Result<Output, EngineError> {
        self.output_with_paths(dir, args, &[])
    }

    /// Like [`Git::output`], with `paths` after `args` (which must end with `--`).
    pub(crate) fn output_with_paths(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        paths: &[&OsStr],
    ) -> Result<Output, EngineError> {
        debug_assert!(paths.is_empty() || args.last() == Some(&"--"));
        self.command(dir, args, paths).output().map_err(|e| {
            EngineError::GitNotFound(msg!(
                "engine-git-spawn",
                program = self.program.to_string_lossy(),
                error = e
            ))
        })
    }

    /// Like [`Git::output`], with arguments that are not UTF-8 (e.g. `--path=<path>`).
    pub(crate) fn output_os(
        &self,
        dir: Option<&Path>,
        args: &[&OsStr],
    ) -> Result<Output, EngineError> {
        self.command(dir, &[], args).output().map_err(|e| {
            EngineError::GitNotFound(msg!(
                "engine-git-spawn",
                program = self.program.to_string_lossy(),
                error = e
            ))
        })
    }

    /// Like [`Git::output_os`], with the extra variables `env` for this run only.
    pub(crate) fn output_os_env(
        &self,
        dir: Option<&Path>,
        args: &[&OsStr],
        env: &[(&str, &OsStr)],
    ) -> Result<Output, EngineError> {
        let mut cmd = self.command(dir, &[], args);
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.output().map_err(|e| {
            EngineError::GitNotFound(msg!(
                "engine-git-spawn",
                program = self.program.to_string_lossy(),
                error = e
            ))
        })
    }

    /// Like [`Git::output`], but writes `input` to git's stdin (e.g. the paths for
    /// `--pathspec-from-file=-`, or a commit message). stdin is written by a separate
    /// thread, so large output does not block git while it waits to read.
    pub(crate) fn output_with_input(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        input: &[u8],
    ) -> Result<Output, EngineError> {
        self.output_with_input_env(dir, args, input, &[])
    }

    /// Like [`Git::output_with_input`], with the extra variables `env` for this
    /// run only (e.g. `GIT_INDEX_FILE` for a temporary index).
    pub(crate) fn output_with_input_env(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        input: &[u8],
        env: &[(&str, &OsStr)],
    ) -> Result<Output, EngineError> {
        let not_found = |e: std::io::Error| {
            EngineError::GitNotFound(msg!(
                "engine-git-spawn",
                program = self.program.to_string_lossy(),
                error = e
            ))
        };
        let mut cmd = self.command(dir, args, &[]);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.stdin(Stdio::piped()).spawn().map_err(not_found)?;
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin was requested with `Stdio::piped`");
        std::thread::scope(|scope| {
            // If git exits before reading everything (e.g. because of an error) the write fails:
            // the git exit code counts, not the write error.
            scope.spawn(move || {
                let _ = stdin.write_all(input);
            });
            child.wait_with_output().map_err(not_found)
        })
    }

    /// Runs git and returns stdout; a non-zero exit code gives
    /// [`EngineError::Git`] with the stderr text.
    pub(crate) fn run(&self, dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, EngineError> {
        self.run_with_paths(dir, args, &[])
    }

    /// Like [`Git::run`], with `paths` after `args` (which must end with `--`).
    pub(crate) fn run_with_paths(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        paths: &[&OsStr],
    ) -> Result<Vec<u8>, EngineError> {
        let output = self.output_with_paths(dir, args, paths)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(failure(args, &output))
        }
    }
}

/// A piece of the stderr of a slow command: the text up to a `\r` or a `\n`, and whether it ended
/// with `\n` (finished line) or not (progress line that git rewrites at the next `\r`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) text: String,
    pub(crate) newline: bool,
}

/// How a slow command ended ([`Git::run_streaming`]).
#[derive(Debug)]
pub(crate) struct Streamed {
    pub(crate) status: ExitStatus,
    /// True if the process was stopped because it was cancelled.
    pub(crate) cancelled: bool,
    /// stdout, if requested (`capture_stdout`), otherwise empty.
    pub(crate) stdout: Vec<u8>,
    /// True if git failed after the user cancelled a credential question
    /// ([`Git::run_network`]): the error is [`EngineError::AuthCancelled`].
    pub(crate) auth_cancelled: bool,
}

/// What to do with stdout in [`Git::stream`].
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stdout {
    Discard,
    /// Read, waiting at most [`PIPE_GRACE`] after git exits (a process left in the background
    /// may keep it open).
    Grace,
    /// Read to the end: a read whose output is the result.
    Full,
}

/// After git exits, how long to wait for stderr to close: a child process left in the
/// background (e.g. an SSH master with `ControlPersist`) can keep it open for minutes.
const PIPE_GRACE: Duration = Duration::from_secs(1);
/// After SIGTERM, how long to wait before SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(3);
/// How often to check for cancellation and exit while no output arrives.
const POLL: Duration = Duration::from_millis(50);

impl Git {
    /// Runs a slow command (`git fetch --progress`) reading stderr while it runs: each
    /// piece separated by `\r` or `\n` goes to `on_segment`, on the caller's thread. stdin is
    /// closed (`/dev/null`), `GIT_TERMINAL_PROMPT=0` and, on Unix, the process starts in a
    /// new session (`setsid`) **with no controlling terminal**: neither git nor ssh nor a
    /// credential helper can open `/dev/tty` to ask for anything, so the command
    /// never waits for input (graphical prompts, `SSH_ASKPASS`/`GIT_ASKPASS`,
    /// remain possible). stdout is discarded.
    ///
    /// If `cancelled()` becomes true the command stops: SIGTERM to the process group (git,
    /// ssh, `git-remote-http`, `index-pack`: git removes its own `.lock` files when it
    /// receives it), after [`TERM_GRACE`] SIGKILL; on other systems `kill`.
    ///
    /// With `capture_stdout` stdout is read (from another thread) and returned in
    /// [`Streamed::stdout`] (e.g. `git push --porcelain`); after git exits we wait at most
    /// [`PIPE_GRACE`] for it to close. Otherwise it is discarded.
    pub(crate) fn run_streaming(
        &self,
        dir: &Path,
        args: &[&str],
        capture_stdout: bool,
        on_segment: impl FnMut(Segment),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Streamed, EngineError> {
        let stdout = if capture_stdout {
            Stdout::Grace
        } else {
            Stdout::Discard
        };
        self.stream(
            self.command(Some(dir), args, &[]),
            stdout,
            on_segment,
            cancelled,
        )
    }

    /// A command that contacts the remote `remote` (fetch, push, clone; name or URL): like
    /// [`Git::run_streaming`] and, if a prompter is set, with the askpass bridge
    /// (`crate::askpass`): git and ssh ask the user for user name, password, passphrase or host
    /// key through the app. `GIT_TERMINAL_PROMPT=0` stays: git tries askpass before the
    /// terminal. Without a prompter (MCP, tests) or if the bridge cannot start, exactly
    /// [`Git::run_streaming`].
    ///
    /// If git fails after the user cancelled a question, [`Streamed::auth_cancelled`]: the
    /// caller reports [`EngineError::AuthCancelled`] where it would report
    /// [`EngineError::Auth`] (`Streamed::remote_error`).
    pub(crate) fn run_network(
        &self,
        dir: &Path,
        args: &[&str],
        capture_stdout: bool,
        remote: &str,
        on_segment: impl FnMut(Segment),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Streamed, EngineError> {
        let stdout = if capture_stdout {
            Stdout::Grace
        } else {
            Stdout::Discard
        };
        // The user's own askpass is never overridden: git keeps `GIT_ASKPASS`, `core.askPass`
        // or `SSH_ASKPASS` (git's fallback), ssh keeps `SSH_ASKPASS` and the user's
        // `SSH_ASKPASS_REQUIRE`; a side already served is not bridged.
        let bridged = self.askpass.as_ref().and_then(|askpass| {
            if !askpass.prompter.asks_now() {
                return None;
            }
            let user_ssh = self.env_set("SSH_ASKPASS");
            let git_side =
                !user_ssh && !self.env_set("GIT_ASKPASS") && !self.askpass_configured(dir);
            let ssh_side = !user_ssh && !self.env_set("SSH_ASKPASS_REQUIRE");
            if !git_side && !ssh_side {
                return None;
            }
            let bridge = Bridge::start(askpass.prompter.clone(), Some(remote.to_owned())).ok()?;
            Some((askpass, bridge, git_side, ssh_side))
        });
        let Some((askpass, bridge, git_side, ssh_side)) = bridged else {
            return self.stream(
                self.command(Some(dir), args, &[]),
                stdout,
                on_segment,
                cancelled,
            );
        };
        let mut env = vec![(ASKPASS_ENV, OsString::from(bridge.env_value()))];
        if git_side {
            env.push(("GIT_ASKPASS", askpass.program.clone()));
        }
        if ssh_side {
            env.push(("SSH_ASKPASS", askpass.program.clone()));
            env.push(("SSH_ASKPASS_REQUIRE", "force".into()));
        }
        let mut cmd = self.command(Some(dir), args, &[]);
        cmd.envs(env);
        let mut streamed = self.stream(cmd, stdout, on_segment, cancelled)?;
        streamed.auth_cancelled = bridge.cancelled() && !streamed.status.success();
        Ok(streamed)
    }

    /// Whether `key` is set, not empty, for git: added with [`Git::set_env`] or inherited.
    fn env_set(&self, key: &str) -> bool {
        match self.env.iter().rev().find(|(k, _)| k == key) {
            Some((_, value)) => !value.is_empty(),
            None => std::env::var_os(key).is_some_and(|v| !v.is_empty()),
        }
    }

    /// Whether `core.askPass` is configured, not empty, in any scope for the repo in `dir`.
    fn askpass_configured(&self, dir: &Path) -> bool {
        self.output(Some(dir), &["config", "--get", "core.askPass"])
            .is_ok_and(|out| out.status.success() && !out.stdout.trim_ascii().is_empty())
    }

    /// A slow read (`git blame`, `git log` of one file) that can be cancelled: like
    /// [`Git::run_streaming`] with `paths` after `args` (which must end with `--`), and all of
    /// stdout, however long git takes to close it after exiting. The stderr lines (trimmed,
    /// without the empty ones) come back joined by newlines next to the [`Streamed`].
    pub(crate) fn run_cancellable(
        &self,
        dir: &Path,
        args: &[&str],
        paths: &[&OsStr],
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(Streamed, String), EngineError> {
        debug_assert!(paths.is_empty() || args.last() == Some(&"--"));
        let mut lines = Vec::new();
        let streamed = self.stream(
            self.command(Some(dir), args, paths),
            Stdout::Full,
            |Segment { text, newline }| {
                if newline && !text.trim().is_empty() {
                    lines.push(text.trim_end().to_owned());
                }
            },
            cancelled,
        )?;
        Ok((streamed, lines.join("\n")))
    }

    /// Runs `cmd` (built by [`Git::command`]) as described in [`Git::run_streaming`].
    fn stream(
        &self,
        mut cmd: Command,
        stdout_mode: Stdout,
        mut on_segment: impl FnMut(Segment),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Streamed, EngineError> {
        let not_found = |e: std::io::Error| {
            EngineError::GitNotFound(msg!(
                "engine-git-spawn",
                program = self.program.to_string_lossy(),
                error = e
            ))
        };
        if stdout_mode == Stdout::Discard {
            cmd.stdout(Stdio::null());
        }
        detach_from_terminal(&mut cmd);
        let mut child = cmd.spawn().map_err(not_found)?;
        let stderr = child
            .stderr
            .take()
            .expect("stderr was requested with `Stdio::piped`");
        let stdout_receiver = child.stdout.take().map(|mut out| {
            let (sender, receiver) = mpsc::channel();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = sender.send(buf);
            });
            receiver
        });
        let (sender, receiver) = mpsc::channel();
        // The thread ends when stderr closes; if a process left in the background keeps it
        // open, it stays blocked without the command waiting for it.
        std::thread::spawn(move || split_segments(stderr, |segment| sender.send(segment).is_ok()));

        let mut exited: Option<(ExitStatus, Instant)> = None;
        let mut terminated: Option<Instant> = None;
        let mut killed = false;
        loop {
            match receiver.recv_timeout(POLL) {
                Ok(segment) => on_segment(segment),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if terminated.is_none() && exited.is_none() && cancelled() {
                terminate(&mut child);
                terminated = Some(Instant::now());
            }
            if let Some(since) = terminated
                && !killed
                && exited.is_none()
                && since.elapsed() > TERM_GRACE
            {
                kill(&mut child);
                killed = true;
            }
            if exited.is_none()
                && let Some(status) = child.try_wait().map_err(not_found)?
            {
                exited = Some((status, Instant::now()));
            }
            if let Some((_, at)) = exited
                && at.elapsed() > PIPE_GRACE
            {
                break;
            }
        }
        let status = match exited {
            Some((status, _)) => status,
            None => child.wait().map_err(not_found)?,
        };
        let stdout = stdout_receiver
            .and_then(|r| match stdout_mode {
                // After a cancellation the process group is gone: the pipe closes.
                Stdout::Full if terminated.is_none() => r.recv().ok(),
                _ => r.recv_timeout(PIPE_GRACE).ok(),
            })
            .unwrap_or_default();
        Ok(Streamed {
            status,
            cancelled: terminated.is_some(),
            stdout,
            auth_cancelled: false,
        })
    }
}

/// Reads `input` to the end and splits it into pieces at every `\r` or `\n` (empty pieces are
/// skipped); `emit` returns `false` to stop.
pub(crate) fn split_segments(mut input: impl Read, mut emit: impl FnMut(Segment) -> bool) {
    let mut pending = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = match input.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        for &b in &buf[..n] {
            if b == b'\r' || b == b'\n' {
                if !pending.is_empty() {
                    let text = String::from_utf8_lossy(&pending).into_owned();
                    pending.clear();
                    if !emit(Segment {
                        text,
                        newline: b == b'\n',
                    }) {
                        return;
                    }
                }
            } else {
                pending.push(b);
            }
        }
    }
    if !pending.is_empty() {
        emit(Segment {
            text: String::from_utf8_lossy(&pending).into_owned(),
            newline: true,
        });
    }
}

/// On Unix the process starts in a new session, with no controlling terminal (`setsid`):
/// it cannot open `/dev/tty`, and it leads its own process group, which [`terminate`] stops
/// all together.
#[cfg(unix)]
fn detach_from_terminal(cmd: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: between `fork` and `exec` only `setsid` is called, which is async-signal-safe and does not
    // allocate.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(not(unix))]
fn detach_from_terminal(_cmd: &mut Command) {}

/// On Windows a console program started by the GUI app would open a console window for every
/// git call: `CREATE_NO_WINDOW` keeps it hidden.
#[cfg(windows)]
fn no_console_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn no_console_window(_cmd: &mut Command) {}

/// SIGTERM to the process group of `child` (git and its children: ssh, helpers, `index-pack`).
#[cfg(unix)]
fn terminate(child: &mut Child) {
    signal_group(child, libc::SIGTERM);
}

/// SIGKILL to the process group of `child`.
#[cfg(unix)]
fn kill(child: &mut Child) {
    signal_group(child, libc::SIGKILL);
}

#[cfg(unix)]
fn signal_group(child: &mut Child, signal: libc::c_int) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `kill` does not touch memory; with `setsid` the group has the process id.
        // If the group no longer exists the error is ignored.
        unsafe {
            libc::kill(-pid, signal);
        }
    }
}

/// Windows has no SIGTERM or process groups here: `TerminateProcess` stops git alone (its
/// children, e.g. ssh, may outlive it) and git cannot remove its lock files (e.g. `index.lock`),
/// which may then remain.
#[cfg(not(unix))]
fn terminate(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill(child: &mut Child) {
    let _ = child.kill();
}

/// [`EngineError::Git`] for a failed command, with the subcommand and stderr; if git could not
/// take the index lock, [`EngineError::IndexLocked`].
pub(crate) fn failure(args: &[&str], output: &Output) -> EngineError {
    let stderr = stderr_text(output);
    if let Some(lock) = locked_index(&stderr) {
        return EngineError::IndexLocked { lock };
    }
    if stderr.is_empty() {
        return EngineError::GitWithReason {
            command: subcommand(args).to_owned(),
            reason: msg!("engine-git-exit-status", status = output.status.to_string()),
        };
    }
    EngineError::Git {
        command: subcommand(args).to_owned(),
        stderr,
    }
}

/// The lock file in git's message `fatal: Unable to create '<lock>': File exists.` (with
/// `LC_ALL=C` the text is always this), if it is the index lock.
fn locked_index(stderr: &str) -> Option<String> {
    const START: &str = "Unable to create '";
    const END: &str = "': File exists.";
    let start = stderr.find(START)? + START.len();
    let len = stderr[start..].find(END)?;
    let lock = &stderr[start..start + len];
    lock.ends_with("index.lock").then(|| lock.to_owned())
}

/// stderr without leading and trailing spaces (converted with `from_utf8_lossy`).
pub(crate) fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

/// The first argument that is not a global option, e.g. `status` in
/// `--no-optional-locks status ...` or in `-c diff.x=y status ...`.
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    let mut args = args.iter().copied();
    while let Some(arg) = args.next() {
        if arg == "-c" {
            // The value of `-c` is not the subcommand.
            args.next();
        } else if !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

/// A path printed by git on a line (e.g. `rev-parse --show-toplevel`), without the trailing
/// newline. Non-UTF-8 bytes stay as they are where the system allows.
pub(crate) fn path_from_output(line: &[u8]) -> PathBuf {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    os_from_bytes(line).into()
}

/// Bytes of a path (e.g. a `RepoPath`) as an argument for git.
#[cfg(unix)]
pub(crate) fn os_from_bytes(bytes: &[u8]) -> OsString {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(bytes).to_owned()
}

#[cfg(not(unix))]
pub(crate) fn os_from_bytes(bytes: &[u8]) -> OsString {
    // On Windows git prints paths in UTF-8.
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcommand_skips_global_options() {
        assert_eq!(
            subcommand(&["--no-optional-locks", "status", "-z"]),
            "status"
        );
        assert_eq!(subcommand(&["log", "-z"]), "log");
        assert_eq!(subcommand(&["-c", "a.b=c", "--x", "diff", "--"]), "diff");
        assert_eq!(subcommand(&["--version"]), "");
    }

    #[test]
    fn command_has_fixed_options_and_environment() {
        let mut git = Git::new("git");
        git.set_env("GIT_CONFIG_NOSYSTEM".into(), "1".into());
        let cmd = git.command(
            Some(Path::new("/repo")),
            &["diff", "--"],
            &[OsStr::new("a b.txt")],
        );
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            [
                "-C",
                "/repo",
                "-c",
                "core.quotePath=false",
                "-c",
                "color.ui=false",
                "diff",
                "--",
                "a b.txt"
            ]
        );
        let envs: Vec<_> = cmd.get_envs().collect();
        let value = |key: &str| envs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v);
        assert_eq!(value("GIT_TERMINAL_PROMPT"), Some(Some("0".as_ref())));
        assert_eq!(value("LC_ALL"), Some(Some("C".as_ref())));
        assert_eq!(value("GIT_CONFIG_NOSYSTEM"), Some(Some("1".as_ref())));
        // Inherited from a git hook they would make git read another repo: removed.
        // Likewise those that change the meaning of pathspecs.
        for key in REPO_ENV
            .iter()
            .chain(PATHSPEC_ENV)
            .chain(CONFIG_ENV)
            .chain(DATE_ENV)
            .chain(ASKPASS_VARS)
        {
            assert_eq!(value(key), Some(None), "{key}");
        }
    }

    #[test]
    fn inherited_config_is_dropped() {
        let vars = [
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_KEY_12",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_NOSYSTEM",
            "PATH",
        ]
        .map(OsString::from);
        assert_eq!(
            inherited_config_vars(vars.into_iter()),
            [
                "GIT_CONFIG_KEY_0",
                "GIT_CONFIG_VALUE_0",
                "GIT_CONFIG_KEY_12"
            ]
        );
        assert!(CONFIG_ENV.contains(&"GIT_CONFIG_PARAMETERS"));
        assert!(CONFIG_ENV.contains(&"GIT_CONFIG_COUNT"));
    }

    #[test]
    fn index_lock_is_recognized() {
        let stderr = "fatal: Unable to create '/r/.git/index.lock': File exists.\n\nAnother git \
                      process seems to be running in this repository";
        assert_eq!(locked_index(stderr).as_deref(), Some("/r/.git/index.lock"));
        let other = "fatal: Unable to create '/r/.git/refs/heads/main.lock': File exists.";
        assert_eq!(locked_index(other), None);
        assert_eq!(
            locked_index("fatal: pathspec 'x' did not match any files"),
            None
        );
    }

    #[test]
    fn input_goes_to_stdin() {
        let git = Git::new("git");
        let out = git
            .output_with_input(None, &["hash-object", "--stdin"], b"hello\n")
            .unwrap();
        // `printf 'hello\n' | git hash-object --stdin`
        assert!(out.status.success());
        assert_eq!(out.stdout, b"ce013625030ba8dba906f756967f9e9ca394464a\n");
    }

    #[test]
    fn stderr_is_split_at_carriage_returns_and_newlines() {
        let input: &[u8] = b"Receiving objects:  50% (1/2)\rReceiving objects: 100% (2/2), done.\n\
            From /r\n\r\nlast";
        let mut seen = Vec::new();
        split_segments(input, |s| {
            seen.push((s.text, s.newline));
            true
        });
        assert_eq!(
            seen,
            [
                ("Receiving objects:  50% (1/2)".to_owned(), false),
                ("Receiving objects: 100% (2/2), done.".to_owned(), true),
                ("From /r".to_owned(), true),
                ("last".to_owned(), true),
            ]
        );
    }

    #[test]
    fn streaming_reads_stderr() {
        let git = Git::new("git");
        let mut lines = Vec::new();
        let out = git
            .run_streaming(
                Path::new("."),
                &["hash-object", "--stdin", "--no-such-option"],
                false,
                |s| lines.push(s.text),
                &|| false,
            )
            .unwrap();
        assert!(!out.status.success());
        assert!(!out.cancelled);
        assert!(
            lines.iter().any(|l| l.contains("no-such-option")),
            "{lines:?}"
        );
    }

    #[test]
    fn streaming_can_capture_stdout() {
        let git = Git::new("git");
        let out = git
            .run_streaming(Path::new("."), &["--version"], true, |_| {}, &|| false)
            .unwrap();
        assert!(out.status.success());
        assert!(out.stdout.starts_with(b"git version "), "{:?}", out.stdout);
    }

    #[test]
    fn path_drops_trailing_newline() {
        assert_eq!(path_from_output(b"/tmp/repo\n"), PathBuf::from("/tmp/repo"));
        assert_eq!(path_from_output(b"/tmp/repo"), PathBuf::from("/tmp/repo"));
    }
}
