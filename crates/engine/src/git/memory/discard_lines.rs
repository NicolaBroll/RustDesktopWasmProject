//! `worktree.discard_lines` and its undo/redo on the [`MemoryBackend`], with the same checks as
//! the `CliBackend` ([`discard_lines_target`], [`discard_line_patch`], strict [`apply_patch`]).
//! There are no filters: the worktree bytes are the clean content.
//!
//! The backup is `refs/project/backup/discard-<n>` in [`MemoryBackend::line_backups`], with the
//! blob of the old bytes.

use super::MemoryBackend;
use super::write::{entry_order, tracked_info};
use crate::git::{
    BACKUP_PREFIX, DiscardLines, DiscardLinesOutcome, LinesBackup, LinesRestore,
    discard_line_patch, discard_lines_target,
};
use crate::{EngineError, GitRead, apply_patch};
use project_domain::{Change, EntryKind, Oid, RepoPath, StatusEntry, TrackedInfo};

impl MemoryBackend {
    /// The backups saved by `worktree.discard_lines`, in order: ref name, path and blob with the
    /// bytes before.
    pub fn line_backups(&self) -> &[(String, RepoPath, Oid)] {
        &self.line_backups
    }

    /// From now on the backup of `worktree.discard_lines` fails with `message` (nothing is
    /// written).
    pub fn fail_line_backups(&mut self, message: impl Into<String>) {
        self.line_backup_failure = Some(message.into());
    }

    /// [`GitIndex::discard_lines`](crate::GitIndex::discard_lines). An error changes nothing.
    pub(super) fn discard_line_selection(
        &mut self,
        selection: &DiscardLines,
    ) -> Result<DiscardLinesOutcome, EngineError> {
        let path = &selection.path;
        discard_lines_target(&self.status()?, path)?;
        let prepared = discard_line_patch(&self.diff(&selection.diff_spec())?, selection)?;
        let mode = self
            .entries
            .iter()
            .find(|e| e.path == *path)
            .and_then(tracked_info)
            .map(|i| i.mode_worktree)
            .ok_or_else(|| {
                EngineError::NotAllowed(msg!("engine-discard-lines-not-a-file", path = path))
            })?;
        let current = self.worktree.get(path).cloned().ok_or_else(|| {
            EngineError::NotAllowed(msg!("engine-discard-lines-not-a-file", path = path))
        })?;
        let new = apply_patch(Some(&current), &prepared.patch)
            .map_err(|e| {
                EngineError::NotAllowed(msg!("engine-lines-patch-failed", target = path, error = e))
            })?
            .ok_or_else(|| {
                EngineError::NotAllowed(msg!("engine-discard-lines-whole-file", path = path))
            })?;
        if let Some(message) = &self.line_backup_failure {
            return Err(EngineError::Git {
                command: "update-ref".to_owned(),
                stderr: message.clone(),
            });
        }
        let before = self.store_blob(current);
        let after = self.store_blob(new.clone());
        let n = self.worktree_backups.len() + self.line_backups.len() + 1;
        let backup = format!("{BACKUP_PREFIX}discard-{n}");
        self.line_backups
            .push((backup.clone(), path.clone(), before));
        self.set_worktree_bytes(path, new);
        Ok(DiscardLinesOutcome {
            file: LinesBackup {
                path: path.clone(),
                mode,
                before,
                after,
            },
            backup,
            warnings: Vec::new(),
        })
    }

    /// [`GitIndex::restore_lines`](crate::GitIndex::restore_lines). An error changes nothing.
    pub(super) fn restore_discarded_lines(
        &mut self,
        request: &LinesRestore,
    ) -> Result<(), EngineError> {
        let path = &request.file.path;
        let (expected, wanted) = request.blobs();
        let mode = self.entries.iter().find(|e| e.path == *path).map_or_else(
            || self.index_tree().get(path).map(|e| e.0),
            |e| tracked_info(e).map(|i| i.mode_worktree),
        );
        let as_expected = mode == Some(request.file.mode)
            && self.worktree_bytes(path).as_ref() == self.blobs.get(&expected);
        if !as_expected {
            return Err(request.conflict());
        }
        let content =
            self.blobs
                .get(&wanted)
                .cloned()
                .ok_or_else(|| EngineError::GitWithReason {
                    command: "cat-file".to_owned(),
                    reason: msg!("engine-memory-backup-missing", stash = wanted),
                })?;
        self.set_worktree_bytes(path, content);
        Ok(())
    }

    /// The worktree bytes of the tracked file `path`: those known, or the index version for a
    /// file without unstaged changes.
    fn worktree_bytes(&self, path: &RepoPath) -> Option<Vec<u8>> {
        if let Some(bytes) = self.worktree.get(path) {
            return Some(bytes.clone());
        }
        let changed = self
            .entries
            .iter()
            .any(|e| e.path == *path && e.worktree.is_change());
        if changed {
            return None;
        }
        let (_, oid) = self.index_tree().get(path).copied()?;
        self.blobs.get(&oid).cloned()
    }

    /// Writes `content` to the worktree file `path` (tracked, same mode) and recomputes its
    /// status entry: no unstaged change if it equals the index.
    fn set_worktree_bytes(&mut self, path: &RepoPath, content: Vec<u8>) {
        let (mut entry, info) = match self
            .entries
            .iter()
            .position(|e| e.path == *path && tracked_info(e).is_some())
        {
            Some(i) => {
                let entry = self.entries.remove(i);
                let info = tracked_info(&entry).cloned().expect("tracked entry");
                (entry, info)
            }
            None => {
                let Some((mode, oid)) = self.index_tree().get(path).copied() else {
                    self.worktree.insert(path.clone(), content);
                    return;
                };
                let info = TrackedInfo {
                    submodule: None,
                    mode_head: mode,
                    mode_index: mode,
                    mode_worktree: mode,
                    oid_head: oid,
                    oid_index: oid,
                };
                let entry = StatusEntry {
                    path: path.clone(),
                    index: Change::Unmodified,
                    worktree: Change::Unmodified,
                    kind: EntryKind::Ordinary(info.clone()),
                };
                (entry, info)
            }
        };
        self.worktree.insert(path.clone(), content);
        entry.worktree =
            if info.mode_worktree == info.mode_index && self.worktree_is(path, info.oid_index) {
                Change::Unmodified
            } else {
                Change::Modified
            };
        self.entries.extend(self.finish(entry, info));
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
    }
}

#[cfg(test)]
#[path = "discard_lines_tests.rs"]
mod tests;
