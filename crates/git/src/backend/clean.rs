//! `worktree.clean` and its undo/redo on the [`CliBackend`] (steps and guarantees in
//! `impl-docs/git/discard.md`, "Deleting untracked files").
//!
//! Clean, in this order, and nothing is deleted if a step before 6 fails:
//!
//! 1. status without `core.fsmonitor` (`--untracked-files=all`), [`clean_plan`], and for each
//!    file `lstat`: a regular file (mode from the executable bit) or a link, every folder of
//!    its path a real folder (not a link); comparison with the confirmed plan;
//! 2. the bytes of every file (for a link its target, never followed) saved as blobs with
//!    `hash-object -w --stdin --no-filters` (no `clean` filter or line-ending conversion);
//! 3. a tree built in a **temporary index** (`GIT_INDEX_FILE`, `update-index --index-info`,
//!    `write-tree`) and a commit without parent (`commit-tree`, fixed identity, no signing):
//!    the user's index is never touched;
//! 4. the ref `refs/project/backup/clean-...`, created only if it does not exist;
//! 5. **last verification**: the tree of the backup has every file with its mode and blob,
//!    and the bytes on disk now hash to the same blob (and the file is still of the same kind);
//! 6. `remove_file` of each file (a link is removed, not its target), after checking again that
//!    no folder of its path became a link; then the folders left empty, from the deepest.
//!
//! Undo: every path absent (`lstat`) and no file or link in place of its folders; then the
//! files recreated from the backup tree with `create_new` (never overwriting), the executable
//! bit, links as links; re-read afterwards (warnings). Redo: every file still untracked in the
//! status, of the same kind and mode, and with the backup's bytes; then deleted again.

use super::CliBackend;
use super::discard::os_bytes;
use crate::command::os_from_bytes;
use project_domain::{EntryKind, Oid, RepoPath};
use project_engine::{
    CleanOutcome, CleanRestore, CleanedFile, EngineError, Message, RestoreOutcome,
    check_same_clean_plan, clean_plan,
};
use std::collections::BTreeSet;
use std::ffi::OsStr;

/// Mode of a link in git.
const LINK: u32 = 0o120000;
/// Mode of an executable file in git.
const EXECUTABLE: u32 = 0o100755;
/// Mode of a regular file in git.
const REGULAR: u32 = 0o100644;

/// Reflog and first line of the backup commit message: fixed, no user data.
const BACKUP_MESSAGE: &str = "project: backup of untracked files before worktree.clean";

impl CliBackend {
    /// [`GitIndex::clean_plan`](project_engine::GitIndex::clean_plan).
    pub(super) fn plan_clean(&self, paths: &[RepoPath]) -> Result<Vec<CleanedFile>, EngineError> {
        let status = self.status_for_write()?;
        let files = clean_plan(&status, paths)?;
        let mut out = Vec::with_capacity(files.len());
        let mut bad = Vec::new();
        for path in files {
            match self.on_disk(&path)? {
                Some((mode, size)) if self.real_parents(&path) => {
                    out.push(CleanedFile { path, mode, size })
                }
                _ => bad.push(path),
            }
        }
        if !bad.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-clean-not-a-file",
                files = list(&bad)
            )));
        }
        Ok(out)
    }

    /// [`GitIndex::clean`](project_engine::GitIndex::clean).
    pub(super) fn clean_paths(
        &mut self,
        paths: &[RepoPath],
        expected: &[CleanedFile],
    ) -> Result<CleanOutcome, EngineError> {
        let files = self.plan_clean(paths)?;
        check_same_clean_plan(&files, expected)?;

        // The blobs, from the bytes read now (the mode and size must still be the plan's).
        let mut blobs = Vec::with_capacity(files.len());
        let mut changed = Vec::new();
        for file in &files {
            let (mode, bytes) = self.read_entry(&file.path)?;
            if mode != file.mode || bytes.len() as u64 != file.size {
                changed.push(file.path.clone());
            }
            blobs.push((file.path.clone(), file.mode, self.hash_blob(&bytes)?));
        }
        if !changed.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-clean-not-a-file",
                files = list(&changed)
            )));
        }
        let commit = self.backup_commit(&blobs)?;
        let backup = self.create_backup_ref("clean", &commit.to_string(), BACKUP_MESSAGE)?;

        // Last step before deleting: the backup has exactly what is on disk now.
        let targets: Vec<RepoPath> = files.iter().map(|f| f.path.clone()).collect();
        let tree = self.ls_tree(&commit, &targets)?;
        let mut missing = Vec::new();
        for (path, mode, oid) in &blobs {
            let in_tree = tree.get(path) == Some(&(*mode, *oid));
            if !in_tree || !self.real_parents(path) || !self.same_as(path, *mode, oid)? {
                missing.push(path.clone());
            }
        }
        if !missing.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-clean-backup-incomplete",
                backup = backup,
                files = list(&missing)
            )));
        }

        let (deleted, failures) = self.delete_files(&files);
        if deleted.is_empty() && !failures.is_empty() {
            return Err(EngineError::GitWithReason {
                command: "clean".to_owned(),
                reason: msg!(
                    "engine-clean-nothing-deleted",
                    errors = failures.join("; "),
                    backup = backup
                ),
            });
        }
        let dirs = self.remove_empty_parents(&deleted);
        let mut warnings = Vec::new();
        if !failures.is_empty() {
            warnings.push(not_deleted(&files, &deleted, &failures, &backup));
        }
        Ok(CleanOutcome {
            files: deleted,
            backup,
            commit,
            dirs,
            warnings,
        })
    }

    /// [`GitIndex::restore_cleaned`](project_engine::GitIndex::restore_cleaned).
    pub(super) fn restore_clean(
        &mut self,
        request: &CleanRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        let paths = request.paths();
        if paths.is_empty() {
            return Ok(RestoreOutcome::default());
        }
        let tree = self.ls_tree(&request.commit, &paths)?;
        let mut conflicts = Vec::new();
        if request.redo {
            // As the undo left it: untracked, same kind, mode and bytes as the backup.
            let status = self.status_for_write()?;
            let untracked: BTreeSet<&RepoPath> = status
                .entries
                .iter()
                .filter(|e| matches!(e.kind, EntryKind::Untracked))
                .map(|e| &e.path)
                .collect();
            for path in &paths {
                let same = match tree.get(path) {
                    Some((mode, oid)) => {
                        untracked.contains(path)
                            && self.real_parents(path)
                            && self.same_as(path, *mode, oid)?
                    }
                    None => false,
                };
                if !same {
                    conflicts.push(path.clone());
                }
            }
        } else {
            // As the clean left it: nothing at the path, nothing but folders on the way. A file
            // already equal to the backup (an earlier undo that stopped midway) counts as
            // restored, so the undo can be retried.
            for path in &paths {
                let ok = match std::fs::symlink_metadata(self.worktree_path(path)) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.parents_absent_or_dirs(path)
                    }
                    Err(_) => false,
                    Ok(_) => self.already_restored(path, &tree)?,
                };
                if !ok {
                    conflicts.push(path.clone());
                }
            }
        }
        request.conflict_error(&conflicts)?;
        let commit = request.commit;

        if request.redo {
            let (deleted, failures) = self.delete_files(&request.files);
            if deleted.is_empty() && !failures.is_empty() {
                return Err(EngineError::GitWithReason {
                    command: "clean".to_owned(),
                    reason: msg!(
                        "engine-clean-nothing-deleted",
                        errors = failures.join("; "),
                        backup = commit
                    ),
                });
            }
            self.remove_empty_parents(&deleted);
            let mut warnings = Vec::new();
            if !failures.is_empty() {
                warnings.push(not_deleted(
                    &request.files,
                    &deleted,
                    &failures,
                    &commit.to_string(),
                ));
            }
            return Ok(RestoreOutcome {
                backups: Vec::new(),
                warnings,
            });
        }

        // Every file must be in the backup before anything is written.
        if let Some(path) = paths.iter().find(|p| !tree.contains_key(*p)) {
            return Err(restore_failed(
                path,
                msg!("engine-clean-not-in-backup"),
                &commit,
            ));
        }
        for path in &paths {
            if self.already_restored(path, &tree)? {
                continue;
            }
            let (mode, oid) = &tree[path];
            let content = self.blob(oid)?;
            self.recreate(path, *mode, &content)
                .map_err(|e| restore_failed(path, e, &commit))?;
        }
        // Re-read: are the files as in the backup?
        let mut wrong = Vec::new();
        for path in &paths {
            let ok = match tree.get(path) {
                Some((mode, oid)) => self.same_as(path, *mode, oid)?,
                None => false,
            };
            if !ok {
                wrong.push(path.clone());
            }
        }
        let warnings = if wrong.is_empty() {
            Vec::new()
        } else {
            vec![msg!(
                "engine-clean-not-as-expected",
                command = request.command,
                files = list(&wrong),
                commit = commit
            )]
        };
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    /// The git mode and size of what is at `path`, without following a link: `None` if there is
    /// nothing, or a folder or special file.
    fn on_disk(&self, path: &RepoPath) -> Result<Option<(u32, u64)>, EngineError> {
        let full = self.worktree_path(path);
        let meta = match std::fs::symlink_metadata(&full) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(unreadable(path, e)),
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&full).map_err(|e| unreadable(path, e))?;
            return Ok(Some((LINK, os_bytes(target.as_os_str()).len() as u64)));
        }
        if !meta.is_file() {
            return Ok(None);
        }
        Ok(Some((file_mode(&meta), meta.len())))
    }

    /// The git mode and the bytes at `path` (for a link its target): a folder or special file
    /// is an error.
    fn read_entry(&self, path: &RepoPath) -> Result<(u32, Vec<u8>), EngineError> {
        let full = self.worktree_path(path);
        let meta = std::fs::symlink_metadata(&full).map_err(|e| unreadable(path, e))?;
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&full).map_err(|e| unreadable(path, e))?;
            return Ok((LINK, os_bytes(target.as_os_str())));
        }
        if !meta.is_file() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-clean-not-a-file",
                files = list(std::slice::from_ref(path))
            )));
        }
        let bytes = std::fs::read(&full).map_err(|e| unreadable(path, e))?;
        Ok((file_mode(&meta), bytes))
    }

    /// True if what is at `path` now has mode `mode` and its bytes hash to `oid` (without
    /// writing the object).
    fn same_as(&self, path: &RepoPath, mode: u32, oid: &Oid) -> Result<bool, EngineError> {
        match self.on_disk(path)? {
            Some((m, _)) if m == mode => {}
            _ => return Ok(false),
        }
        let (m, bytes) = self.read_entry(path)?;
        if m != mode {
            return Ok(false);
        }
        let args = ["hash-object", "--stdin", "--no-filters"];
        let out = self.run_input(&args, &bytes, &[])?;
        Ok(Oid::from_hex(out.trim_ascii_end())? == *oid)
    }

    /// The backup commit: a tree with `blobs` (path, mode, blob) built in a temporary index, and
    /// a commit without parent with the fixed identity, never signed.
    fn backup_commit(&self, blobs: &[(RepoPath, u32, Oid)]) -> Result<Oid, EngineError> {
        let mut message = format!(
            "{BACKUP_MESSAGE}\n\nThe exact bytes and mode of the untracked files deleted by \
             worktree.clean (`git show <commit>:<path>`):\n\n"
        );
        for (path, _, _) in blobs {
            message.push_str(&format!("{path}\n"));
        }
        self.files_commit(blobs, &message, None)
    }

    /// A commit (with parent `parent`, if any) whose tree has only `blobs` (path, mode, blob),
    /// built in a temporary index (the user's is not touched), with the fixed identity, never
    /// signed, and the message `message`. The backups of `worktree.clean` and
    /// `worktree.discard_lines`.
    pub(super) fn files_commit(
        &self,
        blobs: &[(RepoPath, u32, Oid)],
        message: &str,
        parent: Option<&Oid>,
    ) -> Result<Oid, EngineError> {
        let index = super::TempIndex::new(&self.git_dir);
        let index_env = [("GIT_INDEX_FILE", index.0.as_os_str())];
        let mut info = Vec::new();
        for (path, mode, oid) in blobs {
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
        let identity: Vec<(&str, &OsStr)> = super::BACKUP_IDENTITY
            .iter()
            .map(|(k, v)| (*k, OsStr::new(v)))
            .collect();
        let parent = parent.map(Oid::to_string);
        let mut args = vec!["commit-tree", "--no-gpg-sign"];
        if let Some(parent) = &parent {
            args.extend(["-p", parent]);
        }
        args.extend(["-F", "-", &tree]);
        let commit = self.run_input(&args, message.as_bytes(), &identity)?;
        Ok(Oid::from_hex(commit.trim_ascii_end())?)
    }

    /// Deletes each of `files` (a link as a link) if its folders are still real folders and it
    /// is still of the planned kind: the files deleted, and one text per failure.
    fn delete_files(&self, files: &[CleanedFile]) -> (Vec<CleanedFile>, Vec<String>) {
        let mut deleted = Vec::new();
        let mut failures = Vec::new();
        for file in files {
            let kind_ok = matches!(self.on_disk(&file.path), Ok(Some((m, _))) if m == file.mode);
            if !self.real_parents(&file.path) || !kind_ok {
                failures.push(format!(
                    "`{}`: {}",
                    file.path,
                    msg!("engine-clean-changed-after-backup")
                ));
                continue;
            }
            match std::fs::remove_file(self.worktree_path(&file.path)) {
                Ok(()) => deleted.push(file.clone()),
                Err(e) => failures.push(format!("`{}`: {e}", file.path)),
            }
        }
        (deleted, failures)
    }

    /// Removes the folders that the deletion of `files` left empty (`remove_dir` only removes an
    /// empty folder), from each file's folder up, stopping at the first one that is not empty;
    /// never the worktree root. The folders removed, deepest first.
    fn remove_empty_parents(&self, files: &[CleanedFile]) -> Vec<RepoPath> {
        let mut dirs: BTreeSet<RepoPath> = BTreeSet::new();
        for file in files {
            for dir in parents(&file.path).into_iter().rev() {
                if dirs.contains(&dir) {
                    continue;
                }
                let full = self.worktree_path(&dir);
                // Never through a link (which `remove_dir` would refuse anyway).
                let real = std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_dir());
                if !real || std::fs::remove_dir(&full).is_err() {
                    break;
                }
                dirs.insert(dir);
            }
        }
        let mut out: Vec<RepoPath> = dirs.into_iter().collect();
        out.sort_by(|a, b| depth(b).cmp(&depth(a)).then_with(|| a.cmp(b)));
        out
    }

    /// True if every folder in the path of `path` is a real folder (not a link, not a file).
    fn real_parents(&self, path: &RepoPath) -> bool {
        parents(path).iter().all(|dir| {
            std::fs::symlink_metadata(self.worktree_path(dir))
                .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
        })
    }

    /// True if what is at `path` (below real folders) is already the backup's entry: kind, mode
    /// and bytes. An entry missing from `tree` is never restored.
    fn already_restored(
        &self,
        path: &RepoPath,
        tree: &std::collections::BTreeMap<RepoPath, (u32, Oid)>,
    ) -> Result<bool, EngineError> {
        match tree.get(path) {
            Some((mode, oid)) => Ok(self.real_parents(path) && self.same_as(path, *mode, oid)?),
            None => Ok(false),
        }
    }

    /// True if every folder in the path of `path` is either missing (`NotFound`: the undo
    /// creates it) or a real folder: never a file or a link that the undo would write through,
    /// and any other error (permissions...) is a refusal.
    fn parents_absent_or_dirs(&self, path: &RepoPath) -> bool {
        for dir in parents(path) {
            match std::fs::symlink_metadata(self.worktree_path(&dir)) {
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
                Ok(_) => return false,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
                Err(_) => return false,
            }
        }
        true
    }

    /// Creates the missing folders in the path of `path` one level at a time, checking right
    /// after finding or creating each one that it is a real folder (not a link, not a file):
    /// nothing is ever written through a link.
    fn create_real_parents(&self, path: &RepoPath) -> std::io::Result<()> {
        for dir in parents(path) {
            let full = self.worktree_path(&dir);
            match std::fs::symlink_metadata(&full) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    match std::fs::create_dir(&full) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
            let meta = std::fs::symlink_metadata(&full)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(std::io::ErrorKind::NotADirectory.into());
            }
        }
        Ok(())
    }

    /// Writes `content` at `path` with git mode `mode` (a link for `0o120000`), creating the
    /// missing folders; fails if anything is already there.
    fn recreate(&self, path: &RepoPath, mode: u32, content: &[u8]) -> std::io::Result<()> {
        let full = self.worktree_path(path);
        self.create_real_parents(path)?;
        if mode == LINK {
            return make_link(content, &full);
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)?;
        f.write_all(content)?;
        f.sync_all()?;
        if mode == EXECUTABLE {
            set_executable(&full)?;
        }
        Ok(())
    }
}

/// The folders in the path of `path`, from the outermost (`a`, `a/b` for `a/b/c`).
fn parents(path: &RepoPath) -> Vec<RepoPath> {
    let bytes = path.as_bytes();
    bytes
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'/')
        .map(|(i, _)| RepoPath::from_bytes(&bytes[..i]))
        .collect()
}

fn depth(path: &RepoPath) -> usize {
    path.as_bytes().iter().filter(|b| **b == b'/').count()
}

#[cfg(unix)]
pub(super) fn file_mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    if meta.permissions().mode() & 0o100 != 0 {
        EXECUTABLE
    } else {
        REGULAR
    }
}

#[cfg(not(unix))]
fn file_mode(_meta: &std::fs::Metadata) -> u32 {
    REGULAR
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    let mode = perms.mode();
    // Like git: execute wherever there is read.
    perms.set_mode(mode | ((mode & 0o444) >> 2));
    std::fs::set_permissions(path, perms)
}

#[cfg(not(unix))]
fn set_executable(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_link(target: &[u8], path: &std::path::Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(os_from_bytes(target), path)
}

#[cfg(not(unix))]
fn make_link(target: &[u8], path: &std::path::Path) -> std::io::Result<()> {
    // Like git with `core.symlinks=false`: a file with the target as its content.
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    f.write_all(target)
}

fn unreadable(path: &RepoPath, error: std::io::Error) -> EngineError {
    EngineError::GitWithReason {
        command: "clean".to_owned(),
        reason: msg!("engine-clean-unreadable", path = path, error = error),
    }
}

fn restore_failed(path: &RepoPath, error: impl std::fmt::Display, commit: &Oid) -> EngineError {
    EngineError::GitWithReason {
        command: "restore".to_owned(),
        reason: msg!(
            "engine-clean-restore-failed",
            path = path,
            error = error.to_string(),
            commit = commit
        ),
    }
}

/// The warning for the files of `files` not in `deleted`.
fn not_deleted(
    files: &[CleanedFile],
    deleted: &[CleanedFile],
    failures: &[String],
    backup: &str,
) -> Message {
    let kept: Vec<RepoPath> = files
        .iter()
        .filter(|f| !deleted.contains(f))
        .map(|f| f.path.clone())
        .collect();
    msg!(
        "engine-clean-not-deleted",
        files = list(&kept),
        errors = failures.join("; "),
        n = kept.len(),
        backup = backup
    )
}

fn list(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
