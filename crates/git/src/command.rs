//! Esecuzione di `git` secondo le regole di §5: mai una shell, argomenti separati, opzioni e
//! ambiente fissi.

use project_engine::EngineError;
use std::ffi::{OsStr, OsString};
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
        for key in REPO_ENV.iter().chain(PATHSPEC_ENV) {
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

/// [`EngineError::Git`] per un comando non riuscito, con il sottocomando e stderr.
pub(crate) fn failure(args: &[&str], output: &Output) -> EngineError {
    let mut stderr = stderr_text(output);
    if stderr.is_empty() {
        stderr = format!("terminato con {}", output.status);
    }
    EngineError::Git {
        command: subcommand(args).to_owned(),
        stderr,
    }
}

/// stderr senza spazi iniziali e finali (convertito con `from_utf8_lossy`).
pub(crate) fn stderr_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

/// Il primo argomento che non è un'opzione globale, es. `status` in
/// `--no-optional-locks status ...`.
fn subcommand<'a>(args: &[&'a str]) -> &'a str {
    args.iter()
        .copied()
        .find(|arg| !arg.starts_with('-'))
        .unwrap_or("")
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
        for key in REPO_ENV.iter().chain(PATHSPEC_ENV) {
            assert_eq!(value(key), Some(None), "{key}");
        }
    }

    #[test]
    fn path_drops_trailing_newline() {
        assert_eq!(path_from_output(b"/tmp/repo\n"), PathBuf::from("/tmp/repo"));
        assert_eq!(path_from_output(b"/tmp/repo"), PathBuf::from("/tmp/repo"));
    }
}
