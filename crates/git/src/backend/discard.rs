//! `worktree.discard` e il suo undo/redo sul [`CliBackend`] (passi e garanzie
//! in `impl-docs/git/discard.md`).
//!
//! Scarto, in quest'ordine, e niente si scrive nel worktree se un passo fallisce:
//!
//! 1. stato senza `core.fsmonitor`, [`discard_plan`] e confronto con il piano confermato;
//!    l'indice (`ls-files -s`) deve avere le voci dello stato;
//! 2. ogni cartella del percorso di ogni file è una cartella vera e il file non è una cartella
//!    (un non tracciato, ignorato o link al suo posto verrebbe cancellato da `git restore`);
//! 3. `index.lock` → [`EngineError::IndexLocked`] subito;
//! 4. `git stash create` (senza fsmonitor, identità fissa dei backup);
//! 5. i file i cui byte non sono quelli che `git restore` ricreerebbe dal commit (filtri,
//!    `core.autocrlf=input`) si salvano senza filtri in un commit figlio (`raw`);
//! 6. il ref `refs/project/backup/discard-...` (su `raw` se c'è, altrimenti sullo stash);
//! 7. ricontrollo di indice e cartelle, e **ultima verifica** che il backup abbia esattamente
//!    il worktree attuale (modo con `git diff <stash>`, byte come al passo 5);
//! 8. `git restore --worktree --no-overlay` dei soli file del piano (percorsi su stdin).
//!
//! Undo: HEAD (controllata dal diario), voce dell'indice registrata e worktree uguale
//! all'indice; poi `git restore --source=<stash>` e i file di `raw` riscritti con i loro byte.
//! Redo: voce dell'indice e worktree uguale al backup, poi di nuovo `restore --worktree`. Dopo
//! la scrittura si rilegge: un file non come atteso va in `warnings`.

use super::CliBackend;
use crate::command::{failure, os_from_bytes, stderr_text};
use project_domain::{EntryKind, Oid, RepoPath};
use project_engine::{
    DiscardOutcome, DiscardedFile, EngineError, RestoreOutcome, WorktreeRestore, check_same_plan,
    discard_plan,
};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// I byte di un percorso del sistema (la destinazione di un link), come li salva git.
#[cfg(unix)]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_bytes(s: &OsStr) -> Vec<u8> {
    s.to_string_lossy().replace('\\', "/").into_bytes()
}

/// Messaggio del commit di `git stash create`: fisso, nessun dato dell'utente.
const STASH_MESSAGE: &str = "project: backup del worktree prima di worktree.discard";

impl CliBackend {
    /// [`GitBackend::discard`](project_engine::GitBackend::discard).
    pub(super) fn discard_paths(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError> {
        let status = self.status_for_write()?;
        let files = discard_plan(&status, paths)?;
        check_same_plan(&files, expected)?;
        let targets: Vec<RepoPath> = files.iter().map(|f| f.path.clone()).collect();
        self.check_discard_index(&files)?;
        self.check_real_dirs(&targets)?;
        self.check_index_unlocked()?;

        let stash = self.stash_create()?;
        let raw = self.save_raw(&stash, &targets)?;
        let backup = self.create_backup_ref(
            "discard",
            &raw.unwrap_or(stash).to_string(),
            "project: backup del worktree prima di worktree.discard",
        )?;
        self.check_discard_index(&files)?;
        // Ultimo passo prima di scrivere: il backup ha esattamente quello che c'è ora.
        self.check_real_dirs(&targets)?;
        let missing = self.not_as_in_backup(&stash, raw.as_ref(), &targets)?;
        if !missing.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "il backup del worktree (`{backup}`) non ha il contenuto attuale di {} (un file \
                 cambiato proprio adesso?): niente scartato, riprovare",
                list(&missing)
            )));
        }
        let args = ["restore", "--worktree", "--no-overlay"];
        let out = self.output_with_paths(&args, &targets)?;
        if !out.status.success() {
            return Err(with_backup(failure(&args, &out), &backup));
        }
        Ok(DiscardOutcome {
            files,
            backup,
            stash,
            raw,
        })
    }

    /// [`GitBackend::restore_worktree`](project_engine::GitBackend::restore_worktree).
    pub(super) fn restore_discarded(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        let paths = request.paths();
        if paths.is_empty() {
            return Ok(RestoreOutcome::default());
        }
        let status = self.status_for_write()?;
        let index = self.index_listing()?;
        let mut conflicts = request.index_conflicts(|p| index.get(p).map(|e| (e.mode, e.oid)));
        // Prima di leggere i file: un file al posto di una cartella dà un rifiuto chiaro.
        self.check_real_dirs(&paths)?;
        let changed: Vec<RepoPath> = if request.redo {
            // Come l'ha lasciato l'undo: i byte del backup.
            self.not_as_in_backup(&request.stash, request.raw.as_ref(), &paths)?
        } else {
            paths
                .iter()
                .filter(|p| {
                    let hidden = index
                        .get(*p)
                        .is_some_and(|e| e.tag.is_ascii_lowercase() || e.tag == b'S');
                    hidden
                        || status.entries.iter().any(|e| {
                            e.path == **p
                                && (e.worktree.is_change()
                                    || !matches!(
                                        e.kind,
                                        EntryKind::Ordinary(_)
                                            | EntryKind::Renamed { .. }
                                            | EntryKind::Copied { .. }
                                    ))
                        })
                })
                .cloned()
                .collect()
        };
        conflicts.extend(changed.iter().map(|p| request.worktree_conflict(p)));
        request.conflict_error(conflicts)?;
        self.check_index_unlocked()?;

        let source = format!("--source={}", request.stash);
        let args: Vec<&str> = if request.redo {
            vec!["restore", "--worktree", "--no-overlay"]
        } else {
            vec!["restore", &source, "--worktree", "--no-overlay"]
        };
        let kept = || {
            format!(
                "il worktree prima dello scarto resta nel commit {}",
                request.raw.unwrap_or(request.stash)
            )
        };
        let out = self.output_with_paths(&args, &paths)?;
        if !out.status.success() {
            return Err(EngineError::Git {
                command: "restore".to_owned(),
                stderr: format!("{}; {}", stderr_text(&out), kept()),
            });
        }
        if !request.redo
            && let Some(raw) = &request.raw
        {
            // I file salvati byte per byte: si riscrivono quei byte, senza filtri.
            for (path, entry) in self.ls_tree(raw, &paths)? {
                let content = self.blob(&entry.1)?;
                std::fs::write(self.worktree_path(&path), content).map_err(|e| {
                    EngineError::Git {
                        command: "restore".to_owned(),
                        stderr: format!("`{path}` non si riscrive ({e}); {}", kept()),
                    }
                })?;
            }
        }

        // Si rilegge: i file sono come chiesto?
        let wrong = if request.redo {
            self.differing(None, &paths)?
        } else {
            self.not_as_in_backup(&request.stash, request.raw.as_ref(), &paths)?
        };
        let warnings = if wrong.is_empty() {
            Vec::new()
        } else {
            vec![format!(
                "dopo `{}` {} non {} come atteso (un filtro `clean`/`smudge` o un altro \
                 programma?); {}",
                request.command,
                list(&wrong),
                if wrong.len() == 1 { "è" } else { "sono" },
                kept()
            )]
        };
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    fn worktree_path(&self, path: &RepoPath) -> PathBuf {
        self.workdir.join(os_from_bytes(path.as_bytes()))
    }

    /// Ogni cartella del percorso di ogni file è una cartella vera, e il file non è una
    /// cartella. Al posto di una cartella può esserci un file o un link non tracciato (o
    /// ignorato: `git status` non lo dice), e `git restore` lo cancellerebbe per ricreare il
    /// file: niente si scrive.
    fn check_real_dirs(&self, paths: &[RepoPath]) -> Result<(), EngineError> {
        let mut bad = Vec::new();
        for path in paths {
            let bytes = path.as_bytes();
            let mut blocked = false;
            for (i, _) in bytes.iter().enumerate().filter(|(_, b)| **b == b'/') {
                let dir = RepoPath::from_bytes(&bytes[..i]);
                match std::fs::symlink_metadata(self.worktree_path(&dir)) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => {
                        bad.push(dir);
                        blocked = true;
                        break;
                    }
                    // La cartella non c'è: git la crea, niente da perdere più in giù.
                    Err(_) => {
                        blocked = true;
                        break;
                    }
                }
            }
            if !blocked
                && std::fs::symlink_metadata(self.worktree_path(path)).is_ok_and(|m| m.is_dir())
            {
                bad.push(path.clone());
            }
        }
        if bad.is_empty() {
            return Ok(());
        }
        bad.sort();
        bad.dedup();
        Err(EngineError::NotAllowed(format!(
            "nel worktree {} non {} come se lo aspetta git (un file o un link al posto di una \
             cartella, o una cartella al posto di un file): ricreare i file lo cancellerebbe, e \
             il backup non lo contiene. Niente scritto: spostarlo e ripetere",
            list(&bad),
            if bad.len() == 1 { "è" } else { "sono" },
        )))
    }

    /// I file di `paths` il cui worktree non è quello del backup: il contenuto che `git
    /// restore --source=<stash>` ricreerebbe (con i filtri `smudge` e le conversioni di fine
    /// riga), o i byte salvati in `raw`, e lo stesso modo (`git diff <stash>`). Un file che nel
    /// backup non c'è non deve esserci.
    fn not_as_in_backup(
        &self,
        stash: &Oid,
        raw: Option<&Oid>,
        paths: &[RepoPath],
    ) -> Result<Vec<RepoPath>, EngineError> {
        let mut out = self.differing(Some(stash), paths)?;
        let in_stash = self.ls_tree(stash, paths)?;
        let in_raw = match raw {
            Some(raw) => self.ls_tree(raw, paths)?,
            None => BTreeMap::new(),
        };
        for path in paths {
            let expected = match in_raw.get(path) {
                Some((_, oid)) => Some((false, self.blob(oid)?)),
                None => match in_stash.get(path) {
                    Some(entry) => Some(self.checkout_bytes(path, entry)?),
                    None => None,
                },
            };
            if Self::disk(&self.worktree_path(path))? != expected && !out.contains(path) {
                out.push(path.clone());
            }
        }
        out.sort();
        Ok(out)
    }

    /// Cosa c'è nel worktree in `path`: `None` se niente, `(true, destinazione)` per un link,
    /// `(false, byte)` per un file. Una cartella conta come un contenuto che non torna mai.
    fn disk(path: &Path) -> Result<Option<(bool, Vec<u8>)>, EngineError> {
        let io = |e: std::io::Error| EngineError::Git {
            command: "restore".to_owned(),
            stderr: format!("`{}` non si legge: {e}", path.display()),
        };
        let meta = match std::fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(path).map_err(io)?;
            Ok(Some((true, os_bytes(target.as_os_str()))))
        } else if meta.is_dir() {
            Ok(Some((true, b"/".to_vec())))
        } else {
            Ok(Some((false, std::fs::read(path).map_err(io)?)))
        }
    }

    /// I byte che `git restore` scriverebbe per la voce `entry` (modo, blob) in `path`: un link
    /// ha per destinazione il contenuto del blob, un file passa dai filtri
    /// (`cat-file --filters --path=<path>`, gli attributi di quel percorso).
    fn checkout_bytes(
        &self,
        path: &RepoPath,
        entry: &(u32, Oid),
    ) -> Result<(bool, Vec<u8>), EngineError> {
        let (mode, oid) = entry;
        if mode & 0o170000 == 0o120000 {
            return Ok((true, self.blob(oid)?));
        }
        let mut arg = OsString::from("--path=");
        arg.push(os_from_bytes(path.as_bytes()));
        let hex = oid.to_string();
        let out = self.git.output_os(
            Some(&self.workdir),
            &[
                OsStr::new("cat-file"),
                OsStr::new("--filters"),
                &arg,
                OsStr::new(&hex),
            ],
        )?;
        if !out.status.success() {
            return Err(failure(&["cat-file"], &out));
        }
        Ok((false, out.stdout))
    }

    /// Le voci (modo, id) di `paths` nell'albero del commit `commit` (`ls-tree -r -z
    /// --full-tree`, percorsi alla lettera); un percorso che non c'è manca.
    fn ls_tree(
        &self,
        commit: &Oid,
        paths: &[RepoPath],
    ) -> Result<BTreeMap<RepoPath, (u32, Oid)>, EngineError> {
        let commit = commit.to_string();
        let args = [
            "--literal-pathspecs",
            "ls-tree",
            "-r",
            "-z",
            "--full-tree",
            "--end-of-options",
            &commit,
            "--",
        ];
        let os: Vec<OsString> = paths.iter().map(|p| os_from_bytes(p.as_bytes())).collect();
        let os: Vec<&OsStr> = os.iter().map(OsString::as_os_str).collect();
        let out = self.git.run_with_paths(Some(&self.workdir), &args, &os)?;
        let mut map = BTreeMap::new();
        for record in out.split(|&b| b == 0).filter(|r| !r.is_empty()) {
            // `<modo> <tipo> <id>\t<percorso>`
            let bad = || super::failure_text(&args[1..], record);
            let tab = record.iter().position(|&b| b == b'\t').ok_or_else(bad)?;
            let fields: Vec<&[u8]> = record[..tab].split(|&b| b == b' ').collect();
            let [mode, _, oid] = fields[..] else {
                return Err(bad());
            };
            let mode = std::str::from_utf8(mode)
                .ok()
                .and_then(|m| u32::from_str_radix(m, 8).ok())
                .ok_or_else(bad)?;
            map.insert(
                RepoPath::from_bytes(&record[tab + 1..]),
                (mode, Oid::from_hex(oid)?),
            );
        }
        Ok(map)
    }

    /// Se per qualche file regolare i byte del worktree non sono quelli che `git restore`
    /// ricreerebbe da `stash` (es. CRLF con `core.autocrlf=input`, che il filtro `clean` toglie
    /// e lo `smudge` non rimette), li salva senza filtri (`hash-object -w --no-filters`) in un
    /// commit con i soli quei file e genitore `stash`, che l'undo usa per riscriverli identici.
    /// `None` se non serve. Un link o un tipo di file diverso dal backup è un errore: il
    /// backup non lo rappresenta.
    fn save_raw(&self, stash: &Oid, paths: &[RepoPath]) -> Result<Option<Oid>, EngineError> {
        let in_stash = self.ls_tree(stash, paths)?;
        let mut raw = Vec::new();
        let mut wrong = Vec::new();
        for path in paths {
            let disk = Self::disk(&self.worktree_path(path))?;
            let entry = in_stash.get(path);
            let expected = entry.map(|e| self.checkout_bytes(path, e)).transpose()?;
            match (disk, expected, entry) {
                (a, b, _) if a == b => {}
                (Some((false, bytes)), Some((false, _)), Some((mode, _))) => {
                    raw.push((path.clone(), *mode, self.hash_blob(&bytes)?));
                }
                _ => wrong.push(path.clone()),
            }
        }
        if !wrong.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "il backup del worktree (`git stash create`, {stash}) non ha il contenuto attuale \
                 di {} (un file cambiato proprio adesso?): niente scartato, riprovare",
                list(&wrong)
            )));
        }
        if raw.is_empty() {
            return Ok(None);
        }
        let index = super::TempIndex::new(&self.git_dir);
        let index_env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        let mut info = Vec::new();
        for (path, mode, oid) in &raw {
            info.extend_from_slice(format!("{mode:o} {oid}\t").as_bytes());
            info.extend_from_slice(path.as_bytes());
            info.push(0);
        }
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
        let parent = stash.to_string();
        let mut message = String::from(
            "Backup del worktree byte per byte, senza filtri, prima di worktree.discard\n\n\
             Il genitore è il commit di `git stash create`; qui i file i cui byte non sono \
             quelli che git ricreerebbe da lì:\n\n",
        );
        for (path, _, _) in &raw {
            message.push_str(&format!("{path}\n"));
        }
        let identity: Vec<(&str, &OsStr)> = super::BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let args = [
            "commit-tree",
            "--no-gpg-sign",
            "-p",
            &parent,
            "-F",
            "-",
            &tree,
        ];
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        Ok(Some(Oid::from_hex(commit.trim_ascii_end())?))
    }

    /// Le voci dell'indice dei file da scartare sono ancora quelle dello stato (modo e id):
    /// se un altro processo git le ha cambiate, errore senza scrivere.
    fn check_discard_index(&self, files: &[DiscardedFile]) -> Result<(), EngineError> {
        let index = self.index_listing()?;
        let same = files
            .iter()
            .all(|f| index.get(&f.path).map(|e| (e.mode, e.oid)) == Some((f.mode, f.oid)));
        if same {
            Ok(())
        } else {
            Err(EngineError::NotAllowed(
                "l'indice è cambiato mentre si preparava lo scarto (un altro processo git?): \
                 niente scartato, riprovare"
                    .to_owned(),
            ))
        }
    }

    /// `git stash create`: il commit con il worktree e l'indice attuali (non tocca né l'uno
    /// né l'altro, né `refs/stash`). Senza `core.fsmonitor`, che se non è aggiornato farebbe
    /// saltare un file cambiato, e con l'identità fissa dei backup (riesce anche senza
    /// `user.name`).
    fn stash_create(&self) -> Result<Oid, EngineError> {
        let args = [
            "-c",
            "core.fsmonitor=false",
            "stash",
            "create",
            STASH_MESSAGE,
        ];
        let identity: Vec<(&str, &OsStr)> = super::BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let out = self
            .git
            .output_with_input_env(Some(&self.workdir), &args, b"", &identity)?;
        if !out.status.success() {
            // Indice bloccato nel frattempo: `IndexLocked`, come per le altre scritture.
            return Err(match failure(&args, &out) {
                EngineError::Git { command, stderr } => EngineError::Git {
                    command,
                    stderr: format!(
                        "{stderr}; il backup del worktree non si può creare, niente scartato"
                    ),
                },
                other => other,
            });
        }
        let hex = out.stdout.trim_ascii();
        if hex.is_empty() {
            // Nessuna modifica secondo git: lo stato letto prima non è più quello attuale.
            return Err(EngineError::NotAllowed(
                "`git stash create` non ha trovato modifiche da salvare (il worktree è cambiato \
                 nel frattempo?): niente scartato, rileggere lo stato"
                    .to_owned(),
            ));
        }
        Ok(Oid::from_hex(hex)?)
    }

    /// I file di `paths` il cui worktree è diverso dal commit `commit` (o dall'indice, con
    /// `None`): `git diff --name-only -z [<commit>] -- <paths>`, senza fsmonitor, rinomine,
    /// diff esterni né textconv. Qui **non** si usa `diff.autoRefreshIndex=false` (come nelle
    /// altre letture): senza il confronto del contenuto che quell'opzione spegne, un file
    /// cambiato solo nei dati di `stat` (riscritto uguale) comparirebbe fra i diversi.
    /// `--no-optional-locks` evita comunque che git riscriva l'indice per aggiornarli.
    fn differing(
        &self,
        commit: Option<&Oid>,
        paths: &[RepoPath],
    ) -> Result<Vec<RepoPath>, EngineError> {
        let commit = commit.map(Oid::to_string);
        let mut args = vec![
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "--literal-pathspecs",
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
        ];
        if let Some(commit) = &commit {
            args.extend(["--end-of-options", commit]);
        }
        args.push("--");
        let os: Vec<OsString> = paths.iter().map(|p| os_from_bytes(p.as_bytes())).collect();
        let os: Vec<&OsStr> = os.iter().map(OsString::as_os_str).collect();
        let out = self.git.run_with_paths(Some(&self.workdir), &args, &os)?;
        Ok(out
            .split(|&b| b == 0)
            .filter(|r| !r.is_empty())
            .map(RepoPath::from_bytes)
            .collect())
    }
}

/// Aggiunge a un errore di git il ref del backup, che resta.
fn with_backup(err: EngineError, backup: &str) -> EngineError {
    match err {
        EngineError::Git { command, stderr } => EngineError::Git {
            command,
            stderr: format!("{stderr}; il worktree prima dello scarto è salvato in `{backup}`"),
        },
        other => other,
    }
}

fn list(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
