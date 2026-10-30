//! Stash of the [`MemoryBackend`] (S28), with the same checks as the `CliBackend`
//! ([`check_stash_push`], [`check_stash_apply`], [`stash_local_changes`]).
//!
//! Each stash keeps the status entries it set aside and the worktree content of
//! those files, plus a commit (inserted among the repo's commits, with the files changed relative
//! to the
//! base) so `commit.show`/`diff.commit` with the stash id work as with git. The apply
//! is stricter and simpler than git: a file changed by the stash and also by HEAD relative
//! to the base is always a conflict (git merges line by line), with the stash content
//! in the worktree instead of markers. Dropped stashes are set aside by id, so
//! `stash_store` (the undo of a drop) finds them again whole.

use super::commit::{Tree, tree_changes};
use super::write::{ZERO, entry_order, file_type, tracked_info};
use super::{HeadRef, MemoryBackend};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, GitRead, STASH_REF, StashApplied, StashApply, StashDrop, StashDropped,
    StashEntry, StashPush, check_stash_apply, check_stash_push, stash_blocked, stash_local_changes,
};
use project_domain::{
    Change, Commit, Conflict, EntryKind, Oid, RepoPath, Signature, StatusEntry, TrackedInfo,
    UnmergedInfo,
};
use std::collections::BTreeMap;

/// A stash in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MemoryStash {
    pub(super) id: Oid,
    pub(super) parents: Vec<Oid>,
    pub(super) message: String,
    pub(super) time: i64,
    /// The status entries set aside (tracked files and, with `include_untracked`, untracked
    /// ones), as they were.
    saved: Vec<StatusEntry>,
    /// The worktree content of those files (`None` = absent or unknown).
    files: BTreeMap<RepoPath, Option<Vec<u8>>>,
}

impl MemoryStash {
    fn entry(&self, index: usize) -> Option<StashEntry> {
        StashEntry::new(
            index,
            self.id,
            &self.parents,
            self.time,
            self.message.clone(),
        )
    }

    /// The tracked paths the stash changes (also the old name of a rename) and the untracked
    /// ones it puts back.
    fn targets(&self) -> (Vec<RepoPath>, Vec<RepoPath>) {
        let mut tracked = Vec::new();
        let mut untracked = Vec::new();
        for e in &self.saved {
            match &e.kind {
                EntryKind::Untracked => untracked.push(e.path.clone()),
                EntryKind::Renamed { original, .. } | EntryKind::Copied { original, .. } => {
                    tracked.push(original.clone());
                    tracked.push(e.path.clone());
                }
                _ => tracked.push(e.path.clone()),
            }
        }
        (tracked, untracked)
    }
}

impl MemoryBackend {
    /// Sets the stash (`refs/stash`) to commit `target`, like a new stash at the top
    /// of the list that has no saved files (applying it changes nothing).
    pub fn set_stash(&mut self, target: Oid) {
        let (parents, time) = self
            .commits
            .get(&target)
            .map_or((vec![target], 0), |c| (c.parents.clone(), c.committer.time));
        self.stashes.insert(
            0,
            MemoryStash {
                id: target,
                parents,
                message: "On main: stash".to_owned(),
                time,
                saved: Vec::new(),
                files: BTreeMap::new(),
            },
        );
        self.sync_stash_ref();
    }

    /// `refs/stash` on the stash at the top of the list, or removed if the list is empty.
    fn sync_stash_ref(&mut self) {
        match self.stashes.first() {
            Some(top) => {
                let id = top.id;
                self.set_ref(STASH_REF.to_owned(), id, None);
            }
            None => {
                self.refs.remove(STASH_REF);
            }
        }
    }

    /// [`GitStash::stashes`].
    pub(super) fn stash_list(&self) -> Vec<StashEntry> {
        self.stashes
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.entry(i))
            .collect()
    }

    /// The position of stash `id` in the list, or [`EngineError::NotAllowed`].
    fn stash_position(&self, command: &str, id: &Oid) -> Result<usize, EngineError> {
        self.stashes
            .iter()
            .position(|s| s.id == *id)
            .ok_or_else(|| {
                EngineError::NotAllowed(msg!("engine-stash-gone", command = command, id = id))
            })
    }

    /// [`GitStash::stash_push`]. An error changes nothing.
    pub(super) fn push_stash(&mut self, request: &StashPush) -> Result<StashEntry, EngineError> {
        let status = self.status()?;
        check_stash_push(&status, request)?;
        let head = self
            .head_target()
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-stash-memory-unborn")))?;
        let head_tree = self.head_tree();
        let selects = |e: &StatusEntry| {
            let wanted = |p: &RepoPath| {
                request
                    .paths
                    .as_ref()
                    .is_none_or(|ps| ps.iter().any(|w| crate::path_matches(p, w)))
            };
            let original = match &e.kind {
                EntryKind::Renamed { original, .. } => Some(original),
                _ => None,
            };
            let in_scope = wanted(&e.path) || original.is_some_and(wanted);
            in_scope
                && match e.kind {
                    EntryKind::Untracked => request.include_untracked,
                    EntryKind::Ignored | EntryKind::Unmerged(_) => false,
                    _ => e.index.is_change() || e.worktree.is_change(),
                }
        };
        let (saved, kept): (Vec<StatusEntry>, Vec<StatusEntry>) = std::mem::take(&mut self.entries)
            .into_iter()
            .partition(|e| selects(e));
        self.entries = kept;
        let files: BTreeMap<RepoPath, Option<Vec<u8>>> = saved
            .iter()
            .map(|e| (e.path.clone(), self.worktree.get(&e.path).cloned()))
            .collect();

        // The tree of the stash commit: HEAD with the worktree of the saved tracked files.
        let mut tree: Tree = head_tree.clone();
        for e in &saved {
            let Some(info) = tracked_info(e) else {
                continue;
            };
            if let EntryKind::Renamed { original, .. } = &e.kind {
                tree.remove(original);
            }
            if info.mode_worktree == 0 || e.worktree == Change::Deleted {
                tree.remove(&e.path);
            } else if e.worktree == Change::Unmodified {
                tree.insert(e.path.clone(), (info.mode_index, info.oid_index));
            } else {
                let oid = match files.get(&e.path).cloned().flatten() {
                    Some(content) => self.store_blob(content),
                    None => MemoryBackend::fake_oid(u64::MAX),
                };
                tree.insert(e.path.clone(), (info.mode_worktree, oid));
            }
        }
        let branch = match &self.head {
            HeadRef::Branch(name) => name.clone(),
            HeadRef::Detached(_) => "(no branch)".to_owned(),
        };
        let message = match &request.message {
            Some(m) => format!("On {branch}: {m}"),
            None => {
                let summary = self
                    .commits
                    .get(&head)
                    .map(|c| c.summary.clone())
                    .unwrap_or_default();
                format!("WIP on {branch}: {} {summary}", head.short())
            }
        };
        let time = self.next_time();
        let index_commit = self.new_commit_id(&[head], &format!("index on {branch}"), &tree, time);
        let mut parents = vec![head, index_commit];
        if saved.iter().any(|e| matches!(e.kind, EntryKind::Untracked)) {
            parents.push(super::content_oid(index_commit.as_bytes()));
        }
        let id = self.new_commit_id(&parents, &message, &tree, time);
        let (name, email) = self
            .identity
            .clone()
            .unwrap_or_else(|| ("Project".to_owned(), "stash@project.invalid".to_owned()));
        let who = Signature {
            name,
            email,
            time,
            offset_min: 0,
        };
        self.insert_commit(Commit {
            id,
            parents: parents.clone(),
            author: who.clone(),
            committer: who,
            summary: message.clone(),
            body: String::new(),
        });
        self.set_changes(id, tree_changes(&head_tree, &tree));

        // Index and worktree of the saved files go back as HEAD (with `keep_index` as the index).
        for e in &saved {
            match &e.kind {
                EntryKind::Untracked => {
                    self.worktree.remove(&e.path);
                }
                _ => {
                    let Some(info) = tracked_info(e).cloned() else {
                        continue;
                    };
                    if request.keep_index && e.index.is_change() {
                        let mut entry = e.clone();
                        let mut info = info;
                        info.mode_worktree = info.mode_index;
                        entry.worktree = Change::Unmodified;
                        self.put_worktree(&e.path, info.mode_index, info.oid_index);
                        self.entries.extend(self.finish(entry, info));
                        continue;
                    }
                    if let EntryKind::Renamed { original, .. } = &e.kind
                        && let Some((mode, oid)) = head_tree.get(original).copied()
                    {
                        self.put_worktree(original, mode, oid);
                    }
                    match head_tree.get(&e.path).copied() {
                        Some((mode, oid)) => self.put_worktree(&e.path, mode, oid),
                        None => self.put_worktree(&e.path, 0, ZERO),
                    }
                }
            }
        }
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        self.stashes.insert(
            0,
            MemoryStash {
                id,
                parents,
                message,
                time,
                saved,
                files,
            },
        );
        self.sync_stash_ref();
        Ok(self.stashes[0].entry(0).expect("the stash has its base"))
    }

    /// The worktree file `path` with the content of blob `oid` (mode 0: removed). A blob
    /// that is not known leaves the file without known content.
    fn put_worktree(&mut self, path: &RepoPath, mode: u32, oid: Oid) {
        match self.blobs.get(&oid).cloned() {
            Some(content) if mode != 0 => {
                self.worktree.insert(path.clone(), content);
            }
            _ => {
                self.worktree.remove(path);
            }
        }
    }

    /// [`GitStash::stash_apply`]. A refusal changes nothing.
    pub(super) fn apply_stash(
        &mut self,
        request: &StashApply,
    ) -> Result<StashApplied, EngineError> {
        let position = self.stash_position(request.command, &request.id)?;
        let stash = self.stashes[position].clone();
        let status = self.status()?;
        check_stash_apply(request.command, &status)?;
        let (tracked, untracked) = stash.targets();
        // A staged file with the same index entry the stash saved, and that in the stash
        // had no unstaged changes, already has the stash's version.
        let in_stash = |path: &RepoPath, mode: u32, oid: Oid| {
            stash.saved.iter().any(|e| {
                e.path == *path
                    && e.worktree == Change::Unmodified
                    && tracked_info(e).is_some_and(|i| i.mode_index == mode && i.oid_index == oid)
            })
        };
        let mut blocked = stash_local_changes(&status, &tracked, &untracked, in_stash);
        for p in &untracked {
            if self.worktree.contains_key(p) && !blocked.contains(&p.to_string()) {
                blocked.push(p.to_string());
            }
        }
        if !blocked.is_empty() {
            return Err(stash_blocked(
                request.command,
                &format!("stash@{{{position}}}"),
                &blocked,
            ));
        }
        let head_tree = self.head_tree();
        let base_tree = self.tree_at(stash.parents[0]);
        let mut conflicts = Vec::new();
        for saved in &stash.saved {
            let content = stash.files.get(&saved.path).cloned().flatten();
            let Some(info) = tracked_info(saved).cloned() else {
                // An untracked file comes back as it was.
                self.entries.push(saved.clone());
                if let Some(c) = content {
                    self.worktree.insert(saved.path.clone(), c);
                }
                continue;
            };
            // Already with the stash's version (see above): stays as is.
            if self.entries.iter().any(|e| e.path == saved.path) {
                continue;
            }
            let original = match &saved.kind {
                EntryKind::Renamed { original, .. } => Some(original.clone()),
                _ => None,
            };
            let moved = |p: &RepoPath| head_tree.get(p) != base_tree.get(p);
            if moved(&saved.path) || original.as_ref().is_some_and(moved) {
                let ours = head_tree.get(&saved.path).copied().unwrap_or((0, ZERO));
                let base = base_tree.get(&saved.path).copied().unwrap_or((0, ZERO));
                let theirs = match &content {
                    Some(c) => (info.mode_worktree, self.store_blob(c.clone())),
                    None => (0, ZERO),
                };
                self.entries.push(StatusEntry {
                    path: saved.path.clone(),
                    index: Change::Unmerged,
                    worktree: Change::Unmerged,
                    kind: EntryKind::Unmerged(UnmergedInfo {
                        conflict: Conflict::BothModified,
                        submodule: None,
                        mode_base: base.0,
                        mode_ours: ours.0,
                        mode_theirs: theirs.0,
                        mode_worktree: info.mode_worktree,
                        oid_base: base.1,
                        oid_ours: ours.1,
                        oid_theirs: theirs.1,
                    }),
                });
                match content {
                    Some(c) => self.worktree.insert(saved.path.clone(), c),
                    None => self.worktree.remove(&saved.path),
                };
                conflicts.push(saved.path.to_string());
                continue;
            }
            match &content {
                Some(c) => self.worktree.insert(saved.path.clone(), c.clone()),
                None => self.worktree.remove(&saved.path),
            };
            if request.restore_index {
                self.entries.push(saved.clone());
                continue;
            }
            // Without `restore_index` the index stays as HEAD, except for new files, which git
            // leaves staged with the worktree content.
            if let Some(original) = &original
                && let Some((mode, oid)) = head_tree.get(original).copied()
            {
                let entry = StatusEntry {
                    path: original.clone(),
                    index: Change::Unmodified,
                    worktree: Change::Deleted,
                    kind: EntryKind::Ordinary(TrackedInfo {
                        submodule: None,
                        mode_head: mode,
                        mode_index: mode,
                        mode_worktree: 0,
                        oid_head: oid,
                        oid_index: oid,
                    }),
                };
                self.worktree.remove(original);
                self.entries.push(entry);
            }
            let (mode_head, oid_head) = head_tree.get(&saved.path).copied().unwrap_or((0, ZERO));
            let absent = info.mode_worktree == 0 || saved.worktree == Change::Deleted;
            let mut next = TrackedInfo {
                submodule: info.submodule,
                mode_head,
                mode_index: mode_head,
                mode_worktree: if absent { 0 } else { info.mode_worktree },
                oid_head,
                oid_index: oid_head,
            };
            let worktree = if mode_head == 0 {
                if absent {
                    continue;
                }
                next.mode_index = info.mode_worktree;
                next.oid_index = match &content {
                    Some(c) => self.store_blob(c.clone()),
                    None => MemoryBackend::fake_oid(u64::MAX),
                };
                Change::Unmodified
            } else if absent {
                Change::Deleted
            } else if file_type(info.mode_worktree) != file_type(mode_head) {
                Change::TypeChanged
            } else if info.mode_worktree == mode_head
                && content.as_ref() == self.blobs.get(&oid_head)
            {
                Change::Unmodified
            } else {
                Change::Modified
            };
            let entry = StatusEntry {
                path: saved.path.clone(),
                index: Change::Unmodified,
                worktree,
                kind: EntryKind::Ordinary(next.clone()),
            };
            self.entries.extend(self.finish(entry, next));
        }
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        Ok(StashApplied {
            conflicts,
            warnings: Vec::new(),
        })
    }

    /// Simulates a `git stash drop` that fails (with `Some`): to test a pop that applies the
    /// stash and then fails to remove it.
    pub fn fail_stash_drop(&mut self, message: Option<&str>) {
        self.stash_drop_failure = message.map(str::to_owned);
    }

    /// [`GitStash::stash_drop`]: the backup goes in `commit_backups`
    /// (`refs/project/backup/<kind>-<n>`).
    pub(super) fn drop_stash(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError> {
        let position = self.stash_position(request.command, &request.id)?;
        if let Some(message) = &self.stash_drop_failure {
            return Err(EngineError::Git {
                command: "stash".to_owned(),
                stderr: message.clone(),
            });
        }
        let backup = request.backup.map(|kind| {
            let prefix = format!("{BACKUP_PREFIX}{kind}-");
            let n = self
                .commit_backups
                .iter()
                .filter(|(name, _)| name.starts_with(&prefix))
                .count();
            let name = format!("{prefix}{}", n + 1);
            self.commit_backups.push((name.clone(), request.id));
            name
        });
        let stash = self.stashes.remove(position);
        self.dropped_stashes.insert(stash.id, stash);
        self.sync_stash_ref();
        Ok(StashDropped {
            backup,
            warnings: Vec::new(),
        })
    }

    /// [`GitStash::stash_store`]: a dropped stash comes back on top with its files; any
    /// commit as a stash without files.
    pub(super) fn store_stash(&mut self, id: &Oid, message: &str) -> Result<(), EngineError> {
        let mut stash = match self.dropped_stashes.get(id).cloned() {
            Some(stash) => stash,
            None => {
                let commit = self
                    .commits
                    .get(id)
                    .ok_or_else(|| EngineError::GitWithReason {
                        command: "stash".to_owned(),
                        reason: msg!("engine-memory-not-a-commit", id = id),
                    })?;
                MemoryStash {
                    id: *id,
                    parents: commit.parents.clone(),
                    message: String::new(),
                    time: commit.committer.time,
                    saved: Vec::new(),
                    files: BTreeMap::new(),
                }
            }
        };
        stash.message = message.to_owned();
        self.stashes.insert(0, stash);
        self.sync_stash_ref();
        Ok(())
    }
}

#[cfg(test)]
#[path = "stash_tests.rs"]
mod tests;
