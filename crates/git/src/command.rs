//! Esecuzione di `git` secondo le regole di §5: mai una shell, argomenti separati, opzioni e
//! ambiente fissi.

use project_engine::EngineError;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Variabili che dicono a git quale repo usare: ereditate (es. da un hook git che avvia l'app)
/// prevarrebbero su `-C` e farebbero leggere un altro repo. Si tolgono sempre.
const REPO_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
];

/// Configurazione passata dall'ambiente (`git -c` di un processo git che avvia l'app, oppure
/// `GIT_CONFIG_COUNT` con `GIT_CONFIG_KEY_<n>`/`GIT_CONFIG_VALUE_<n>`): vale come la riga di
/// comando, quindi prevale sulla config del repo, e un `core.worktree` iniettato così
/// sposterebbe le scritture in un'altra cartella. Si tolgono sempre, insieme a ogni
/// `GIT_CONFIG_KEY_*`/`GIT_CONFIG_VALUE_*` ereditata ([`inherited_config_vars`]). Le variabili
/// impostate apposta con [`Git::set_env`] (es. `GIT_CONFIG_GLOBAL` nei test) restano.
const CONFIG_ENV: &[&str] = &["GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT"];

/// Le variabili `GIT_CONFIG_KEY_*` e `GIT_CONFIG_VALUE_*` fra `vars` (i nomi delle variabili
/// d'ambiente del processo).
fn inherited_config_vars(vars: impl Iterator<Item = OsString>) -> Vec<OsString> {
    vars.filter(|key| {
        key.to_str()
            .is_some_and(|k| k.starts_with("GIT_CONFIG_KEY_") || k.starts_with("GIT_CONFIG_VALUE_"))
    })
    .collect()
}

/// Date di autore e committer ereditate (es. un'app avviata da un hook git o da uno script che
/// le imposta): darebbero a ogni commit, e ai commit di backup, la data di allora invece di
/// quella attuale. Si tolgono sempre. Nome ed email (`GIT_AUTHOR_NAME`, `GIT_COMMITTER_EMAIL`,
/// ...) invece restano, come per git da terminale: sono l'identità scelta dall'utente.
const DATE_ENV: &[&str] = &["GIT_AUTHOR_DATE", "GIT_COMMITTER_DATE"];

/// Variabili che cambiano il significato dei pathspec (glob, maiuscole, letterali): i percorsi
/// passati a git devono valere per quello che sono. Si tolgono sempre; dove servono percorsi
/// letterali si passa `--literal-pathspecs`.
const PATHSPEC_ENV: &[&str] = &[
    "GIT_LITERAL_PATHSPECS",
    "GIT_GLOB_PATHSPECS",
    "GIT_NOGLOB_PATHSPECS",
    "GIT_ICASE_PATHSPECS",
];

/// Come avviare `git`: il programma e le variabili d'ambiente aggiunte da chi lo configura.
#[derive(Debug, Clone)]
pub(crate) struct Git {
    program: OsString,
    env: Vec<(OsString, OsString)>,
}

impl Git {
    pub(crate) fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            env: Vec::new(),
        }
    }

    pub(crate) fn set_env(&mut self, key: OsString, value: OsString) {
        self.env.push((key, value));
    }

    /// `git [-C <dir>] -c core.quotePath=false -c color.ui=false <args> <paths>`, con stdin
    /// vuoto, `GIT_TERMINAL_PROMPT=0` e `LC_ALL=C`. Il resto dell'ambiente si eredita (SSH
    /// agent, credential helper). `paths` vanno dopo un `--` che chi chiama mette in fondo ad
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
        for key in REPO_ENV
            .iter()
            .chain(PATHSPEC_ENV)
            .chain(CONFIG_ENV)
            .chain(DATE_ENV)
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

    /// Esegue git e restituisce l'output qualunque sia il codice di uscita. Errore solo se il
    /// programma non si avvia.
    pub(crate) fn output(&self, dir: Option<&Path>, args: &[&str]) -> Result<Output, EngineError> {
        self.output_with_paths(dir, args, &[])
    }

    /// Come [`Git::output`], con `paths` dopo `args` (che deve finire con `--`).
    pub(crate) fn output_with_paths(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        paths: &[&OsStr],
    ) -> Result<Output, EngineError> {
        debug_assert!(paths.is_empty() || args.last() == Some(&"--"));
        self.command(dir, args, paths).output().map_err(|e| {
            EngineError::GitNotFound(format!("{}: {e}", self.program.to_string_lossy()))
        })
    }

    /// Come [`Git::output`], con argomenti che non sono UTF-8 (es. `--path=<percorso>`).
    pub(crate) fn output_os(
        &self,
        dir: Option<&Path>,
        args: &[&OsStr],
    ) -> Result<Output, EngineError> {
        self.command(dir, &[], args).output().map_err(|e| {
            EngineError::GitNotFound(format!("{}: {e}", self.program.to_string_lossy()))
        })
    }

    /// Come [`Git::output_os`], con in più le variabili `env` solo per questa esecuzione.
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
            EngineError::GitNotFound(format!("{}: {e}", self.program.to_string_lossy()))
        })
    }

    /// Come [`Git::output`], ma scrive `input` su stdin di git (es. i percorsi per
    /// `--pathspec-from-file=-`, o un messaggio di commit). stdin viene scritto da un thread a
    /// parte, così un output grande non blocca git mentre aspetta di leggere.
    pub(crate) fn output_with_input(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        input: &[u8],
    ) -> Result<Output, EngineError> {
        self.output_with_input_env(dir, args, input, &[])
    }

    /// Come [`Git::output_with_input`], con in più le variabili `env` solo per questa
    /// esecuzione (es. `GIT_INDEX_FILE` per un indice temporaneo).
    pub(crate) fn output_with_input_env(
        &self,
        dir: Option<&Path>,
        args: &[&str],
        input: &[u8],
        env: &[(&str, &OsStr)],
    ) -> Result<Output, EngineError> {
        let not_found = |e: std::io::Error| {
            EngineError::GitNotFound(format!("{}: {e}", self.program.to_string_lossy()))
        };
        let mut cmd = self.command(dir, args, &[]);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let mut child = cmd.stdin(Stdio::piped()).spawn().map_err(not_found)?;
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin è stato chiesto con `Stdio::piped`");
        std::thread::scope(|scope| {
            // Se git termina prima di leggere tutto (es. per un errore) la scrittura fallisce:
            // conta il codice di uscita di git, non l'errore di scrittura.
            scope.spawn(move || {
                let _ = stdin.write_all(input);
            });
            child.wait_with_output().map_err(not_found)
        })
    }

    /// Esegue git e restituisce stdout; un codice di uscita diverso da zero dà
    /// [`EngineError::Git`] con il testo di stderr.
    pub(crate) fn run(&self, dir: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, EngineError> {
        self.run_with_paths(dir, args, &[])
    }

    /// Come [`Git::run`], con `paths` dopo `args` (che deve finire con `--`).
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

/// Un pezzo dello stderr di un comando lento: il testo fino a un `\r` o un `\n`, e se finiva
/// con `\n` (riga finita) o no (riga di avanzamento che git riscrive al prossimo `\r`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Segment {
    pub(crate) text: String,
    pub(crate) newline: bool,
}

/// Come è finito un comando lento ([`Git::run_streaming`]).
#[derive(Debug)]
pub(crate) struct Streamed {
    pub(crate) status: ExitStatus,
    /// Vero se il processo è stato fermato perché annullato.
    pub(crate) cancelled: bool,
    /// stdout, se chiesto (`capture_stdout`), altrimenti vuoto.
    pub(crate) stdout: Vec<u8>,
}

/// Dopo l'uscita di git, quanto aspettare che stderr si chiuda: un processo figlio rimasto in
/// background (es. un master SSH con `ControlPersist`) può tenerlo aperto per minuti.
const PIPE_GRACE: Duration = Duration::from_secs(1);
/// Dopo SIGTERM, quanto aspettare prima di SIGKILL.
const TERM_GRACE: Duration = Duration::from_secs(3);
/// Ogni quanto controllare annullamento e uscita mentre non arriva output.
const POLL: Duration = Duration::from_millis(50);

impl Git {
    /// Esegue un comando lento (`git fetch --progress`) leggendo stderr mentre gira: ogni
    /// pezzo separato da `\r` o `\n` va a `on_segment`, sul thread di chi chiama. stdin è
    /// chiuso (`/dev/null`), `GIT_TERMINAL_PROMPT=0` e, su Unix, il processo parte in una
    /// sessione nuova (`setsid`) **senza terminale di controllo**: né git né ssh né un
    /// credential helper possono aprire `/dev/tty` per chiedere qualcosa, quindi il comando
    /// non resta mai ad aspettare input (le richieste grafiche, `SSH_ASKPASS`/`GIT_ASKPASS`,
    /// restano possibili). stdout si scarta.
    ///
    /// Se `cancelled()` diventa vero il comando si ferma: SIGTERM al gruppo di processi (git,
    /// ssh, `git-remote-http`, `index-pack`: git toglie i propri file `.lock` quando lo
    /// riceve), dopo [`TERM_GRACE`] SIGKILL; su altri sistemi `kill`.
    ///
    /// Con `capture_stdout` stdout si legge (da un altro thread) e torna in
    /// [`Streamed::stdout`] (es. `git push --porcelain`); dopo l'uscita di git si aspetta al più
    /// [`PIPE_GRACE`] che si chiuda. Altrimenti si scarta.
    pub(crate) fn run_streaming(
        &self,
        dir: &Path,
        args: &[&str],
        capture_stdout: bool,
        mut on_segment: impl FnMut(Segment),
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Streamed, EngineError> {
        let not_found = |e: std::io::Error| {
            EngineError::GitNotFound(format!("{}: {e}", self.program.to_string_lossy()))
        };
        let mut cmd = self.command(Some(dir), args, &[]);
        if !capture_stdout {
            cmd.stdout(Stdio::null());
        }
        detach_from_terminal(&mut cmd);
        let mut child = cmd.spawn().map_err(not_found)?;
        let stderr = child
            .stderr
            .take()
            .expect("stderr è stato chiesto con `Stdio::piped`");
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
        // Il thread finisce quando stderr si chiude; se un processo rimasto in background lo
        // tiene aperto, resta bloccato senza che il comando lo aspetti.
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
            .and_then(|r| r.recv_timeout(PIPE_GRACE).ok())
            .unwrap_or_default();
        Ok(Streamed {
            status,
            cancelled: terminated.is_some(),
            stdout,
        })
    }
}

/// Legge `input` fino alla fine e lo divide in pezzi a ogni `\r` o `\n` (i pezzi vuoti si
/// saltano); `emit` restituisce `false` per smettere.
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

/// Su Unix il processo parte in una sessione nuova, senza terminale di controllo (`setsid`):
/// non può aprire `/dev/tty`, ed è capo del suo gruppo di processi, che [`terminate`] ferma
/// tutto insieme.
#[cfg(unix)]
fn detach_from_terminal(cmd: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: fra `fork` ed `exec` si chiama solo `setsid`, che è async-signal-safe e non
    // alloca.
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

/// SIGTERM al gruppo di processi di `child` (git e i suoi figli: ssh, helper, `index-pack`).
#[cfg(unix)]
fn terminate(child: &mut Child) {
    signal_group(child, libc::SIGTERM);
}

/// SIGKILL al gruppo di processi di `child`.
#[cfg(unix)]
fn kill(child: &mut Child) {
    signal_group(child, libc::SIGKILL);
}

#[cfg(unix)]
fn signal_group(child: &mut Child, signal: libc::c_int) {
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `kill` non tocca la memoria; con `setsid` il gruppo ha l'id del processo.
        // Se il gruppo non c'è più l'errore si ignora.
        unsafe {
            libc::kill(-pid, signal);
        }
    }
}

#[cfg(not(unix))]
fn terminate(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill(child: &mut Child) {
    let _ = child.kill();
}

/// [`EngineError::Git`] per un comando non riuscito, con il sottocomando e stderr; se git non ha
/// potuto prendere il lock dell'indice, [`EngineError::IndexLocked`].
pub(crate) fn failure(args: &[&str], output: &Output) -> EngineError {
    let mut stderr = stderr_text(output);
    if let Some(lock) = locked_index(&stderr) {
        return EngineError::IndexLocked { lock };
    }
    if stderr.is_empty() {
        stderr = format!("terminato con {}", output.status);
    }
    EngineError::Git {
        command: subcommand(args).to_owned(),
        stderr,
    }
}

/// Il file di lock nel messaggio di git `fatal: Unable to create '<lock>': File exists.` (con
/// `LC_ALL=C` il testo è sempre questo), se è il lock dell'indice.
fn locked_index(stderr: &str) -> Option<String> {
    const START: &str = "Unable to create '";
    const END: &str = "': File exists.";
    let start = stderr.find(START)? + START.len();
    let len = stderr[start..].find(END)?;
    let lock = &stderr[start..start + len];
    lock.ends_with("index.lock").then(|| lock.to_owned())
}

/// stderr senza spazi iniziali e finali (convertito con `from_utf8_lossy`).
pub(crate) fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

/// Il primo argomento che non è un'opzione globale, es. `status` in
/// `--no-optional-locks status ...` o in `-c diff.x=y status ...`.
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    let mut args = args.iter().copied();
    while let Some(arg) = args.next() {
        if arg == "-c" {
            // Il valore di `-c` non è il sottocomando.
            args.next();
        } else if !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

/// Un percorso stampato da git su una riga (es. `rev-parse --show-toplevel`), senza l'a capo
/// finale. I byte non UTF-8 restano tali dove il sistema lo permette.
pub(crate) fn path_from_output(line: &[u8]) -> PathBuf {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    os_from_bytes(line).into()
}

/// Byte di un percorso (es. un `RepoPath`) come argomento per git.
#[cfg(unix)]
pub(crate) fn os_from_bytes(bytes: &[u8]) -> OsString {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(bytes).to_owned()
}

#[cfg(not(unix))]
pub(crate) fn os_from_bytes(bytes: &[u8]) -> OsString {
    // Su Windows git stampa i percorsi in UTF-8.
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
        // Ereditate da un hook git farebbero leggere un altro repo: tolte.
        // Così come quelle che cambiano il significato dei pathspec.
        for key in REPO_ENV
            .iter()
            .chain(PATHSPEC_ENV)
            .chain(CONFIG_ENV)
            .chain(DATE_ENV)
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
            .output_with_input(None, &["hash-object", "--stdin"], b"ciao\n")
            .unwrap();
        // `printf 'ciao\n' | git hash-object --stdin`
        assert!(out.status.success());
        assert_eq!(out.stdout, b"887ae9333d92a1d72400c210546e28baa1050e44\n");
    }

    #[test]
    fn stderr_is_split_at_carriage_returns_and_newlines() {
        let input: &[u8] = b"Receiving objects:  50% (1/2)\rReceiving objects: 100% (2/2), done.\n\
            From /r\n\r\nultima";
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
                ("ultima".to_owned(), true),
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
