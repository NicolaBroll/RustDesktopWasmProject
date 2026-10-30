//! Conflict editor on the [`MemoryBackend`] (`conflict.show`, `conflict.resolve`, S30), with
//! the same checks as the `CliBackend`.
//!
//! The `u` entries of the status (from [`merge_three_way`](MemoryBackend::merge_three_way) or
//! [`set_entries`](MemoryBackend::set_entries)) have the ids of the three stages; the contents are
//! in the
//! blobs. The worktree backup is a commit without parents with just the file, in the store of
//! commit backups (like the other `refs/project/backup/...` refs).

use super::{MemoryBackend, commit::Tree};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, ConflictResolution, ConflictResolved, ConflictVersions, DEFAULT_MARKER_SIZE,
    GitRead, check_conflict, check_digest,
};
use project_domain::{Commit, EntryKind, Oid, RepoPath, Signature};

/// Mode of a normal file, for a file the worktree did not have.
const FILE_MODE: u32 = 0o100644;

impl MemoryBackend {
    /// The content of a stage: `None` if missing (mode 0).
    fn stage_blob(&self, mode: u32, oid: Oid) -> Result<Option<Vec<u8>>, EngineError> {
        if mode == 0 {
            return Ok(None);
        }
        self.blobs
            .get(&oid)
            .cloned()
            .map(Some)
            .ok_or_else(|| EngineError::GitWithReason {
                command: "cat-file".to_owned(),
                reason: msg!("engine-memory-unknown-blob", oid = oid),
            })
    }

    /// [`GitOperations::conflict_versions`].
    pub(super) fn conflict_files(&self, path: &RepoPath) -> Result<ConflictVersions, EngineError> {
        let status = self.status()?;
        let info = check_conflict("conflict.show", &status, path)?;
        Ok(ConflictVersions {
            path: path.clone(),
            kind: info.conflict,
            base: self.stage_blob(info.mode_base, info.oid_base)?,
            ours: self.stage_blob(info.mode_ours, info.oid_ours)?,
            theirs: self.stage_blob(info.mode_theirs, info.oid_theirs)?,
            worktree: self.worktree.get(path).cloned(),
            marker_size: DEFAULT_MARKER_SIZE,
        })
    }

    /// [`GitOperations::resolve_conflict`]. An error changes nothing.
    pub(super) fn resolve_file(
        &mut self,
        request: &ConflictResolution,
    ) -> Result<ConflictResolved, EngineError> {
        let path = &request.path;
        let status = self.status()?;
        let info = check_conflict("conflict.resolve", &status, path)?.clone();
        let current = self.worktree.get(path).cloned();
        check_digest(path, &request.digest, current.as_deref())?;

        let mut work = self.clone();
        let backup = match current {
            Some(bytes) => Some(work.conflict_backup(path, info.mode_worktree, bytes)?),
            None => None,
        };
        // The new file in the worktree, and its mode in the `u` entry that the stage reads.
        let mode = match &request.content {
            Some(content) => {
                work.worktree.insert(path.clone(), content.clone());
                if info.mode_worktree == 0 {
                    FILE_MODE
                } else {
                    info.mode_worktree
                }
            }
            None => {
                work.worktree.remove(path);
                0
            }
        };
        for entry in &mut work.entries {
            if let EntryKind::Unmerged(u) = &mut entry.kind
                && entry.path == *path
            {
                u.mode_worktree = mode;
            }
        }
        let index_backup = work.stage_paths(std::slice::from_ref(path))?;
        *self = work;
        Ok(ConflictResolved {
            backup,
            index_backup,
        })
    }

    /// The backup of the worktree bytes of `path`: a commit without parents with just the file.
    fn conflict_backup(
        &mut self,
        path: &RepoPath,
        mode: u32,
        bytes: Vec<u8>,
    ) -> Result<String, EngineError> {
        let mode = if mode == 0 { FILE_MODE } else { mode };
        let blob = self.store_blob(bytes);
        let tree: Tree = [(path.clone(), (mode, blob))].into_iter().collect();
        let message =
            format!("project: backup of the worktree before conflict.resolve\n\n{path}\n");
        let sig = Signature {
            name: "Project".to_owned(),
            email: "backup@project.invalid".to_owned(),
            time: self.next_time(),
            offset_min: 0,
        };
        let id = self.new_commit_id(&[], &message, &tree, sig.time);
        let (summary, body) = crate::git::split_message(&message);
        self.insert_commit(Commit {
            id,
            parents: Vec::new(),
            author: sig.clone(),
            committer: sig,
            summary,
            body,
        });
        self.set_changes(id, super::commit::tree_changes(&Tree::new(), &tree));
        let prefix = format!("{BACKUP_PREFIX}conflict-");
        let n = self
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix))
            .count()
            + 1;
        let name = format!("{prefix}{n}");
        self.commit_backups.push((name.clone(), id));
        Ok(name)
    }
}
