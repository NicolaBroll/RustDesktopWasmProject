//! [`CliBackend`] e [`CliOpener`]: lettura di un repository eseguendo `git`.

use crate::GitVersion;
use crate::command::{Git, failure, path_from_output, stderr_text};
use project_domain::{Commit, Head, RefInfo, Status};
use project_domain::{
    LOG_ARGS, Oid, Operation, REFS_ARGS, STATUS_ARGS, parse_log, parse_refs, parse_status,
};
use project_engine::{EngineError, GitBackend, LogQuery, RepoOpener};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Apre repository con il `git` di sistema. È il [`RepoOpener`] del desktop e della CLI.
///
/// A ogni apertura controlla che git ci sia e che la versione sia almeno [`GitVersion::MIN`].
#[derive(Debug, Clone)]
pub struct CliOpener {
    git: Git,
}

impl Default for CliOpener {
    fn default() -> Self {
        Self::new()
    }
}

impl CliOpener {
    /// Usa `git` dal `PATH`.
    pub fn new() -> Self {
        Self::with_program("git")
    }

    /// Usa il programma indicato al posto di `git` (es. un percorso assoluto).
    pub fn with_program(program: impl Into<OsString>) -> Self {
        Self {
            git: Git::new(program),
        }
    }

    /// Aggiunge una variabile d'ambiente a ogni esecuzione di git, anche dei backend aperti.
    /// Serve ai test per un ambiente deterministico (es. `GIT_CONFIG_GLOBAL=/dev/null`).
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.git.set_env(key.into(), value.into());
        self
    }

    /// Non cerca repository sopra `root`: aprendo una cartella dentro `root` che non è un repo,
    /// git non risale fino a un repo che contiene `root` (`GIT_CEILING_DIRECTORIES`). Un repo
    /// in `root` o sotto si apre come sempre. Serve al server MCP per restare nella radice; `root` va dato assoluto e senza link
    /// simbolici (`canonicalize`).
    pub fn confine_to(self, root: &Path) -> Self {
        match root.parent() {
            Some(parent) => self.env("GIT_CEILING_DIRECTORIES", parent),
            None => self,
        }
    }

    /// La versione del git configurato. [`EngineError::GitNotFound`] se non si avvia.
    pub fn git_version(&self) -> Result<GitVersion, EngineError> {
        let out = self.git.run(None, &["--version"])?;
        let text = String::from_utf8_lossy(&out);
        GitVersion::parse(&text).ok_or_else(|| {
            EngineError::GitNotFound(format!(
                "risposta di `git --version` non riconosciuta: `{}`",
                text.trim()
            ))
        })
    }

    /// Come [`RepoOpener::open`], ma restituisce il tipo concreto.
    pub fn open_cli(&self, path: &str) -> Result<CliBackend, EngineError> {
        let version = self.git_version()?;
        if !version.is_supported() {
            return Err(EngineError::GitTooOld {
                found: version.to_string(),
                required: GitVersion::MIN.to_string(),
            });
        }
        let not_found = |reason: String| EngineError::RepoNotFound {
            path: path.to_owned(),
            reason,
        };
        // `git -C ""` resterebbe nella cartella corrente.
        if path.is_empty() {
            return Err(not_found("percorso vuoto".to_owned()));
        }
        let dir = Path::new(path);

        let args = [
            "rev-parse",
            "--is-bare-repository",
            "--absolute-git-dir",
            "--show-object-format",
        ];
        let out = self.git.output(Some(dir), &args)?;
        if !out.status.success() {
            return Err(not_found(stderr_text(&out)));
        }
        let lines: Vec<&[u8]> = out.stdout.split(|&b| b == b'\n').collect();
        let [bare, git_dir, format, b""] = lines[..] else {
            return Err(failure_text(&args, &out.stdout));
        };
        if bare == b"true" {
            return Err(not_found(
                "è un repository bare, senza cartella di lavoro: non supportato".to_owned(),
            ));
        }
        if format != b"sha1" {
            return Err(not_found(format!(
                "formato degli oggetti `{}` non supportato (solo sha1)",
                String::from_utf8_lossy(format)
            )));
        }
        let git_dir = path_from_output(git_dir);

        // Fallisce, ad esempio, dentro la cartella `.git`.
        let out = self
            .git
            .output(Some(dir), &["rev-parse", "--show-toplevel"])?;
        if !out.status.success() {
            return Err(not_found(stderr_text(&out)));
        }
        let workdir = path_from_output(&out.stdout);

        Ok(CliBackend {
            git: self.git.clone(),
            workdir,
            git_dir,
        })
    }
}

/// [`EngineError::Git`] per un comando riuscito ma con un output inatteso.
fn failure_text(args: &[&str], stdout: &[u8]) -> EngineError {
    EngineError::Git {
        command: args[0].to_owned(),
        stderr: format!(
            "output inatteso: `{}`",
            String::from_utf8_lossy(stdout).trim()
        ),
    }
}

impl RepoOpener for CliOpener {
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError> {
        Ok(Box::new(self.open_cli(path)?))
    }
}

/// Un repository letto con il `git` di sistema, creato da [`CliOpener`].
///
/// Ogni lettura esegue git nella radice del worktree e legge l'output con i parser del dominio.
#[derive(Debug, Clone)]
pub struct CliBackend {
    git: Git,
    workdir: PathBuf,
    git_dir: PathBuf,
}

impl CliBackend {
    /// Radice del worktree (`git rev-parse --show-toplevel`).
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    /// Cartella git di questo worktree (`git rev-parse --absolute-git-dir`).
    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    fn run(&self, args: &[&str]) -> Result<Vec<u8>, EngineError> {
        self.git.run(Some(&self.workdir), args)
    }

    /// Il commit a cui porta `rev`, `None` se non si risolve (anche se è ambigua).
    fn resolve(&self, rev: &str) -> Result<Option<Oid>, EngineError> {
        let spec = format!("{rev}^{{commit}}");
        let args = [
            "rev-parse",
            "--quiet",
            "--verify",
            "--end-of-options",
            &spec,
        ];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            let hex = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
            return Ok(Some(Oid::from_hex(hex)?));
        }
        // Con `--quiet` una rev sconosciuta non scrive niente; altro su stderr è un errore vero.
        if out.stderr.iter().all(u8::is_ascii_whitespace) {
            Ok(None)
        } else {
            Err(failure(&args, &out))
        }
    }

    /// Operazione in corso, dai file che git lascia nella sua cartella (come `git status`).
    fn operation(&self) -> Option<Operation> {
        let has = |name: &str| self.git_dir.join(name).exists();
        if has("MERGE_HEAD") {
            Some(Operation::Merge)
        } else if has("rebase-merge") || (has("rebase-apply") && !has("rebase-apply/applying")) {
            // `rebase-apply/applying` indica `git am`, che `Operation` non ha.
            Some(Operation::Rebase)
        } else if has("CHERRY_PICK_HEAD") {
            Some(Operation::CherryPick)
        } else if has("REVERT_HEAD") {
            Some(Operation::Revert)
        } else if has("BISECT_LOG") {
            Some(Operation::Bisect)
        } else {
            None
        }
    }
}

impl GitBackend for CliBackend {
    fn head(&self) -> Result<Head, EngineError> {
        let args = ["symbolic-ref", "--quiet", "HEAD"];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() {
            let full = out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout);
            let full = String::from_utf8_lossy(full);
            let name = full.strip_prefix("refs/heads/").unwrap_or(&full).to_owned();
            return Ok(Head::Branch {
                name,
                target: self.resolve("HEAD")?,
            });
        }
        // Codice 1 senza messaggi: HEAD non è simbolica, cioè staccata.
        if out.status.code() != Some(1) || !out.stderr.is_empty() {
            return Err(failure(&args, &out));
        }
        match self.resolve("HEAD")? {
            Some(oid) => Ok(Head::Detached(oid)),
            None => Err(EngineError::UnknownRev("HEAD".to_owned())),
        }
    }

    fn refs(&self) -> Result<Vec<RefInfo>, EngineError> {
        let mut args = vec!["for-each-ref"];
        args.extend_from_slice(REFS_ARGS);
        Ok(parse_refs(&self.run(&args)?)?)
    }

    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError> {
        let start = match &query.rev {
            Some(rev) => self
                .resolve(rev)?
                .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?,
            // Branch senza commit: log vuoto.
            None => match self.resolve("HEAD")? {
                Some(oid) => oid,
                None => return Ok(Vec::new()),
            },
        };
        if query.limit == Some(0) {
            return Ok(Vec::new());
        }
        let skip = format!("--skip={}", query.skip);
        let max_count = query.limit.map(|n| format!("--max-count={n}"));
        let start = start.to_string();

        let mut args = vec!["log"];
        args.extend_from_slice(LOG_ARGS);
        // Figli prima dei genitori, poi per data del committer, come `MemoryBackend`.
        args.extend(["--date-order", &skip]);
        args.extend(max_count.as_deref());
        args.extend(["--end-of-options", &start, "--"]);
        Ok(parse_log(&self.run(&args)?)?)
    }

    fn status(&self) -> Result<Status, EngineError> {
        // Una lettura non deve prendere il lock dell'indice per aggiornarlo.
        let mut args = vec!["--no-optional-locks", "status"];
        args.extend_from_slice(STATUS_ARGS);
        let mut status = parse_status(&self.run(&args)?)?;
        status.operation = self.operation();
        Ok(status)
    }
}
