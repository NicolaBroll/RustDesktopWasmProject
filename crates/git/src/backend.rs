//! [`CliBackend`] e [`CliOpener`]: lettura di un repository eseguendo `git`.

use crate::GitVersion;
use crate::command::{Git, failure, os_from_bytes, path_from_output, stderr_text};
use project_domain::{Change, Commit, CommitDetails, Diff, Head, RefInfo, RepoPath, Status};
use project_domain::{
    DIFF_ARGS, DIFF_TREE_ARGS, LOG_ARGS, Oid, Operation, REFS_ARGS, STATUS_ARGS, parse_diff,
    parse_diff_tree, parse_log, parse_refs, parse_status,
};
use project_engine::{DiffSpec, EngineError, GitBackend, LogQuery, RepoOpener};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
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

/// Id dell'albero vuoto in sha1 (l'unico formato supportato): git lo conosce sempre, anche se
/// non è nel repo. Serve a confrontare un commit radice.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

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

    /// Il primo genitore del commit `id`, `None` per un commit radice.
    fn first_parent(&self, id: &str) -> Result<Option<Oid>, EngineError> {
        // Una riga `<commit> <genitori>...`.
        let args = [
            "rev-list",
            "--max-count=1",
            "--parents",
            "--end-of-options",
            id,
            "--",
        ];
        let out = self.run(&args)?;
        let mut ids = out
            .split(|b| b.is_ascii_whitespace())
            .filter(|t| !t.is_empty());
        ids.next();
        match ids.next() {
            Some(hex) => Ok(Some(Oid::from_hex(hex)?)),
            None => Ok(None),
        }
    }

    /// `git diff` con `compare` (cosa confrontare, già con `--end-of-options` se servono
    /// revisioni) e `extra` (opzioni), limitato a `paths`, presi alla lettera: niente glob né
    /// pathspec magici (`--literal-pathspecs`). Una lettura: non aggiorna l'indice. Per
    /// `git diff` non basta `--no-optional-locks`: con un file cambiato solo nei dati di
    /// `stat` (es. riscritto uguale) riscrive l'indice per aggiornarli; lo impedisce
    /// `diff.autoRefreshIndex=false`, e il file resta comunque fuori dal diff.
    fn run_diff(
        &self,
        extra: &[&str],
        compare: &[&str],
        paths: &[&RepoPath],
    ) -> Result<Vec<u8>, EngineError> {
        let mut args = vec![
            "--no-optional-locks",
            "-c",
            "diff.autoRefreshIndex=false",
            "--literal-pathspecs",
            "diff",
        ];
        args.extend_from_slice(extra);
        args.extend_from_slice(compare);
        args.push("--");
        let paths: Vec<OsString> = paths.iter().map(|p| os_from_bytes(p.as_bytes())).collect();
        let paths: Vec<&OsStr> = paths.iter().map(OsString::as_os_str).collect();
        self.git.run_with_paths(Some(&self.workdir), &args, &paths)
    }

    /// I percorsi da passare a `git diff` per il file `path`: anche l'altro percorso se il
    /// file è rinominato (da o verso `path`), altrimenti git, limitato a un solo percorso, non
    /// vedrebbe la rinomina ma un file aggiunto o eliminato.
    fn diff_paths(&self, compare: &[&str], path: &RepoPath) -> Result<Vec<RepoPath>, EngineError> {
        // Solo le rinomine (`--diff-filter=R`), nel formato di `diff-tree`.
        let raw = [
            "-z",
            "--raw",
            "--no-abbrev",
            "--find-renames",
            "--diff-filter=R",
        ];
        let renames = parse_diff_tree(&self.run_diff(&raw, compare, &[])?)?;
        let mut paths = vec![path.clone()];
        for file in renames {
            if file.change != Change::Renamed {
                continue;
            }
            let old = file.old_path().clone();
            if file.path == *path {
                paths.push(old);
            } else if old == *path {
                paths.push(file.path);
            }
        }
        Ok(paths)
    }

    /// I file la cui cancellazione è in stage (in HEAD, non nell'indice), senza rinomine.
    fn staged_deletions(&self) -> Result<BTreeSet<RepoPath>, EngineError> {
        // Su un branch senza commit il confronto è con l'albero vuoto: nessuna cancellazione.
        let args = [
            "--no-optional-locks",
            "diff",
            "--cached",
            "--no-renames",
            "--no-relative",
            "--name-only",
            "-z",
            "--diff-filter=D",
            "--",
        ];
        Ok(nul_separated(&self.run(&args)?).collect())
    }

    /// I file in conflitto (con stadi 1–3 nell'indice), da `git ls-files --unmerged`.
    fn unmerged_paths(&self) -> Result<BTreeSet<RepoPath>, EngineError> {
        // Un record `<modo> <id> <stadio>\t<percorso>` per stadio.
        let out = self.run(&["ls-files", "-z", "--unmerged", "--"])?;
        nul_separated(&out)
            .map(|record| {
                let bytes = record.as_bytes();
                match bytes.iter().position(|&b| b == b'\t') {
                    Some(tab) => Ok(RepoPath::from_bytes(&bytes[tab + 1..])),
                    None => Err(failure_text(&["ls-files"], bytes)),
                }
            })
            .collect()
    }

    /// Esegue `git <args>` con `paths` letti da stdin (`--pathspec-from-file=-
    /// --pathspec-file-nul`, presi alla lettera da `--literal-pathspecs`): nessun limite di
    /// lunghezza della riga di comando e nessun percorso che git possa leggere come opzione.
    fn output_with_paths(
        &self,
        args: &[&str],
        paths: &[RepoPath],
    ) -> Result<std::process::Output, EngineError> {
        debug_assert!(
            !paths.is_empty(),
            "senza percorsi git lavorerebbe su tutto il repo"
        );
        let mut full = vec!["--literal-pathspecs"];
        full.extend_from_slice(args);
        full.extend(["--pathspec-from-file=-", "--pathspec-file-nul"]);
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        self.git
            .output_with_input(Some(&self.workdir), &full, &input)
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

    fn show_commit(&self, rev: &str) -> Result<CommitDetails, EngineError> {
        let id = self
            .resolve(rev)?
            .ok_or_else(|| EngineError::UnknownRev(rev.to_owned()))?;
        let hex = id.to_string();

        let mut args = vec!["log"];
        args.extend_from_slice(LOG_ARGS);
        args.extend(["--max-count=1", "--end-of-options", &hex, "--"]);
        let commit = parse_log(&self.run(&args)?)?
            .into_iter()
            .next()
            .ok_or_else(|| EngineError::Git {
                command: "log".to_owned(),
                stderr: format!("nessun commit letto per `{hex}`"),
            })?;

        // Confronto con il primo genitore, anche per i merge (senza opzioni `diff-tree` non
        // mostrerebbe niente per un merge); un commit radice si confronta con l'albero vuoto.
        let compared_to = commit.parents.first().copied();
        let parent = compared_to.map(|p| p.to_string());
        let mut args = vec!["diff-tree"];
        args.extend_from_slice(DIFF_TREE_ARGS);
        match &parent {
            Some(parent) => args.extend(["--end-of-options", parent, &hex]),
            None => args.extend(["--root", "--end-of-options", &hex]),
        }
        args.push("--");
        let files = parse_diff_tree(&self.run(&args)?)?;
        Ok(CommitDetails {
            commit,
            compared_to,
            files,
        })
    }

    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError> {
        let (parent, id);
        let compare: Vec<&str> = match spec {
            DiffSpec::Worktree { .. } => Vec::new(),
            // Su un branch senza commit git confronta l'indice con l'albero vuoto.
            DiffSpec::Index { .. } => vec!["--cached"],
            DiffSpec::Commit { rev, .. } => {
                id = self
                    .resolve(rev)?
                    .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?
                    .to_string();
                // Primo genitore anche per i merge, come `show_commit`.
                parent = match self.first_parent(&id)? {
                    Some(p) => p.to_string(),
                    None => EMPTY_TREE.to_owned(),
                };
                vec!["--end-of-options", &parent, &id]
            }
        };
        let Some(path) = spec.path() else {
            return Ok(parse_diff(&self.run_diff(DIFF_ARGS, &compare, &[])?)?);
        };
        let paths = self.diff_paths(&compare, path)?;
        let paths: Vec<&RepoPath> = paths.iter().collect();
        let mut diff = parse_diff(&self.run_diff(DIFF_ARGS, &compare, &paths)?)?;
        // Un percorso letterale comprende anche i file sotto una cartella con quel nome (es. se
        // `path` era un file e ora è una cartella): solo il file con quel percorso esatto.
        diff.files
            .retain(|f| f.new_path == *path || f.old_path == *path);
        Ok(diff)
    }

    fn stage(&mut self, paths: &[RepoPath]) -> Result<(), EngineError> {
        if paths.is_empty() {
            // `git add` senza percorsi non aggiunge niente, ma meglio non chiederlo affatto.
            return Ok(());
        }
        // Un file la cui cancellazione è già in stage non è più né nell'indice né nel
        // worktree: `git add` lo rifiuterebbe ("did not match any files"). Non c'è niente da
        // fare: si salta (succede chiedendo di nuovo i due percorsi di una rinomina in stage).
        let deleted = self.staged_deletions()?;
        let paths: Vec<RepoPath> = paths
            .iter()
            .filter(|p| {
                !(deleted.contains(*p)
                    && std::fs::symlink_metadata(self.workdir.join(os_from_bytes(p.as_bytes())))
                        .is_err())
            })
            .cloned()
            .collect();
        if paths.is_empty() {
            return Ok(());
        }
        // `--no-ignore-errors`: un file illeggibile ferma tutto (la config `add.ignoreErrors`
        // farebbe continuare con gli altri).
        let add = ["add", "--no-ignore-errors"];
        // Prima a vuoto: con un file ignorato `git add` aggiunge gli altri e poi fallisce,
        // con un percorso inesistente fallisce prima di aggiungere. La prova a vuoto fa gli
        // stessi controlli senza toccare l'indice, così o va tutto o non cambia niente.
        let dry = [&add[..], &["--dry-run"]].concat();
        let out = self.output_with_paths(&dry, &paths)?;
        if !out.status.success() {
            let stderr = stderr_text(&out);
            return Err(match ignored_paths(&stderr) {
                Some(ignored) => EngineError::ignored(&ignored),
                None => failure(&dry, &out),
            });
        }
        let out = self.output_with_paths(&add, &paths)?;
        if !out.status.success() {
            return Err(failure(&add, &out));
        }
        Ok(())
    }

    fn unstage(&mut self, paths: &[RepoPath]) -> Result<(), EngineError> {
        if paths.is_empty() {
            return Ok(());
        }
        // `restore --staged` su un file in conflitto toglierebbe senza dire niente gli stadi
        // del conflitto (le versioni base, nostra e loro).
        let conflicts: Vec<RepoPath> = self
            .unmerged_paths()?
            .into_iter()
            .filter(|u| paths.iter().any(|p| under(u, p)))
            .collect();
        if !conflicts.is_empty() {
            return Err(EngineError::unmerged(&conflicts));
        }
        // L'indice torna come in HEAD; senza commit come l'albero vuoto, cioè i file tornano
        // non tracciati (`restore` senza `--source` fallirebbe: HEAD non si risolve).
        // `--no-overlay` (già il default di `restore`): i file che in HEAD non ci sono escono
        // dall'indice. Senza `--worktree` il worktree non si tocca.
        let source = match self.resolve("HEAD")? {
            Some(head) => head.to_string(),
            None => EMPTY_TREE.to_owned(),
        };
        let source = format!("--source={source}");
        let args = ["restore", "--staged", "--no-overlay", &source];
        let out = self.output_with_paths(&args, paths)?;
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        Ok(())
    }
}

/// I percorsi separati da NUL nell'output di git (`-z`), senza il NUL finale.
fn nul_separated(out: &[u8]) -> impl Iterator<Item = RepoPath> + '_ {
    out.split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(RepoPath::from_bytes)
}

/// Vero se `path` è `wanted` o un file sotto la cartella `wanted` (come un pathspec letterale).
fn under(path: &RepoPath, wanted: &RepoPath) -> bool {
    let (path, wanted) = (path.as_bytes(), wanted.as_bytes());
    path == wanted || (path.starts_with(wanted) && path.get(wanted.len()) == Some(&b'/'))
}

/// I percorsi elencati da `git add` quando rifiuta dei file ignorati (con `LC_ALL=C`):
///
/// ```text
/// The following paths are ignored by one of your .gitignore files:
/// <percorso>
/// hint: Use -f if you really want to add them.
/// ```
///
/// `None` se stderr non contiene quel messaggio.
fn ignored_paths(stderr: &str) -> Option<Vec<String>> {
    const HEADER: &str = "The following paths are ignored by one of your .gitignore files:";
    let start = stderr.find(HEADER)? + HEADER.len();
    let paths: Vec<String> = stderr[start..]
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .take_while(|l| !l.starts_with("hint:") && !l.starts_with("fatal:"))
        .map(str::to_owned)
        .collect();
    Some(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_paths_from_git_add() {
        let stderr = "The following paths are ignored by one of your .gitignore files:\n\
                      debug.log\n\
                      dir/x.log\n\
                      hint: Use -f if you really want to add them.\n\
                      hint: Turn this message off by running";
        assert_eq!(
            ignored_paths(stderr),
            Some(vec!["debug.log".to_owned(), "dir/x.log".to_owned()])
        );
        assert_eq!(
            ignored_paths("fatal: pathspec 'x' did not match any files"),
            None
        );
    }

    #[test]
    fn under_matches_files_and_folders() {
        let p = |s: &str| RepoPath::from(s);
        assert!(under(&p("a/b.txt"), &p("a")));
        assert!(under(&p("a"), &p("a")));
        assert!(!under(&p("ab/c"), &p("a")));
        assert!(!under(&p("a"), &p("a/b")));
    }
}
