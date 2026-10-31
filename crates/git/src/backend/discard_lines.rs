//! `worktree.discard_lines` and its undo/redo on the [`CliBackend`] (steps and guarantees in
//! `impl-docs/git/discard.md`, "Discarding lines").
//!
//! Discard, in this order, and nothing is written to the worktree if a step fails:
//!
//! 1. status without `core.fsmonitor` and [`discard_lines_target`] (the refusals of
//!    `worktree.discard`, only a modified file); the index entry (`ls-files -s`) is the status one;
//! 2. the file's `diff.worktree` diff and [`discard_line_patch`] (digest, selection); the diff's
//!    old side must be that index entry;
//! 3. every folder on the path is a real folder; the file is a regular file (`lstat`): its bytes
//!    and mode (executable bit) are read once;
//! 4. **clean and smudge**: the bytes cleaned (`hash-object -w --stdin --path=<path>`, with the
//!    path's filters and line-ending conversions) must give the diff's new id (otherwise
//!    [`EngineError::StaleDiff`]), and smudging that blob back (`cat-file --filters
//!    --path=<path>`) must give exactly the bytes on disk (otherwise
//!    [`EngineError::NotAllowed`]: rewriting the file would change lines that were not
//!    chosen, e.g. CRLF with `core.autocrlf=input`);
//! 5. the new clean content with the strict [`apply_patch`], saved without filters, and its
//!    smudged form: the bytes to write;
//! 6. backup: the exact current bytes (`hash-object -w --no-filters`) and mode in a commit with only
//!    the file (temporary index), held by a new ref `refs/project/backup/discard-...`;
//!    verified with `ls-tree`. Its parent is a commit with the bytes to write, so that both
//!    blobs of the journal entry stay reachable (redo after a `git gc`);
//! 7. **last check**: the file still has the bytes and mode read in step 3 (otherwise
//!    [`EngineError::StaleDiff`], nothing written; the backup ref stays, harmless);
//! 8. atomic write: a new temporary file in the same folder (`create_new`), the bytes, the
//!    permissions of the old file, `sync_all`, then `rename` over the file.
//!
//! Undo/redo: HEAD (checked by the journal), then the file must have exactly the expected bytes
//! and mode, then the other bytes are written the same way. Afterwards the file is reread: if it
//! does not have the bytes written, a warning.

use super::CliBackend;
use super::clean::file_mode;
use crate::command::{failure, os_from_bytes};
use project_domain::{Oid, RepoPath};
use project_engine::{
    DiscardLines, DiscardLinesOutcome, EngineError, GitRead, LinesBackup, LinesRestore, Message,
    RestoreOutcome, apply_patch, discard_line_patch, discard_lines_target,
};
use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

impl CliBackend {
    /// [`GitIndex::discard_lines`](project_engine::GitIndex::discard_lines).
    pub(super) fn discard_line_selection(
        &mut self,
        selection: &DiscardLines,
    ) -> Result<DiscardLinesOutcome, EngineError> {
        let path = &selection.path;
        let status = self.status_for_write()?;
        let entry = discard_lines_target(&status, path)?;
        let index = self.index_listing()?;
        if index.get(path).map(|e| (e.mode, e.oid)) != Some(entry) {
            return Err(EngineError::NotAllowed(msg!("engine-discard-index-moved")));
        }
        let diff = self.diff(&selection.diff_spec())?;
        let prepared = discard_line_patch(&diff, selection)?;
        let changed = || EngineError::StaleDiff {
            path: path.to_string(),
            reason: msg!("engine-discard-lines-changed"),
        };
        if prepared.file.old_oid != Some(entry.1) {
            return Err(changed());
        }
        self.check_real_dirs(std::slice::from_ref(path))?;
        let (mode, bytes) = self.read_file(path)?;
        if prepared.file.new_mode.is_some_and(|m| m != mode) {
            return Err(changed());
        }

        // The bytes on disk clean to the diff's new side, and smudge back to themselves.
        let clean_oid = self.hash_with_filters(path, &bytes)?;
        if prepared.file.new_oid != Some(clean_oid) {
            return Err(changed());
        }
        if self.checkout_bytes(path, &(mode, clean_oid))?.1 != bytes {
            return Err(EngineError::NotAllowed(msg!(
                "engine-discard-lines-filters",
                path = path
            )));
        }
        let clean = self.blob(&clean_oid)?;
        let new_clean = apply_patch(Some(&clean), &prepared.patch)
            .map_err(|e| {
                EngineError::NotAllowed(msg!("engine-lines-patch-failed", target = path, error = e))
            })?
            .ok_or_else(|| {
                EngineError::NotAllowed(msg!("engine-discard-lines-whole-file", path = path))
            })?;
        let new_oid = self.hash_blob(&new_clean)?;
        let (_, new_bytes) = self.checkout_bytes(path, &(mode, new_oid))?;

        // Backup of the exact bytes and mode.
        let before = self.hash_blob(&bytes)?;
        let after = self.hash_blob(&new_bytes)?;
        // The bytes to write go in the parent commit: so the blob that redo writes again
        // stays reachable (`git gc` would prune it otherwise).
        let message = format!(
            "{BACKUP_MESSAGE}\n\nThe bytes worktree.discard_lines writes (the child commit has \
             the bytes before):\n\n{path}\n"
        );
        let written = self.files_commit(&[(path.clone(), mode, after)], &message, None)?;
        let message = format!(
            "{BACKUP_MESSAGE}\n\nThe exact bytes and mode of the file before \
             worktree.discard_lines (`git show <commit>:<path>`; the parent has the bytes \
             written):\n\n{path}\n"
        );
        let commit =
            self.files_commit(&[(path.clone(), mode, before)], &message, Some(&written))?;
        let backup = self.create_backup_ref("discard", &commit.to_string(), BACKUP_MESSAGE)?;
        let in_backup = self.ls_tree(&commit, std::slice::from_ref(path))?;
        if in_backup.get(path) != Some(&(mode, before)) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-discard-backup-incomplete",
                backup = backup,
                files = format!("`{path}`")
            )));
        }

        // Last check before writing: the file is still the one read.
        self.check_real_dirs(std::slice::from_ref(path))?;
        if self.read_file(path).ok() != Some((mode, bytes)) {
            return Err(changed());
        }
        self.write_file(path, &new_bytes, &backup)?;
        let warnings = self.not_written("worktree.discard_lines", path, mode, &new_bytes, &backup);
        Ok(DiscardLinesOutcome {
            file: LinesBackup {
                path: path.clone(),
                mode,
                before,
                after,
            },
            backup,
            warnings,
        })
    }

    /// [`GitIndex::restore_lines`](project_engine::GitIndex::restore_lines).
    pub(super) fn restore_discarded_lines(
        &mut self,
        request: &LinesRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        let path = &request.file.path;
        let (expected_oid, wanted) = request.blobs();
        let expected = self.blob(&expected_oid)?;
        let content = self.blob(&wanted)?;
        self.check_real_dirs(std::slice::from_ref(path))
            .map_err(|_| request.conflict())?;
        if self.read_file(path).ok() != Some((request.file.mode, expected)) {
            return Err(request.conflict());
        }
        // The bytes overwritten stay in the object database as blob `expected_oid`.
        let note = format!("blob {expected_oid}");
        self.write_file(path, &content, &note)?;
        let warnings = self.not_written(request.command, path, request.file.mode, &content, &note);
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    /// The git mode (`100644`/`100755`) and the bytes of the regular file `path`; a link, a folder,
    /// a special file or a missing file is [`EngineError::NotAllowed`].
    fn read_file(&self, path: &RepoPath) -> Result<(u32, Vec<u8>), EngineError> {
        let full = self.worktree_path(path);
        let not_a_file =
            || EngineError::NotAllowed(msg!("engine-discard-lines-not-a-file", path = path));
        let meta = std::fs::symlink_metadata(&full).map_err(|_| not_a_file())?;
        if !meta.is_file() {
            return Err(not_a_file());
        }
        let bytes = std::fs::read(&full).map_err(|e| EngineError::GitWithReason {
            command: "read".to_owned(),
            reason: msg!("engine-discard-unreadable", path = path, error = e),
        })?;
        Ok((file_mode(&meta), bytes))
    }

    /// The blob id of `bytes` cleaned as git would for `path` (`hash-object -w --stdin
    /// --path=<path>`: `clean` filters and line-ending conversions of that path), written to
    /// the object database.
    fn hash_with_filters(&self, path: &RepoPath, bytes: &[u8]) -> Result<Oid, EngineError> {
        let mut arg = OsString::from("--path=");
        arg.push(os_from_bytes(path.as_bytes()));
        let args = [
            OsStr::new("hash-object"),
            OsStr::new("-w"),
            OsStr::new("--stdin"),
            &arg,
        ];
        let out = self
            .git
            .output_os_with_input(Some(&self.workdir), &args, bytes)?;
        if !out.status.success() {
            return Err(failure(&["hash-object"], &out));
        }
        Ok(Oid::from_hex(out.stdout.trim_ascii_end())?)
    }

    /// Replaces the file `path` with `content`, atomically: a new temporary file next to it, with
    /// the old file's permissions, synced to disk and renamed over it. If a step fails the file is
    /// untouched and the temporary file removed; the error names `backup`.
    fn write_file(&self, path: &RepoPath, content: &[u8], backup: &str) -> Result<(), EngineError> {
        let full = self.worktree_path(path);
        let error = |e: std::io::Error| EngineError::GitWithReason {
            command: "write".to_owned(),
            reason: msg!(
                "engine-discard-lines-write-failed",
                path = path,
                error = e,
                backup = backup
            ),
        };
        let permissions = std::fs::metadata(&full).map_err(error)?.permissions();
        let temp = temp_name(&full);
        let written = (|| {
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(content)?;
            file.set_permissions(permissions)?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temp, &full)
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&temp);
            return Err(error(e));
        }
        Ok(())
    }

    /// A warning if `path` reread does not have `mode` and `content` (another program wrote it
    /// right after?).
    fn not_written(
        &self,
        command: &str,
        path: &RepoPath,
        mode: u32,
        content: &[u8],
        backup: &str,
    ) -> Vec<Message> {
        match self.read_file(path) {
            Ok((m, bytes)) if m == mode && bytes == content => Vec::new(),
            _ => vec![msg!(
                "engine-discard-lines-not-as-expected",
                command = command,
                path = path,
                backup = backup
            )],
        }
    }
}

/// Reflog and first line of the backup commit.
const BACKUP_MESSAGE: &str = "project: backup of the file before worktree.discard_lines";

/// `.<name>.project-<pid>-<nanoseconds>.tmp` in the folder of `full`.
fn temp_name(full: &Path) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut name = OsString::from(".");
    name.push(full.file_name().unwrap_or_default());
    name.push(format!(".project-{}-{nanos}.tmp", std::process::id()));
    full.with_file_name(name)
}
