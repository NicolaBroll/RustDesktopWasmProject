//! `edit.undo` / `edit.redo` on the [`MemoryBackend`]: refs and HEAD moved with the same
//! checks as the `CliBackend` ([`RefRestore::check`], [`RefRestore::conflicts`]) and the backup
//! refs of the commits that would no longer be reachable (in
//! [`MemoryBackend::commit_backups`], with the names `refs/project/backup/undo-<n>`).
//!
//! The index and worktree do not change, as with `update-ref`: the index is the previous one, and the
//! status entries are recomputed against the new HEAD (the changes of the undone commit
//! reappear in the stage). Staged renames come back as deletion and addition: the in-memory repo
//! does not detect renames. The in-memory repo has no symbolic refs or replace
//! objects: for reachability the refs, the new values and the backups count.

use super::write::{TreeEntry, ZERO, entry_order, tracked_info};
use super::{HEADS, HeadRef, MemoryBackend, RefEntry};
use crate::EngineError;
use crate::git::{BACKUP_PREFIX, HeadState, RefRestore};
use project_domain::{Change, EntryKind, Oid, RepoPath, StatusEntry, TrackedInfo};
use std::collections::{BTreeMap, BTreeSet};

impl MemoryBackend {
    /// [`GitBackend::restore_refs`](crate::GitBackend::restore_refs). An error changes
    /// nothing.
    pub(super) fn restore(&mut self, request: &RefRestore) -> Result<Vec<String>, EngineError> {
        request.check(self.operation)?;
        let head = self.head_state();
        let conflicts =
            request.conflicts(&head, |name| Ok(self.refs.get(name).map(RefEntry::value)))?;
        if !conflicts.is_empty() {
            return Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            });
        }

        // What stays reachable afterwards: the new values, the other refs and the backups.
        let moved: BTreeSet<&str> = request.refs.iter().map(|r| r.name.as_str()).collect();
        let mut keep = request.targets();
        keep.extend(
            self.refs
                .iter()
                .filter(|(name, _)| !moved.contains(name.as_str()))
                .map(|(_, entry)| entry.target),
        );
        keep.extend(self.commit_backups.iter().map(|(_, oid)| *oid));
        let reachable: BTreeSet<Oid> = keep
            .iter()
            .flat_map(|k| self.reachable(*k))
            .chain(keep.iter().copied())
            .collect();
        let kind = request.backup_kind();
        let prefix = format!("{BACKUP_PREFIX}{kind}-");
        let old_index = self.index_tree();

        let mut backups = Vec::new();
        for oid in request.abandoned() {
            let commit = self.tag_objects.get(&oid).copied().unwrap_or(oid);
            if reachable.contains(&commit) {
                continue;
            }
            let n = self
                .commit_backups
                .iter()
                .filter(|(name, _)| name.starts_with(&prefix))
                .count()
                + 1;
            let name = format!("{prefix}{n}");
            self.commit_backups.push((name.clone(), oid));
            backups.push(name);
        }
        for r in &request.refs {
            match r.target {
                Some(target) => {
                    // A tag object goes back with the commit it points to.
                    let entry = match self.tag_objects.get(&target) {
                        Some(commit) => RefEntry {
                            target: *commit,
                            tag_object: Some(target),
                        },
                        None => RefEntry {
                            target,
                            tag_object: None,
                        },
                    };
                    self.refs.insert(r.name.clone(), entry);
                }
                None => {
                    self.refs.remove(&r.name);
                }
            }
        }
        if let HeadState::Detached(target) = request.head_target {
            self.head = HeadRef::Detached(target);
        }
        self.rebase_entries(&old_index);
        Ok(backups)
    }

    /// HEAD as the journal records it.
    pub(super) fn head_state(&self) -> HeadState {
        match &self.head {
            HeadRef::Branch(name) => HeadState::Branch(format!("{HEADS}{name}")),
            HeadRef::Detached(oid) => HeadState::Detached(*oid),
        }
    }

    /// After a HEAD move: the status entries relative to the new HEAD, with the same
    /// index (`index`, the index tree before the move) and the same worktree. The
    /// conflicted, untracked and ignored files stay as they were; an `intent-to-add` file
    /// (no version in the index) too.
    pub(super) fn rebase_entries(&mut self, index: &BTreeMap<RepoPath, TreeEntry>) {
        let head = self.head_tree();
        let mut tracked = BTreeMap::new();
        let mut next = Vec::new();
        for entry in std::mem::take(&mut self.entries) {
            if tracked_info(&entry).is_some() {
                tracked.insert(entry.path.clone(), entry);
            } else {
                next.push(entry);
            }
        }
        let unmerged: BTreeSet<RepoPath> = next
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.clone())
            .collect();
        let paths: BTreeSet<RepoPath> = index
            .keys()
            .chain(head.keys())
            .chain(tracked.keys())
            .cloned()
            .collect();
        for path in paths {
            if unmerged.contains(&path) {
                continue;
            }
            let existing = tracked.remove(&path);
            if let Some(entry) = &existing
                && entry.worktree == Change::Added
                && !index.contains_key(&path)
            {
                next.push(entry.clone());
                continue;
            }
            let (mode_head, oid_head) = head.get(&path).copied().unwrap_or((0, ZERO));
            let (mode_index, oid_index) = index.get(&path).copied().unwrap_or((0, ZERO));
            let (worktree, mode_worktree, submodule) = match existing.as_ref().and_then(|e| {
                tracked_info(e).map(|info| (e.worktree, info.mode_worktree, info.submodule))
            }) {
                Some(found) => found,
                None => (Change::Unmodified, mode_index, None),
            };
            let info = TrackedInfo {
                submodule,
                mode_head,
                mode_index,
                mode_worktree,
                oid_head,
                oid_index,
            };
            let entry = StatusEntry {
                path,
                index: Change::Unmodified,
                worktree,
                kind: EntryKind::Ordinary(info.clone()),
            };
            next.extend(self.finish(entry, info));
        }
        next.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        self.entries = next;
    }
}

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
