//! [`CliBackend`] e [`CliOpener`]: lettura di un repository eseguendo `git`.

mod branch;
mod commit;
mod discard;
mod remote;
mod remote_admin;
mod restore;
mod stash;
mod sync;

use crate::GitVersion;
use crate::command::{Git, failure, os_from_bytes, path_from_output, stderr_text};
use project_domain::EntryKind;
use project_domain::{Change, Commit, CommitDetails, Diff, Head, RefInfo, RepoPath, Status};
use project_domain::{
    DIFF_ARGS, DIFF_TREE_ARGS, LOG_ARGS, Oid, Operation, PatchDirection, REFS_ARGS, STATUS_ARGS,
    parse_diff, parse_diff_tree, parse_log, parse_refs, parse_status,
};
use project_engine::{
    BACKUP_PREFIX, BranchDeleted, CommitOutcome, CommitRequest, ConfigEntry, DiffSpec,
    DiscardOutcome, DiscardedFile, EngineError, FetchOutcome, FetchRequest, GitBackend, HeadState,
    JobContext, LinePatch, LineSelection, LogQuery, PullOutcome, PullRequest, PullSettings,
    PushOutcome, PushRequest, RefRestore, RemoteSnapshot, RepoOpener, RestoreOutcome,
    StagedVersion, StashApplied, StashApply, StashDrop, StashDropped, StashEntry, StashPush,
    UpstreamConfig, WorktreeRestore, apply_patch, backup_message, line_patch, path_matches,
    versions_at_risk_hidden,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
        let empty_tree = empty_tree(format).ok_or_else(|| {
            not_found(format!(
                "formato degli oggetti `{}` sconosciuto",
                String::from_utf8_lossy(format)
            ))
        })?;
        let empty_blob = empty_blob(format).ok_or_else(|| {
            not_found(format!(
                "formato degli oggetti `{}` sconosciuto",
                String::from_utf8_lossy(format)
            ))
        })?;
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
            empty_tree,
            empty_blob,
        })
    }
}

/// Id dell'albero vuoto nel formato degli oggetti `format` (`rev-parse --show-object-format`):
/// git lo conosce sempre, anche se non è nel repo. Serve a confrontare un commit radice e a
/// togliere dallo stage su un branch senza commit. Per ora i parser supportano solo sha1, ma
/// l'id non va mai dato per scontato.
fn empty_tree(format: &[u8]) -> Option<&'static str> {
    match format {
        b"sha1" => Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904"),
        b"sha256" => Some("6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"),
        _ => None,
    }
}

/// Id del blob vuoto nel formato degli oggetti `format`: è l'id della voce di un file
/// `intent-to-add` (`git add -N`) nell'indice. Oggi `Oid` è solo SHA-1 e un repo SHA-256 non
/// si apre: il ramo sha256 non serve ancora, c'è per non dare per scontato il formato.
fn empty_blob(format: &[u8]) -> Option<&'static str> {
    match format {
        b"sha1" => Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"),
        b"sha256" => Some("473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813"),
        _ => None,
    }
}

/// Una voce di stadio 0 dell'indice, da `git ls-files -v -s`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexEntry {
    /// Lettera di `-v`: minuscola per `--assume-unchanged`, `S` per `--skip-worktree`.
    tag: u8,
    mode: u32,
    oid: Oid,
}

type IndexListing = BTreeMap<RepoPath, IndexEntry>;

/// Identità dei commit di backup dell'indice: non serve quella dell'utente, e così il backup
/// riesce anche se `user.name`/`user.email` non sono configurati.
const BACKUP_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Project"),
    ("GIT_AUTHOR_EMAIL", "backup@project.invalid"),
    ("GIT_COMMITTER_NAME", "Project"),
    ("GIT_COMMITTER_EMAIL", "backup@project.invalid"),
];

/// Un indice temporaneo nella cartella git, cancellato (con il suo lock) quando esce dallo
/// scope. Serve a costruire l'albero del backup senza toccare l'indice vero.
struct TempIndex(PathBuf);

impl TempIndex {
    fn new(git_dir: &Path) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self(git_dir.join(format!(
            "project-backup-index-{}-{nanos}",
            std::process::id()
        )))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
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
    /// Id dell'albero vuoto nel formato del repo.
    empty_tree: &'static str,
    /// Id del blob vuoto nel formato del repo (la voce di un file `intent-to-add`).
    empty_blob: &'static str,
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

    /// Vero se `path` esiste nel worktree (anche come link simbolico rotto o cartella).
    fn in_worktree(&self, path: &RepoPath) -> bool {
        std::fs::symlink_metadata(self.workdir.join(os_from_bytes(path.as_bytes()))).is_ok()
    }

    /// `paths` senza quelli che indicano solo cancellazioni già in stage: un file così non è
    /// più né nell'indice né nel worktree e `git add` lo rifiuterebbe ("did not match any
    /// files"), anche se non c'è niente da fare. Vale per un file (per esempio il vecchio
    /// percorso di una rinomina in stage) e per una cartella che contiene solo cancellazioni
    /// in stage.
    fn without_staged_deletions(&self, status: &Status, paths: &[RepoPath]) -> Vec<RepoPath> {
        // (percorso, è una cancellazione in stage) per ogni voce dello stato.
        let mut items: Vec<(&RepoPath, bool)> = Vec::new();
        for entry in &status.entries {
            match &entry.kind {
                EntryKind::Ordinary(_) => {
                    items.push((&entry.path, entry.index == Change::Deleted));
                }
                EntryKind::Renamed { original, .. } => {
                    items.push((original, true));
                    items.push((&entry.path, false));
                }
                _ => items.push((&entry.path, false)),
            }
        }
        paths
            .iter()
            .filter(|p| {
                let mut matched = items.iter().filter(|(path, _)| path_matches(path, p));
                let mut any = false;
                let only_deletions = matched.all(|(_, deleted)| {
                    any = true;
                    *deleted
                });
                !(any && only_deletions && !self.in_worktree(p))
            })
            .cloned()
            .collect()
    }

    /// Da dove `unstage` riprende l'indice: il commit di HEAD, oppure l'albero vuoto se HEAD
    /// è un branch che non ha ancora commit. Un HEAD che non si risolve per altri motivi (ref
    /// rotto, oggetto mancante) è un errore: trattarlo come "nessun commit" toglierebbe
    /// dall'indice tutti i file richiesti.
    fn index_source(&self) -> Result<String, EngineError> {
        if let Some(head) = self.resolve("HEAD")? {
            return Ok(head.to_string());
        }
        let out = self
            .git
            .output(Some(&self.workdir), &["symbolic-ref", "--quiet", "HEAD"])?;
        if out.status.success() {
            let name =
                String::from_utf8_lossy(out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout))
                    .into_owned();
            let exists = self
                .git
                .output(
                    Some(&self.workdir),
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "--end-of-options",
                        &name,
                    ],
                )?
                .status
                .success();
            if !exists {
                return Ok(self.empty_tree.to_owned());
            }
        }
        Err(EngineError::Git {
            command: "rev-parse".to_owned(),
            stderr: "HEAD non punta a un commit valido (né a un branch senza commit): \
                     niente tolto dallo stage"
                .to_owned(),
        })
    }

    /// Prima di riscrivere l'indice per `operation` sui `paths`: se ci sono versioni in stage
    /// che esistono solo nell'indice ([`versions_at_risk`]) le salva con [`Self::save_backup`]
    /// e restituisce il ref; `None` se non c'è niente a rischio.
    fn backup(
        &self,
        operation: &str,
        status: &Status,
        index: &IndexListing,
        paths: &[RepoPath],
    ) -> Result<Option<String>, EngineError> {
        // Con `--assume-unchanged` (lettera minuscola) o `--skip-worktree` (`S`) git non
        // guarda il worktree e `status` dà Y = `.` anche se il file è diverso.
        let hidden: BTreeSet<RepoPath> = index
            .iter()
            .filter(|(_, e)| e.tag.is_ascii_lowercase() || e.tag == b'S')
            .map(|(p, _)| p.clone())
            .collect();
        let versions = versions_at_risk_hidden(status, paths, &hidden);
        if versions.is_empty() {
            return Ok(None);
        }
        self.save_backup(operation, &versions).map(Some)
    }

    /// Salva `versions` come §7: un albero con quei file (costruito in un indice temporaneo,
    /// così funziona anche con conflitti nell'indice vero), un commit con genitore HEAD (se
    /// c'è) e un ref nuovo `refs/project/backup/index-<secondi>-<nanosecondi>`, creato solo se
    /// non esiste (`update-ref <ref> <id> ""`): un ref esistente non si sovrascrive mai. Il
    /// contenuto si recupera con `git show <ref>:<percorso>`.
    fn save_backup(
        &self,
        operation: &str,
        versions: &[StagedVersion],
    ) -> Result<String, EngineError> {
        let index = TempIndex::new(&self.git_dir);
        let index_env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        let mut info = Vec::new();
        for v in versions {
            info.extend_from_slice(format!("{:o} {}\t", v.mode, v.oid).as_bytes());
            info.extend_from_slice(v.path.as_bytes());
            info.push(0);
        }
        // L'indice temporaneo è semplice: niente split index (lascerebbe un `sharedindex.*` per
        // ogni backup), fsmonitor o indice sparso, qualunque sia la config del repo.
        let plain = [
            "-c",
            "core.splitIndex=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "index.sparse=false",
        ];
        let update = [&plain[..], &["update-index", "-z", "--index-info"]].concat();
        self.run_input(&update, &info, &index_env)?;
        let write = [&plain[..], &["write-tree"]].concat();
        let tree = self.run_input(&write, b"", &index_env)?;
        let tree = String::from_utf8_lossy(tree.trim_ascii_end()).into_owned();

        let message = backup_message(operation, versions);
        let head = self.resolve("HEAD")?.map(|h| h.to_string());
        let mut args = vec!["commit-tree", "--no-gpg-sign"];
        if let Some(head) = &head {
            args.extend(["-p", head]);
        }
        args.extend(["-F", "-", &tree]);
        let identity: Vec<(&str, &OsStr)> = BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        let commit = String::from_utf8_lossy(commit.trim_ascii_end()).into_owned();

        let reflog = format!("project: backup dell'indice prima di {operation}");
        self.create_backup_ref("index", &commit, &reflog)
    }

    /// Crea un ref nuovo `refs/project/backup/<kind>-<secondi>-<nanosecondi>` che punta a
    /// `commit`, solo se non esiste (`update-ref <ref> <id> ""`): un ref esistente non si
    /// sovrascrive mai; se il nome è preso si prova con un suffisso `-<n>`. Restituisce il nome.
    fn create_backup_ref(
        &self,
        kind: &str,
        commit: &str,
        reflog: &str,
    ) -> Result<String, EngineError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let base = format!(
            "{BACKUP_PREFIX}{kind}-{}-{:09}",
            now.as_secs(),
            now.subsec_nanos()
        );
        for attempt in 0..10 {
            let name = match attempt {
                0 => base.clone(),
                n => format!("{base}-{n}"),
            };
            let args = ["update-ref", "-m", reflog, &name, commit, ""];
            let out = self.git.output(Some(&self.workdir), &args)?;
            if out.status.success() {
                return Ok(name);
            }
            // Solo se il nome è già preso si prova il successivo.
            let taken = self
                .git
                .output(
                    Some(&self.workdir),
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        "--end-of-options",
                        &name,
                    ],
                )?
                .status
                .success();
            if !taken {
                return Err(failure(&args, &out));
            }
        }
        Err(EngineError::Git {
            command: "update-ref".to_owned(),
            stderr: format!("nessun nome libero per il backup dopo `{base}`"),
        })
    }

    /// Lo stato letto prima di una scrittura: come [`GitBackend::status`], ma senza
    /// `core.fsmonitor`, che se non è aggiornato fa sembrare invariato un file cambiato.
    fn status_for_write(&self) -> Result<Status, EngineError> {
        let mut args = vec![
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "status",
        ];
        args.extend_from_slice(STATUS_ARGS);
        let mut status = parse_status(&self.run(&args)?)?;
        status.operation = self.operation();
        Ok(status)
    }

    /// Le voci di stadio 0 dell'indice (`git ls-files -v -s -z`), per percorso.
    fn index_listing(&self) -> Result<IndexListing, EngineError> {
        let args = ["ls-files", "-v", "-s", "-z", "--"];
        let out = self.run(&args)?;
        let mut listing = BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<lettera> <modo> <id> <stadio>\t<percorso>`
            let bad = || failure_text(&args, record);
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let (head, path) = (&record[..tab], &record[tab + 1..]);
            let fields: Vec<&[u8]> = head.split(|&b| b == b' ').collect();
            let [[tag], mode, oid, stage] = fields[..] else {
                return Err(bad());
            };
            if stage != b"0" {
                continue;
            }
            let mode = std::str::from_utf8(mode)
                .ok()
                .and_then(|m| u32::from_str_radix(m, 8).ok())
                .ok_or_else(bad)?;
            listing.insert(
                RepoPath::from_bytes(path),
                IndexEntry {
                    tag: *tag,
                    mode,
                    oid: Oid::from_hex(oid)?,
                },
            );
        }
        Ok(listing)
    }

    /// Subito prima di scrivere: le voci dell'indice toccate da `paths` concordano con lo stato
    /// letto all'inizio e sono ancora quelle lette all'inizio (`before`, da cui vengono anche il controllo dei rischi e il backup).
    /// Se un altro processo git le ha cambiate nel frattempo, errore senza scrivere.
    fn check_index_unchanged(
        &self,
        status: &Status,
        before: &IndexListing,
        paths: &[RepoPath],
    ) -> Result<(), EngineError> {
        let now = self.index_listing()?;
        let touched = |listing: &IndexListing| -> Vec<(RepoPath, u32, Oid)> {
            listing
                .iter()
                .filter(|(p, _)| paths.iter().any(|w| path_matches(p, w)))
                .map(|(p, e)| (p.clone(), e.mode, e.oid))
                .collect()
        };
        // E le voci dello stato (lette ancora prima) concordano con l'indice letto dopo.
        let consistent = status.entries.iter().all(|entry| {
            let info = match &entry.kind {
                EntryKind::Ordinary(info)
                | EntryKind::Renamed { info, .. }
                | EntryKind::Copied { info, .. } => info,
                _ => return true,
            };
            if !paths.iter().any(|w| path_matches(&entry.path, w)) {
                return true;
            }
            match before.get(&entry.path) {
                // Un file `intent-to-add` (`.A`): `git status` dà modo e id zero, ma
                // nell'indice c'è una voce con il blob vuoto.
                Some(e) if entry.worktree == Change::Added && info.mode_index == 0 => {
                    self.is_intent_to_add(e)
                }
                Some(e) => info.mode_index == e.mode && info.oid_index == e.oid,
                None => info.mode_index == 0,
            }
        });
        if consistent && touched(before) == touched(&now) {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(
                "l'indice è cambiato mentre si preparava la scrittura (un altro processo git?): \
                 niente scritto, riprovare"
                    .to_owned(),
            ))
        }
    }

    /// [`EngineError::IndexLocked`] se l'indice è bloccato da un altro processo git (esiste
    /// `index.lock`). Solo un controllo anticipato, per non salvare un backup inutile prima di
    /// una scrittura che fallirebbe: la scrittura vera lo ricontrolla comunque (git non
    /// sovrascrive mai un lock).
    fn check_index_unlocked(&self) -> Result<(), EngineError> {
        let lock = self.git_dir.join("index.lock");
        if lock.symlink_metadata().is_ok() {
            return Err(EngineError::IndexLocked {
                lock: lock.to_string_lossy().into_owned(),
            });
        }
        Ok(())
    }

    /// Subito dopo aver letto il diff da cui viene la patch: la voce dell'indice che la patch
    /// cambia è quella su cui il diff è stato calcolato, cioè quella letta prima in `index`
    /// (da cui vengono anche il controllo dei rischi e il backup). Il lato indice del diff è il
    /// vecchio per lo stage (`git diff`), il nuovo per l'unstage (`git diff --cached`).
    fn check_diff_matches_index(
        &self,
        index: &IndexListing,
        prepared: &LinePatch,
        direction: PatchDirection,
    ) -> Result<(), EngineError> {
        let file = &prepared.file;
        let entry = index.get(prepared.target());
        let consistent = match direction {
            // Un file nuovo nel diff del worktree è un segnaposto `intent-to-add`: nell'indice
            // c'è, con il blob vuoto.
            PatchDirection::Stage if file.change == Change::Added => {
                entry.is_some_and(|e| self.is_intent_to_add(e))
            }
            PatchDirection::Stage => entry.map(|e| e.oid) == file.old_oid,
            // Un file eliminato in stage non è nell'indice.
            PatchDirection::Unstage if file.change == Change::Deleted => entry.is_none(),
            PatchDirection::Unstage => entry.map(|e| e.oid) == file.new_oid,
        };
        if consistent {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(
                "l'indice è cambiato mentre si preparava la scrittura (un altro processo git?): \
                 niente scritto, riprovare"
                    .to_owned(),
            ))
        }
    }

    /// Vero se la voce è quella di un file `intent-to-add`: il blob vuoto nel formato del repo
    /// (`git status` la riporta con modo e id zero e Y = `A`).
    fn is_intent_to_add(&self, entry: &IndexEntry) -> bool {
        entry.oid.to_string() == self.empty_blob
    }

    /// Il contenuto del blob `oid` (`git cat-file blob`, senza filtri).
    fn blob(&self, oid: &Oid) -> Result<Vec<u8>, EngineError> {
        self.run(&["cat-file", "blob", &oid.to_string()])
    }

    /// Salva `content` come blob (`hash-object -w --stdin --no-filters`: è già nella forma
    /// dell'indice, niente filtri `clean` né conversioni di fine riga) e ne restituisce l'id.
    /// Scrive solo nel database degli oggetti, non nell'indice.
    fn hash_blob(&self, content: &[u8]) -> Result<Oid, EngineError> {
        let args = ["hash-object", "-w", "--stdin", "--no-filters"];
        let out = self.run_input(&args, content, &[])?;
        Ok(Oid::from_hex(out.trim_ascii_end())?)
    }

    /// Scrive nell'indice la voce di `path`: `Some((modo, id))` la crea o la sostituisce,
    /// `None` la toglie (`update-index -z --index-info`, percorso su stdin). È l'unico passo
    /// che scrive l'indice. `--index-info` crea una voce nuova: i flag `--skip-worktree` e
    /// `--assume-unchanged` della vecchia si perdono e li rimette [`Self::restore_flags`].
    fn write_index_entry(
        &self,
        path: &RepoPath,
        entry: Option<(u32, Oid)>,
    ) -> Result<(), EngineError> {
        let mut info = match &entry {
            Some((mode, oid)) => format!("{mode:o} {oid}\t").into_bytes(),
            // Modo 0: `update-index --index-info` toglie la voce.
            None => format!("0 {}\t", Oid::from_bytes([0; Oid::LEN])).into_bytes(),
        };
        info.extend_from_slice(path.as_bytes());
        info.push(0);
        self.run_input(&["update-index", "-z", "--index-info"], &info, &[])?;
        Ok(())
    }

    /// Dopo una scrittura che ricrea le voci dell'indice (`update-index --index-info`,
    /// `restore --staged`), rimette i flag `--skip-worktree` e `--assume-unchanged` che le voci
    /// di `paths` avevano in `before` (lettere di `ls-files -v`: `S`/`s` skip-worktree,
    /// minuscola assume-unchanged) e che ora non hanno più. Senza, un file fuori dal cono di
    /// uno sparse checkout sembrerebbe eliminato dal worktree, e un `git add -A` o `commit -a`
    /// successivo metterebbe in stage una cancellazione mai fatta. Le voci che non sono più
    /// nell'indice si saltano. Se non riesce, errore: le voci scritte sono comunque corrette.
    fn restore_flags(&self, before: &IndexListing, paths: &[RepoPath]) -> Result<(), EngineError> {
        let wanted = |listing: &IndexListing| -> Vec<(RepoPath, bool, bool)> {
            listing
                .iter()
                .filter(|(p, _)| paths.iter().any(|w| path_matches(p, w)))
                .map(|(p, e)| {
                    (
                        p.clone(),
                        e.tag.eq_ignore_ascii_case(&b'S'),
                        e.tag.is_ascii_lowercase(),
                    )
                })
                .collect()
        };
        let flagged: Vec<_> = wanted(before)
            .into_iter()
            .filter(|(_, s, a)| *s || *a)
            .collect();
        if flagged.is_empty() {
            return Ok(());
        }
        let now = self.index_listing()?;
        let mut skip = Vec::new();
        let mut assume = Vec::new();
        for (path, s, a) in flagged {
            let Some(e) = now.get(&path) else { continue };
            if s && !e.tag.eq_ignore_ascii_case(&b'S') {
                skip.extend_from_slice(path.as_bytes());
                skip.push(0);
            }
            if a && !e.tag.is_ascii_lowercase() {
                assume.extend_from_slice(path.as_bytes());
                assume.push(0);
            }
        }
        for (flag, list) in [("--skip-worktree", skip), ("--assume-unchanged", assume)] {
            if list.is_empty() {
                continue;
            }
            let args = ["update-index", flag, "-z", "--stdin"];
            self.run_input(&args, &list, &[])
                .map_err(|e| EngineError::Git {
                    command: "update-index".to_owned(),
                    stderr: format!(
                        "l'indice è stato scritto correttamente, ma non si è riusciti a rimettere \
                     `{flag}` sui file che lo avevano (rimetterlo a mano con `git update-index \
                     {flag} <percorso>`): {e}"
                    ),
                })?;
        }
        Ok(())
    }

    /// `git <args>` con `input` su stdin e le variabili `env`; stdout, o l'errore di git.
    fn run_input(
        &self,
        args: &[&str],
        input: &[u8],
        env: &[(&str, &OsStr)],
    ) -> Result<Vec<u8>, EngineError> {
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), args, input, env)?;
        if out.status.success() {
            Ok(out.stdout)
        } else {
            Err(failure(args, &out))
        }
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
    ///
    /// `rebase-apply/applying` indica `git am` ([`Operation::Am`]). Un cherry-pick o un revert
    /// di più commit fermo fra un commit e l'altro (dopo `git commit` di una risoluzione, o con
    /// un commit vuoto) lascia `sequencer/` senza `CHERRY_PICK_HEAD`/`REVERT_HEAD`: è ancora in
    /// corso, e il tipo si legge dalla prima riga di `sequencer/todo` (`revert ...` →
    /// [`Operation::Revert`], altrimenti [`Operation::CherryPick`]).
    fn operation(&self) -> Option<Operation> {
        let has = |name: &str| self.git_dir.join(name).exists();
        if has("MERGE_HEAD") {
            Some(Operation::Merge)
        } else if has("rebase-apply/applying") {
            Some(Operation::Am)
        } else if has("rebase-merge") || has("rebase-apply") {
            Some(Operation::Rebase)
        } else if has("CHERRY_PICK_HEAD") {
            Some(Operation::CherryPick)
        } else if has("REVERT_HEAD") {
            Some(Operation::Revert)
        } else if has("sequencer") {
            Some(sequencer_operation(
                &std::fs::read(self.git_dir.join("sequencer/todo")).unwrap_or_default(),
            ))
        } else if has("BISECT_LOG") {
            Some(Operation::Bisect)
        } else {
            None
        }
    }
}

/// Il tipo di un sequencer fermo dal contenuto di `sequencer/todo`: la prima riga che non è
/// un commento dice `revert` o `pick`; senza righe (o illeggibile) cherry-pick.
fn sequencer_operation(todo: &[u8]) -> Operation {
    let first = todo
        .split(|&b| b == b'\n')
        .map(<[u8]>::trim_ascii)
        .find(|l| !l.is_empty() && !l.starts_with(b"#"));
    match first {
        Some(line) if line.starts_with(b"revert ") || line.starts_with(b"r ") => Operation::Revert,
        _ => Operation::CherryPick,
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
        query.check()?;
        let start = match &query.rev {
            Some(rev) => Some(
                self.resolve(rev)?
                    .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?,
            ),
            // Branch senza commit: log vuoto (con `all` restano gli altri ref).
            None => self.resolve("HEAD")?,
        };
        if query.limit == Some(0) || (start.is_none() && !query.all) {
            return Ok(Vec::new());
        }
        // Con `stashes` anche le basi degli stash (non gli stash stessi).
        let bases: Vec<String> = if query.stashes {
            self.stash_entries()?
                .iter()
                .map(|s| s.base.to_string())
                .collect()
        } else {
            Vec::new()
        };
        let skip = format!("--skip={}", query.skip);
        let max_count = query.limit.map(|n| format!("--max-count={n}"));
        let start = start.map(|oid| oid.to_string());

        let mut args = vec!["log"];
        args.extend_from_slice(LOG_ARGS);
        // Figli prima dei genitori, poi per data del committer, come `MemoryBackend`; vale anche
        // con più punti di partenza.
        args.extend(["--date-order", &skip]);
        args.extend(max_count.as_deref());
        if query.all {
            // Solo questi spazi: `refs/stash`, i backup in `refs/project/` e le note restano
            // fuori. Senza alcun ref il log è vuoto (git non ripiega su HEAD).
            args.extend(["--branches", "--remotes", "--tags"]);
        }
        // HEAD come id, così una HEAD staccata compare anche con `all`.
        args.push("--end-of-options");
        args.extend(start.as_deref());
        args.extend(bases.iter().map(String::as_str));
        args.push("--");
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
                    None => self.empty_tree.to_owned(),
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

    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        if paths.is_empty() {
            // `git add` senza percorsi non aggiunge niente, ma meglio non chiederlo affatto.
            return Ok(None);
        }
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        // Un file in conflitto messo in stage è risolto: solo se chiesto per nome, non perché
        // è dentro una cartella richiesta.
        let in_folder: Vec<&RepoPath> = status
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| &e.path)
            .filter(|u| !paths.contains(u) && paths.iter().any(|p| path_matches(u, p)))
            .collect();
        if !in_folder.is_empty() {
            return Err(EngineError::unmerged_in_folder(&in_folder));
        }
        let paths = self.without_staged_deletions(&status, paths);
        if paths.is_empty() {
            return Ok(None);
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
        // Le versioni in stage che resterebbero solo in un blob irraggiungibile: prima un
        // backup; se non riesce, l'indice non si tocca.
        let backup = self.backup("index.stage", &status, &index, &paths)?;
        self.check_index_unchanged(&status, &index, &paths)?;
        let out = self.output_with_paths(&add, &paths)?;
        if !out.status.success() {
            return Err(failure(&add, &out));
        }
        Ok(backup)
    }

    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        if paths.is_empty() {
            return Ok(None);
        }
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        // `restore --staged` su un file in conflitto toglierebbe senza dire niente gli stadi
        // del conflitto (le versioni base, nostra e loro).
        let conflicts: Vec<&RepoPath> = status
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| &e.path)
            .filter(|u| paths.iter().any(|p| path_matches(u, p)))
            .collect();
        if !conflicts.is_empty() {
            return Err(EngineError::unmerged(&conflicts));
        }
        // L'indice torna come in HEAD; senza commit come l'albero vuoto, cioè i file tornano
        // non tracciati (`restore` senza `--source` fallirebbe: HEAD non si risolve).
        // `--no-overlay` (già il default di `restore`): i file che in HEAD non ci sono escono
        // dall'indice. Senza `--worktree` il worktree non si tocca.
        let source = format!("--source={}", self.index_source()?);
        let backup = self.backup("index.unstage", &status, &index, paths)?;
        self.check_index_unchanged(&status, &index, paths)?;
        let args = ["restore", "--staged", "--no-overlay", &source];
        let out = self.output_with_paths(&args, paths)?;
        if !out.status.success() {
            return Err(failure(&args, &out));
        }
        // `restore --staged` ricrea le voci senza `--skip-worktree`/`--assume-unchanged`.
        self.restore_flags(&index, paths)?;
        Ok(backup)
    }

    fn commit(&mut self, request: &CommitRequest) -> Result<CommitOutcome, EngineError> {
        self.create_commit(request)
    }

    fn restore_refs(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        self.restore(request)
    }

    fn discard(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError> {
        self.discard_paths(paths, expected)
    }

    fn restore_worktree(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        self.restore_discarded(request)
    }

    fn remotes(&self) -> Result<Vec<String>, EngineError> {
        self.remote_names()
    }

    fn fetch(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
        self.fetch_remote(request, job)
    }

    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError> {
        self.count_unmerged(tip, into)
    }

    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError> {
        self.upstream_of(branch)
    }

    fn pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError> {
        self.read_pull_settings(branch)
    }

    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError> {
        self.config_all(key)
    }

    fn missing_commits(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError> {
        self.commits_missing(tip, exclude, limit)
    }

    fn pull(&mut self, request: &PullRequest) -> Result<PullOutcome, EngineError> {
        self.pull_upstream(request)
    }

    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        self.reset_keep(request)
    }

    fn push(
        &mut self,
        request: &PushRequest,
        job: &JobContext,
    ) -> Result<PushOutcome, EngineError> {
        self.push_branch(request, job)
    }

    fn create_branch(&mut self, name: &str, target: &Oid) -> Result<(), EngineError> {
        self.create_local_branch(name, target)
    }

    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError> {
        self.switch_to(target)
    }

    fn delete_branch(&mut self, name: &str, expected: &Oid) -> Result<BranchDeleted, EngineError> {
        self.delete_local_branch(name, expected)
    }

    fn rename_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError> {
        self.rename_local_branch(from, to, expected)
    }

    fn restore_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<String> {
        self.set_branch_config(name, entries, remove)
    }

    fn remote_snapshot(&self, name: &str) -> Result<RemoteSnapshot, EngineError> {
        self.snapshot_remote(name)
    }

    fn create_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError> {
        self.add_remote(name, url)
    }

    fn remove_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
        self.drop_remote_config(snapshot)
    }

    fn write_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
        self.put_remote_config(snapshot)
    }

    fn remote_config_elsewhere(&self, name: &str) -> Result<Vec<String>, EngineError> {
        self.remote_elsewhere(name)
    }

    fn stashes(&self) -> Result<Vec<StashEntry>, EngineError> {
        self.stash_entries()
    }

    fn stash_push(&mut self, request: &StashPush) -> Result<StashEntry, EngineError> {
        self.push_stash(request)
    }

    fn stash_apply(&mut self, request: &StashApply) -> Result<StashApplied, EngineError> {
        self.apply_stash(request)
    }

    fn stash_drop(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError> {
        self.drop_stash(request)
    }

    fn stash_store(&mut self, id: &Oid, message: &str) -> Result<(), EngineError> {
        self.store_stash(id, message)
    }

    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError> {
        let operation = selection.command_id();
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        // Il diff si legge dopo l'indice: se nel frattempo un altro git ha cambiato la voce,
        // `check_diff_matches_index` se ne accorge.
        let diff = self.diff(&selection.diff_spec())?;
        let prepared = line_patch(&diff, selection)?;
        self.check_diff_matches_index(&index, &prepared, selection.direction)?;

        // Il contenuto nuovo si calcola qui, con l'applicatore rigoroso, sul blob della voce
        // appena controllata: ogni hunk solo alla sua posizione, contesto byte per byte. Non
        // si usa `git apply`, che cerca il preimage anche altrove (con poco contesto potrebbe
        // mettere le righe nel posto sbagliato e riuscire lo stesso).
        let target = prepared.target().clone();
        let entry = index.get(&target);
        let base = match entry {
            Some(e) if !(prepared.patch.change == Change::Added && self.is_intent_to_add(e)) => {
                Some(self.blob(&e.oid)?)
            }
            _ => None,
        };
        let content = apply_patch(base.as_deref(), &prepared.patch).map_err(|e| {
            EngineError::NotAllowed(format!(
                "la patch non si applica alla versione in stage di `{target}` ({e}): niente \
                 scritto, rileggere il diff"
            ))
        })?;
        // Il modo resta quello dell'indice; per un file che nell'indice non c'era (o era solo
        // `intent-to-add`) quello del lato nuovo della patch.
        let mode = match entry {
            Some(e) if base.is_some() => e.mode,
            _ => prepared
                .patch
                .new_mode
                .or(entry.map(|e| e.mode))
                .unwrap_or(0o100644),
        };

        self.check_index_unlocked()?;
        // Il blob si salva prima del backup e del ricontrollo: fra il ricontrollo e la
        // scrittura resta solo `update-index`. Con la CLI di git non c'è un vero
        // compare-and-swap sull'indice: rimane l'istante fra quel ricontrollo e il lock che
        // prende `update-index`.
        let written = match &content {
            Some(c) => Some((mode, self.hash_blob(c)?)),
            None => None,
        };
        let touched = prepared.touched(selection);
        let backup = self.backup(operation, &status, &index, &touched)?;
        self.check_index_unchanged(&status, &index, &touched)?;
        self.write_index_entry(&target, written)?;
        self.restore_flags(&index, std::slice::from_ref(&target))?;

        // La voce nell'indice è quella scritta (un altro git potrebbe averla cambiata subito
        // dopo: in quel caso lo si dice, il backup resta).
        let now = self.index_listing()?;
        if now.get(&target).map(|e| (e.mode, e.oid)) != written {
            return Err(EngineError::Git {
                command: "update-index".to_owned(),
                stderr: format!(
                    "dopo la scrittura la voce di `{target}` nell'indice non è quella attesa \
                     (un altro processo git?){}",
                    backup
                        .as_ref()
                        .map(|b| format!("; la versione in stage precedente è in `{b}`"))
                        .unwrap_or_default()
                ),
            });
        }
        Ok(backup)
    }
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
    fn sequencer_kind_from_todo() {
        assert_eq!(
            sequencer_operation(b"revert 1234567 x\npick 89abcde y\n"),
            Operation::Revert
        );
        assert_eq!(
            sequencer_operation(b"# c\npick 1234567 x\n"),
            Operation::CherryPick
        );
        assert_eq!(sequencer_operation(b""), Operation::CherryPick);
    }

    #[test]
    fn empty_blob_per_object_format() {
        // `git hash-object -t blob /dev/null`, la voce di un file `intent-to-add`.
        assert_eq!(
            empty_blob(b"sha1"),
            Some("e69de29bb2d1d6434b8b29ae775ad8c2e48c5391")
        );
        assert_eq!(empty_blob(b"sha256").map(str::len), Some(64));
        assert_eq!(empty_blob(b"md5"), None);
    }

    #[test]
    fn empty_tree_per_object_format() {
        assert_eq!(
            empty_tree(b"sha1"),
            Some("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
        );
        assert_eq!(empty_tree(b"sha256").map(str::len), Some(64));
        assert_eq!(empty_tree(b"md5"), None);
    }
}
