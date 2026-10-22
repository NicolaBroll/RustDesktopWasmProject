//! Esecuzione di `git` secondo le regole di §5: mai una shell, argomenti separati, opzioni e
//! ambiente fissi.

use project_engine::EngineError;
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
        for key in REPO_ENV.iter().chain(PATHSPEC_ENV).chain(CONFIG_ENV) {
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
        for key in REPO_ENV.iter().chain(PATHSPEC_ENV).chain(CONFIG_ENV) {
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
    fn path_drops_trailing_newline() {
        assert_eq!(path_from_output(b"/tmp/repo\n"), PathBuf::from("/tmp/repo"));
        assert_eq!(path_from_output(b"/tmp/repo"), PathBuf::from("/tmp/repo"));
    }
}
