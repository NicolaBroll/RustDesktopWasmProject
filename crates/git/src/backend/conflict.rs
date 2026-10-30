//! Conflict editor on the [`CliBackend`] (`conflict.show`, `conflict.resolve`, S30), per
//! §5 and `impl-docs/git/conflicts.md`.
//!
//! - versions: the ids of stages 1/2/3 from the `u` record of the status (`git status
//!   --porcelain=v2`, the same as `ls-files -s`), the bytes with `git cat-file blob <id>` (no
//!   filters); the worktree read from the file system (a link or a directory, also among the
//!   directories of the path, → `NotAllowed`); `conflict-marker-size` with `git check-attr -z --stdin`;
//! - resolution: checks (conflict, digest, locked index), backup of the worktree bytes
//!   (`hash-object -w --no-filters --stdin`, tree in a temporary index, `commit-tree` with no
//!   parents using the backup identity, new ref `refs/project/backup/conflict-...`), temporary
//!   file in the same directory and `rename` after rechecking the digest, then the stage
//!   of [`GitIndex::stage`] (`git add` of the path only, with its index backup).

use super::{BACKUP_IDENTITY, CliBackend};
use crate::command::os_from_bytes;
use project_domain::RepoPath;
use project_engine::{
    ConflictResolution, ConflictResolved, ConflictVersions, EngineError, GitIndex, GitRead,
    Message, StagedVersion, check_conflict, check_digest, marker_size,
};
use std::ffi::OsStr;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Mode of an executable file in the index.
const EXECUTABLE: u32 = 0o100755;
/// Mode of a regular file in the index.
const REGULAR: u32 = 0o100644;

/// A file read from the worktree: the bytes and the mode for the index.
struct WorktreeFile {
    bytes: Vec<u8>,
    mode: u32,
}

/// Adds to an error that arrived after the backup the name of the ref that holds it.
fn naming_backup(error: EngineError, backup: &Option<String>) -> EngineError {
    let Some(backup) = backup else {
        return error;
    };
    let note = msg!("engine-conflict-backup-note", backup = backup);
    match error {
        EngineError::Git { command, stderr } => EngineError::GitWithReason {
            command,
            reason: msg!("engine-note-paren", text = stderr, note = note),
        },
        EngineError::GitWithReason { command, reason } => EngineError::GitWithReason {
            command,
            reason: msg!("engine-note-paren", text = reason, note = note),
        },
        EngineError::NotAllowed(reason) => {
            EngineError::NotAllowed(msg!("engine-note-paren", text = reason, note = note))
        }
        EngineError::StaleDiff { path, reason } => EngineError::StaleDiff {
            path,
            reason: msg!("engine-note-paren", text = reason, note = note),
        },
        other => EngineError::GitWithReason {
            command: "add".to_owned(),
            reason: msg!("engine-note-paren", text = other, note = note),
        },
    }
}

/// An I/O error on the worktree file.
fn io_error(what: Message, path: &RepoPath, error: std::io::Error) -> EngineError {
    EngineError::GitWithReason {
        command: "add".to_owned(),
        reason: msg!(
            "engine-conflict-io",
            what = what,
            path = path,
            error = error
        ),
    }
}

impl CliBackend {
    /// The path of the file in the worktree, after checking that no directory of the
    /// path is a symbolic link (it would write outside the repo, or into another file).
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

    /// The worktree file: `None` if absent; a link or a directory → `NotAllowed`.
    fn read_conflict_file(&self, path: &RepoPath) -> Result<Option<WorktreeFile>, EngineError> {
        let full = self.conflict_file_path(path)?;
        let meta = match full.symlink_metadata() {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error(msg!("engine-conflict-io-read"), path, e)),
        };
        if !meta.is_file() {
            return Err(EngineError::not_a_file(path));
        }
        let bytes =
            std::fs::read(&full).map_err(|e| io_error(msg!("engine-conflict-io-read"), path, e))?;
        Ok(Some(WorktreeFile {
            bytes,
            mode: if is_executable(&meta) {
                EXECUTABLE
            } else {
                REGULAR
            },
        }))
    }

    /// `conflict-marker-size` of `path` (`git check-attr -z --stdin`, path on stdin).
    fn conflict_marker_size(&self, path: &RepoPath) -> Result<usize, EngineError> {
        let mut input = path.as_bytes().to_vec();
        input.push(0);
        let args = ["check-attr", "-z", "--stdin", "conflict-marker-size"];
        let out = self.run_input(&args, &input, &[])?;
        // `<path> NUL <attribute> NUL <value> NUL`
        let value = out
            .split(|&b| b == 0)
            .nth(2)
            .map(|v| String::from_utf8_lossy(v).into_owned());
        Ok(marker_size(value.as_deref()))
    }

    /// The stage `oid` if present (mode other than 0).
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

    /// The backup of the worktree bytes: a commit with no parents holding only the file at its
    /// path, in a new ref `refs/project/backup/conflict-...`.
    fn conflict_backup(&self, path: &RepoPath, file: &WorktreeFile) -> Result<String, EngineError> {
        let oid = self.hash_blob(&file.bytes)?;
        let tree = self.backup_tree(&[StagedVersion {
            path: path.clone(),
            mode: file.mode,
            oid,
        }])?;
        let message = format!(
            "Backup of the worktree before conflict.resolve\n\nThe conflicted file as it was in the \
             worktree (markers and manual edits included):\n\n{path}\n"
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
            "project: backup of the worktree before conflict.resolve",
        )
    }

    /// Writes `content` in place of the file `path` (or deletes it with `None`), after
    /// rechecking that the worktree is still the one of `digest`. The new file takes the
    /// permissions of the one it replaces; if there was none, it is executable when the
    /// remaining stage is (`executable`).
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
            )
        };
        let Some(content) = &request.content else {
            recheck()?;
            if before.is_some() {
                std::fs::remove_file(&full)
                    .map_err(|e| io_error(msg!("engine-conflict-io-delete"), path, e))?;
            }
            return Ok(());
        };
        let dir = full.parent().unwrap_or(&self.workdir).to_path_buf();
        if before.is_none() {
            // Before the recheck: after a `StaleDiff` no empty directories are left.
            recheck()?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| io_error(msg!("engine-conflict-io-mkdir"), path, e))?;
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
                .map_err(|e| io_error(msg!("engine-conflict-io-write"), path, e))?;
            file.write_all(content)
                .and_then(|()| file.sync_all())
                .map_err(|e| io_error(msg!("engine-conflict-io-write"), path, e))?;
            drop(file);
            let perms = match before {
                Some(_) => std::fs::metadata(&full)
                    .map(|m| std::fs::set_permissions(&temp, m.permissions())),
                None => Ok(set_executable(&temp, executable)),
            };
            perms
                .and_then(|r| r)
                .map_err(|e| io_error(msg!("engine-conflict-io-write"), path, e))?;
            recheck()?;
            std::fs::rename(&temp, &full)
                .map_err(|e| io_error(msg!("engine-conflict-io-write"), path, e))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }

    /// [`GitOperations::conflict_versions`](project_engine::GitOperations::conflict_versions).
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

    /// [`GitOperations::resolve_conflict`](project_engine::GitOperations::resolve_conflict).
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

/// A new file with no file to replace: executable or not (the rest from the `umask`, like the
/// files git writes).
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
