//! `worktree.discard` and its undo/redo on the [`MemoryBackend`], with the same checks as the
//! `CliBackend` ([`discard_plan`], [`WorktreeRestore::index_conflicts`]).
//!
//! The "commit of `git stash create`" is a fake id registered in
//! [`MemoryBackend::worktree_backups`] with the ref `refs/project/backup/discard-<n>`, the status
//! entry and the worktree content of every discarded file (`None` if it was deleted).

use super::MemoryBackend;
use super::write::{entry_order, tracked_info};
use crate::EngineError;
use crate::GitRead;
use crate::git::{
    BACKUP_PREFIX, DiscardOutcome, DiscardedFile, WorktreeRestore, check_same_plan, discard_plan,
};
use project_domain::{Change, EntryKind, Oid, RepoPath, StatusEntry};
use std::collections::BTreeMap;

/// A file saved by the backup of a discard: the status entry before the discard and the
/// worktree content (`None` = the file was deleted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashedFile {
    pub entry: StatusEntry,
    pub content: Option<Vec<u8>>,
}

/// The backup of a discard: ref name, (fake) commit and saved files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeStash {
    pub name: String,
    pub stash: Oid,
    pub files: BTreeMap<RepoPath, StashedFile>,
}

impl MemoryBackend {
    /// The worktree backups saved by `worktree.discard`, in order.
    pub fn worktree_backups(&self) -> &[WorktreeStash] {
        &self.worktree_backups
    }

    /// The content of file `path` in the worktree, if present (and if known).
    pub fn worktree_file(&self, path: &RepoPath) -> Option<&[u8]> {
        self.worktree.get(path).map(Vec::as_slice)
    }

    /// [`GitIndex::discard`]. An error changes nothing.
    pub(super) fn discard_paths(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError> {
        let files = discard_plan(&self.status()?, paths)?;
        check_same_plan(&files, expected)?;
        let n = self.worktree_backups.len() as u64 + 1;
        let stash = MemoryBackend::fake_oid(0x5354_4153_4800_0000 + n);
        let saved = files
            .iter()
            .filter_map(|f| {
                let entry = self.tracked_entry(&f.path)?.clone();
                let content = match f.change {
                    Change::Deleted => None,
                    _ => self.worktree.get(&f.path).cloned(),
                };
                Some((f.path.clone(), StashedFile { entry, content }))
            })
            .collect();
        let name = format!("{BACKUP_PREFIX}discard-{n}");
        self.worktree_backups.push(WorktreeStash {
            name: name.clone(),
            stash,
            files: saved,
        });
        self.discard_files(&files);
        Ok(DiscardOutcome {
            files,
            backup: name,
            stash,
            raw: None,
        })
    }

    /// [`GitIndex::restore_worktree`]. An error changes nothing.
    pub(super) fn restore_discarded(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<(), EngineError> {
        let stash = self
            .worktree_backups
            .iter()
            .find(|b| b.stash == request.stash)
            .cloned()
            .ok_or_else(|| EngineError::GitWithReason {
                command: "restore".to_owned(),
                reason: msg!("engine-memory-backup-missing", stash = request.stash),
            })?;
        let index = self.index_tree();
        let mut conflicts = request.index_conflicts(|p| index.get(p).copied());
        for file in &request.files {
            let saved = stash.files.get(&file.path);
            let as_expected = if request.redo {
                // As the undo left it: the saved entry and content.
                saved.is_some_and(|s| {
                    self.tracked_entry(&file.path).map(|e| e.worktree) == Some(s.entry.worktree)
                        && self.worktree.get(&file.path) == s.content.as_ref()
                })
            } else {
                // As the discard left it: equal to the index.
                saved.is_some()
                    && !self.entries.iter().any(|e| {
                        e.path == file.path
                            && (e.worktree != Change::Unmodified || tracked_info(e).is_none())
                    })
            };
            if !as_expected {
                conflicts.push(request.worktree_conflict(&file.path));
            }
        }
        request.conflict_error(conflicts)?;

        if request.redo {
            self.discard_files(&request.files);
            return Ok(());
        }
        for file in &request.files {
            let saved = &stash.files[&file.path];
            let Some(saved_info) = tracked_info(&saved.entry) else {
                continue;
            };
            // The index is the recorded one; HEAD is reread (the entry is recomputed against
            // the current one).
            let mut entry = match self.entries.iter().position(|e| e.path == file.path) {
                Some(i) => self.entries.remove(i),
                None => StatusEntry {
                    path: file.path.clone(),
                    index: Change::Unmodified,
                    worktree: Change::Unmodified,
                    kind: EntryKind::Ordinary(project_domain::TrackedInfo {
                        submodule: None,
                        mode_head: file.mode,
                        mode_index: file.mode,
                        mode_worktree: file.mode,
                        oid_head: file.oid,
                        oid_index: file.oid,
                    }),
                },
            };
            let mut info = tracked_info(&entry)
                .cloned()
                .expect("entry of a tracked file");
            info.mode_worktree = saved_info.mode_worktree;
            entry.worktree = saved.entry.worktree;
            match &saved.content {
                Some(content) => self.worktree.insert(file.path.clone(), content.clone()),
                None => self.worktree.remove(&file.path),
            };
            self.entries.extend(self.finish(entry, info));
        }
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        Ok(())
    }

    /// The entry of a tracked file (not conflicted, untracked or ignored) with path
    /// `path`.
    fn tracked_entry(&self, path: &RepoPath) -> Option<&StatusEntry> {
        self.entries
            .iter()
            .find(|e| e.path == *path && tracked_info(e).is_some())
    }

    /// Brings the worktree of `files` back to the staged version and recomputes the entries.
    fn discard_files(&mut self, files: &[DiscardedFile]) {
        for file in files {
            let Some(i) = self
                .entries
                .iter()
                .position(|e| e.path == file.path && tracked_info(e).is_some())
            else {
                continue;
            };
            let mut entry = self.entries.remove(i);
            let mut info = tracked_info(&entry)
                .cloned()
                .expect("entry of a tracked file");
            match self.blobs.get(&info.oid_index).cloned() {
                Some(content) => self.worktree.insert(file.path.clone(), content),
                None => self.worktree.remove(&file.path),
            };
            info.mode_worktree = info.mode_index;
            entry.worktree = Change::Unmodified;
            self.entries.extend(self.finish(entry, info));
        }
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
    }
}

#[cfg(test)]
#[path = "discard_tests.rs"]
mod tests;
