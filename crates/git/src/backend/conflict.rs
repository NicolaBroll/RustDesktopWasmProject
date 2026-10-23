//! Editor dei conflitti sul [`CliBackend`] (`conflict.show`, `conflict.resolve`, S30), secondo
//! §5 e `impl-docs/git/conflitti.md`.
//!
//! - versioni: gli id degli stadi 1/2/3 dal record `u` dello stato (`git status
//!   --porcelain=v2`, gli stessi di `ls-files -s`), i byte con `git cat-file blob <id>` (senza
//!   filtri); il worktree letto dal file system (un link o una cartella, anche fra le cartelle
//!   del percorso, → `NotAllowed`); `conflict-marker-size` con `git check-attr -z --stdin`;
//! - risoluzione: controlli (conflitto, digest, indice bloccato), backup dei byte del worktree
//!   (`hash-object -w --no-filters --stdin`, albero in un indice temporaneo, `commit-tree` senza
//!   genitori con l'identità dei backup, ref nuovo `refs/project/backup/conflict-...`), file
//!   temporaneo nella stessa cartella e `rename` dopo aver ricontrollato il digest, poi lo stage
//!   di [`GitBackend::stage`] (`git add` del solo percorso, con il suo backup dell'indice).

use super::{BACKUP_IDENTITY, CliBackend};
use crate::command::os_from_bytes;
use project_domain::RepoPath;
use project_engine::{
    ConflictResolution, ConflictResolved, ConflictVersions, EngineError, GitBackend, StagedVersion,
    check_conflict, check_digest, marker_size,
};
use std::ffi::OsStr;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Modo di un file eseguibile nell'indice.
const EXECUTABLE: u32 = 0o100755;
/// Modo di un file normale nell'indice.
const REGULAR: u32 = 0o100644;

/// Un file letto dal worktree: i byte e il modo per l'indice.
struct WorktreeFile {
    bytes: Vec<u8>,
    mode: u32,
}

/// Aggiunge a un errore arrivato dopo il backup il nome del ref che lo contiene.
fn naming_backup(error: EngineError, backup: &Option<String>) -> EngineError {
    let Some(backup) = backup else {
        return error;
    };
    let note = format!(
        " (il file del worktree com'era prima è salvato in `{backup}`: `git show \
         {backup}:<percorso>`)"
    );
    match error {
        EngineError::Git { command, stderr } => EngineError::Git {
            command,
            stderr: stderr + &note,
        },
        EngineError::NotAllowed(reason) => EngineError::NotAllowed(reason + &note),
        EngineError::StaleDiff { path, reason } => EngineError::StaleDiff {
            path,
            reason: reason + &note,
        },
        other => EngineError::Git {
            command: "add".to_owned(),
            stderr: format!("{other}{note}"),
        },
    }
}

/// Un errore di I/O sul file del worktree.
fn io_error(what: &str, path: &RepoPath, error: std::io::Error) -> EngineError {
    EngineError::Git {
        command: "add".to_owned(),
        stderr: format!("{what} `{path}` non riuscito: {error}"),
    }
}

impl CliBackend {
    /// Il percorso del file nel worktree, dopo aver controllato che nessuna cartella del
    /// percorso sia un link simbolico (si scriverebbe fuori dal repo, o in un altro file).
    fn conflict_file_path(&self, path: &RepoPath) -> Result<PathBuf, EngineError> {
        let bytes = path.as_bytes();
        for (i, _) in bytes.iter().enumerate().filter(|(_, b)| **b == b'/') {
            let dir = self.workdir.join(os_from_bytes(&bytes[..i]));
            if let Ok(meta) = dir.symlink_metadata()
                && !meta.is_dir()
            {
                return Err(EngineError::not_a_file(path));
            }
        }
        Ok(self.workdir.join(os_from_bytes(bytes)))
    }

    /// Il file del worktree: `None` se non c'è; un link o una cartella → `NotAllowed`.
    fn read_conflict_file(&self, path: &RepoPath) -> Result<Option<WorktreeFile>, EngineError> {
        let full = self.conflict_file_path(path)?;
        let meta = match full.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error("lettura di", path, e)),
        };
        if !meta.is_file() {
            return Err(EngineError::not_a_file(path));
        }
        let bytes = std::fs::read(&full).map_err(|e| io_error("lettura di", path, e))?;
        Ok(Some(WorktreeFile {
            bytes,
            mode: if is_executable(&meta) {
                EXECUTABLE
            } else {
                REGULAR
            },
        }))
    }

    /// `conflict-marker-size` di `path` (`git check-attr -z --stdin`, percorso su stdin).
    fn conflict_marker_size(&self, path: &RepoPath) -> Result<usize, EngineError> {
        let mut input = path.as_bytes().to_vec();
        input.push(0);
        let args = ["check-attr", "-z", "--stdin", "conflict-marker-size"];
        let out = self.run_input(&args, &input, &[])?;
        // `<percorso> NUL <attributo> NUL <valore> NUL`
        let value = out
            .split(|&b| b == 0)
            .nth(2)
            .map(|v| String::from_utf8_lossy(v).into_owned());
        Ok(marker_size(value.as_deref()))
    }

    /// Lo stadio `oid` se c'è (modo diverso da 0).
    fn stage_bytes(
        &self,
        mode: u32,
        oid: &project_domain::Oid,
    ) -> Result<Option<Vec<u8>>, EngineError> {
        if mode == 0 {
            return Ok(None);
        }
        self.blob(oid).map(Some)
    }

    /// Il backup dei byte del worktree: un commit senza genitori con il solo file al suo
    /// percorso, in un ref nuovo `refs/project/backup/conflict-...`.
    fn conflict_backup(&self, path: &RepoPath, file: &WorktreeFile) -> Result<String, EngineError> {
        let oid = self.hash_blob(&file.bytes)?;
        let tree = self.backup_tree(&[StagedVersion {
            path: path.clone(),
            mode: file.mode,
            oid,
        }])?;
        let message = format!(
            "Backup del worktree prima di conflict.resolve\n\nIl file in conflitto com'era nel \
             worktree (marcatori e modifiche a mano compresi):\n\n{path}\n"
        );
        let identity: Vec<(&str, &OsStr)> = BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let args = ["commit-tree", "--no-gpg-sign", "-F", "-", &tree];
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        let commit = String::from_utf8_lossy(commit.trim_ascii_end()).into_owned();
        self.create_backup_ref(
            "conflict",
            &commit,
            "project: backup del worktree prima di conflict.resolve",
        )
    }

    /// Scrive `content` al posto del file `path` (o lo cancella con `None`), dopo aver
    /// ricontrollato che il worktree sia ancora quello di `digest`. Il file nuovo prende i
    /// permessi di quello che sostituisce; se non c'era, è eseguibile quando lo stadio rimasto
    /// lo è (`executable`).
    fn write_resolution(
        &self,
        request: &ConflictResolution,
        before: Option<&WorktreeFile>,
        executable: bool,
    ) -> Result<(), EngineError> {
        let path = &request.path;
        let full = self.conflict_file_path(path)?;
        let recheck = || -> Result<(), EngineError> {
            let now = self.read_conflict_file(path)?;
            check_digest(
                path,
                &request.digest,
                now.as_ref().map(|f| f.bytes.as_slice()),
                "",
            )
        };
        let Some(content) = &request.content else {
            recheck()?;
            if before.is_some() {
                std::fs::remove_file(&full).map_err(|e| io_error("cancellazione di", path, e))?;
            }
            return Ok(());
        };
        let dir = full.parent().unwrap_or(&self.workdir).to_path_buf();
        if before.is_none() {
            // Prima del ricontrollo: dopo uno `StaleDiff` non restano cartelle vuote.
            recheck()?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| io_error("creazione della cartella di", path, e))?;
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let temp = dir.join(format!(
            ".project-conflict-{}-{nanos}.tmp",
            std::process::id()
        ));
        let result = (|| -> Result<(), EngineError> {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)
                .map_err(|e| io_error("scrittura di", path, e))?;
            file.write_all(content)
                .and_then(|()| file.sync_all())
                .map_err(|e| io_error("scrittura di", path, e))?;
            drop(file);
            let perms = match before {
                Some(_) => std::fs::metadata(&full)
                    .map(|m| std::fs::set_permissions(&temp, m.permissions())),
                None => Ok(set_executable(&temp, executable)),
            };
            perms
                .and_then(|r| r)
                .map_err(|e| io_error("scrittura di", path, e))?;
            recheck()?;
            std::fs::rename(&temp, &full).map_err(|e| io_error("scrittura di", path, e))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    /// [`GitBackend::conflict_versions`].
    pub(super) fn conflict_files(&self, path: &RepoPath) -> Result<ConflictVersions, EngineError> {
        let status = self.status()?;
        let info = check_conflict("conflict.show", &status, path)?;
        let worktree = self.read_conflict_file(path)?.map(|f| f.bytes);
        Ok(ConflictVersions {
            path: path.clone(),
            kind: info.conflict,
            base: self.stage_bytes(info.mode_base, &info.oid_base)?,
            ours: self.stage_bytes(info.mode_ours, &info.oid_ours)?,
            theirs: self.stage_bytes(info.mode_theirs, &info.oid_theirs)?,
            worktree,
            marker_size: self.conflict_marker_size(path)?,
        })
    }

    /// [`GitBackend::resolve_conflict`].
    pub(super) fn resolve_file(
        &mut self,
        request: &ConflictResolution,
    ) -> Result<ConflictResolved, EngineError> {
        let path = &request.path;
        let status = self.status_for_write()?;
        let info = check_conflict("conflict.resolve", &status, path)?;
        let executable = [info.mode_ours, info.mode_theirs].contains(&EXECUTABLE);
        let before = self.read_conflict_file(path)?;
        check_digest(
            path,
            &request.digest,
            before.as_ref().map(|f| f.bytes.as_slice()),
            "",
        )?;
        self.check_index_unlocked()?;
        let backup = match &before {
            Some(file) => Some(self.conflict_backup(path, file)?),
            None => None,
        };
        self.write_resolution(request, before.as_ref(), executable)
            .map_err(|e| naming_backup(e, &backup))?;
        let index_backup = self
            .stage(std::slice::from_ref(path))
            .map_err(|e| naming_backup(e, &backup))?;
        Ok(ConflictResolved {
            backup,
            index_backup,
        })
    }
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Un file nuovo senza un file da sostituire: eseguibile o no (il resto dalla `umask`, come i
/// file che scrive git).
#[cfg(unix)]
fn set_executable(file: &std::path::Path, executable: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if !executable {
        return Ok(());
    }
    let mut perms = std::fs::metadata(file)?.permissions();
    let mode = perms.mode();
    perms.set_mode(mode | ((mode & 0o444) >> 2));
    std::fs::set_permissions(file, perms)
}

#[cfg(not(unix))]
fn set_executable(_file: &std::path::Path, _executable: bool) -> std::io::Result<()> {
    Ok(())
}
