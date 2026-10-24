//! Writes of the [`MemoryBackend`]: stage and unstage on the status entries.
//!
//! The index is not kept separately: it is described by the status entries ([`StatusEntry`]), which
//! have for each file the modes and ids in HEAD and in the index, like `git status --porcelain=v2`.
//! Stage and unstage update those entries, the blobs and the worktree files, so `repo.status`,
//! `diff.index` and `diff.worktree` immediately see the new state. Every operation works on a
//! copy and applies it only if all paths are valid: an error changes nothing.

use super::{MemoryBackend, content_oid};
use crate::git::{
    BACKUP_PREFIX, GitBackend, LineSelection, apply_patch, line_patch, versions_at_risk,
};
use crate::{EngineError, path_matches as matches};
use project_domain::{
    Change, ChangedFile, EntryKind, Oid, RepoPath, StatusEntry, TrackedInfo, UnmergedInfo,
};
use std::collections::{BTreeMap, BTreeSet};

/// Mode and id of a file in a tree.
pub(super) type TreeEntry = (u32, Oid);

pub(super) const ZERO: Oid = Oid::from_bytes([0; Oid::LEN]);

/// The object type of a mode (file, link, submodule), without the permissions.
pub(super) fn file_type(mode: u32) -> u32 {
    mode & 0o170000
}

/// Order of the entries of `git status --porcelain=v2`: tracked and conflicted files by
/// path, then the untracked, then the ignored.
pub(super) fn entry_order(entry: &StatusEntry) -> (u8, &RepoPath) {
    let group = match entry.kind {
        EntryKind::Untracked => 1,
        EntryKind::Ignored => 2,
        _ => 0,
    };
    (group, &entry.path)
}

pub(super) fn tracked_info(entry: &StatusEntry) -> Option<&TrackedInfo> {
    match &entry.kind {
        EntryKind::Ordinary(info)
        | EntryKind::Renamed { info, .. }
        | EntryKind::Copied { info, .. } => Some(info),
        _ => None,
    }
}

fn untracked(path: RepoPath) -> StatusEntry {
    StatusEntry {
        path,
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    }
}

impl MemoryBackend {
    /// The tree of commit `id`: the files changed along the chain of first parents, applied
    /// from the root (each commit has the files changed relative to the first parent).
    pub(super) fn tree_at(&self, id: Oid) -> BTreeMap<RepoPath, TreeEntry> {
        let mut chain = Vec::new();
        let mut next = Some(id);
        while let Some(id) = next {
            let Some(commit) = self.commits.get(&id) else {
                break;
            };
            chain.push(id);
            next = commit.parents.first().copied();
        }
        let mut tree = BTreeMap::new();
        for id in chain.iter().rev() {
            for file in self.changes.get(id).into_iter().flatten() {
                apply_change(&mut tree, file);
            }
        }
        tree
    }

    pub(super) fn head_tree(&self) -> BTreeMap<RepoPath, TreeEntry> {
        self.head_target()
            .map(|id| self.tree_at(id))
            .unwrap_or_default()
    }

    /// True if the blobs `a` and `b` have the same content (same id or known equal contents).
    fn same_blob(&self, a: Oid, b: Oid) -> bool {
        a == b || matches!((self.blobs.get(&a), self.blobs.get(&b)), (Some(x), Some(y)) if x == y)
    }

    /// True if the worktree file `path` has the content of blob `oid` (false if either
    /// is not known).
    fn worktree_is(&self, path: &RepoPath, oid: Oid) -> bool {
        matches!(
            (self.worktree.get(path), self.blobs.get(&oid)),
            (Some(w), Some(b)) if w == b
        )
    }

    /// Saves `content` as a blob and returns its id.
    pub(super) fn store_blob(&mut self, content: Vec<u8>) -> Oid {
        let id = content_oid(&content);
        self.blobs.insert(id, content);
        id
    }

    /// Id in the index of the staged worktree file `path`.
    fn stage_content(&mut self, path: &RepoPath) -> Oid {
        match self.worktree.get(path).cloned() {
            Some(content) => self.store_blob(content),
            // Unknown content: a fixed id, the diff will show it as binary.
            None => MemoryBackend::fake_oid(u64::MAX),
        }
    }

    /// Before changing the index of a file unmodified in the worktree, it copies its
    /// content from the index to the worktree: afterwards, the worktree must stay as it was.
    fn keep_worktree(&mut self, entry: &StatusEntry, index_mode: u32, index_oid: Oid) {
        if entry.worktree == Change::Unmodified
            && index_mode != 0
            && !self.worktree.contains_key(&entry.path)
            && let Some(content) = self.blobs.get(&index_oid).cloned()
        {
            self.worktree.insert(entry.path.clone(), content);
        }
    }

    /// The entries matching `wanted`, or an error like git's if `wanted` is
    /// neither among the entries nor in HEAD. `in_head` says whether there is at least one HEAD file under `wanted`.
    fn hits(
        entries: &[StatusEntry],
        head: &BTreeMap<RepoPath, TreeEntry>,
        wanted: &RepoPath,
        not_found: impl FnOnce() -> EngineError,
    ) -> Result<Vec<usize>, EngineError> {
        let hits: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches(&e.path, wanted))
            .map(|(i, _)| i)
            .collect();
        let in_head = head.keys().any(|p| matches(p, wanted));
        if hits.is_empty() && !in_head {
            return Err(not_found());
        }
        Ok(hits)
    }

    /// [`GitBackend::stage`](crate::GitBackend::stage) on the status entries.
    /// Saves in a (in-memory) backup the staged versions that stage or unstage of `paths`
    /// would lose; the ref name, if there were any.
    fn backup(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        let versions = versions_at_risk(&self.status()?, paths);
        if versions.is_empty() {
            return Ok(None);
        }
        let name = format!("{BACKUP_PREFIX}index-{}", self.backups.len() + 1);
        self.backups.push((name.clone(), versions));
        Ok(Some(name))
    }

    pub(super) fn stage_paths(
        &mut self,
        paths: &[RepoPath],
    ) -> Result<Option<String>, EngineError> {
        let head = self.head_tree();
        let mut selected = BTreeSet::new();
        let mut ignored = Vec::new();
        let mut conflicts = Vec::new();
        for wanted in paths {
            let hits = Self::hits(&self.entries, &head, wanted, || EngineError::Git {
                command: "add".to_owned(),
                stderr: format!("fatal: pathspec '{wanted}' did not match any files"),
            })?;
            for i in hits {
                let entry = &self.entries[i];
                match entry.kind {
                    // An ignored file is rejected only if requested by name; inside a
                    // folder it is skipped, as git does.
                    EntryKind::Ignored if entry.path == *wanted => ignored.push(wanted.clone()),
                    // A conflict is resolved only by requesting it by name, not by mistake with
                    // its folder.
                    EntryKind::Unmerged(_) if !paths.contains(&entry.path) => {
                        conflicts.push(entry.path.clone());
                    }
                    EntryKind::Ignored => {}
                    _ => {
                        selected.insert(i);
                    }
                }
            }
        }
        if !ignored.is_empty() {
            return Err(EngineError::ignored(&ignored));
        }
        if !conflicts.is_empty() {
            return Err(EngineError::unmerged_in_folder(&conflicts));
        }
        let backup = self.backup(paths)?;

        let entries = std::mem::take(&mut self.entries);
        let mut next = Vec::with_capacity(entries.len());
        for (i, entry) in entries.into_iter().enumerate() {
            if selected.contains(&i) {
                next.extend(self.stage_entry(entry));
            } else {
                next.push(entry);
            }
        }
        next.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        self.entries = next;
        Ok(backup)
    }

    /// The entry after staging the file; `None` if the file no longer changed.
    fn stage_entry(&mut self, mut entry: StatusEntry) -> Option<StatusEntry> {
        match &entry.kind {
            EntryKind::Untracked => {
                let oid = self.stage_content(&entry.path);
                let info = TrackedInfo {
                    submodule: None,
                    mode_head: 0,
                    mode_index: FILE_MODE,
                    mode_worktree: FILE_MODE,
                    oid_head: ZERO,
                    oid_index: oid,
                };
                Some(StatusEntry {
                    index: Change::Added,
                    worktree: Change::Unmodified,
                    kind: EntryKind::Ordinary(info),
                    path: entry.path,
                })
            }
            EntryKind::Unmerged(u) => {
                let u = u.clone();
                self.resolve_conflict(entry.path, &u)
            }
            EntryKind::Ignored => Some(entry),
            EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. } => {
                let mut info = tracked_info(&entry).cloned()?;
                match entry.worktree {
                    Change::Unmodified => return Some(entry),
                    Change::Deleted => {
                        info.mode_index = 0;
                        info.oid_index = ZERO;
                    }
                    _ => {
                        info.oid_index = self.stage_content(&entry.path);
                        info.mode_index = info.mode_worktree;
                    }
                }
                entry.worktree = Change::Unmodified;
                self.finish(entry, info)
            }
        }
    }

    /// A conflicted file that is staged: resolved with the worktree content and
    /// compared with "ours" (HEAD).
    fn resolve_conflict(&mut self, path: RepoPath, u: &UnmergedInfo) -> Option<StatusEntry> {
        let (mode_index, oid_index) = if u.mode_worktree == 0 {
            (0, ZERO)
        } else {
            (u.mode_worktree, self.stage_content(&path))
        };
        let info = TrackedInfo {
            submodule: u.submodule,
            mode_head: u.mode_ours,
            mode_index,
            mode_worktree: u.mode_worktree,
            oid_head: u.oid_ours,
            oid_index,
        };
        let entry = StatusEntry {
            path,
            index: Change::Unmodified,
            worktree: Change::Unmodified,
            kind: EntryKind::Ordinary(info.clone()),
        };
        self.finish(entry, info)
    }

    /// Updates the entry with the new index data and recomputes the staged change;
    /// `None` if the file no longer changed, neither staged nor unstaged.
    pub(super) fn finish(&self, mut entry: StatusEntry, info: TrackedInfo) -> Option<StatusEntry> {
        let index = if info.mode_index == 0 {
            if info.mode_head == 0 {
                // Added and then deleted: for git it no longer exists.
                return None;
            }
            Change::Deleted
        } else if info.mode_head == 0 {
            Change::Added
        } else if file_type(info.mode_head) != file_type(info.mode_index) {
            Change::TypeChanged
        } else if info.mode_head == info.mode_index && self.same_blob(info.oid_head, info.oid_index)
        {
            Change::Unmodified
        } else {
            Change::Modified
        };
        if index == Change::Unmodified && entry.worktree == Change::Unmodified {
            return None;
        }
        entry.kind = match entry.kind {
            EntryKind::Renamed {
                original, score, ..
            } if index != Change::Unmodified => {
                entry.index = Change::Renamed;
                EntryKind::Renamed {
                    info,
                    original,
                    score,
                }
            }
            EntryKind::Copied {
                original, score, ..
            } if index != Change::Unmodified => {
                entry.index = Change::Copied;
                EntryKind::Copied {
                    info,
                    original,
                    score,
                }
            }
            _ => {
                entry.index = index;
                EntryKind::Ordinary(info)
            }
        };
        Some(entry)
    }

    /// [`GitBackend::unstage`](crate::GitBackend::unstage) on the status entries. The
    /// worktree does not change (if needed, the content of an unmodified file is copied
    /// from the index to the worktree, which for the `MemoryBackend` is just a map).
    pub(super) fn unstage_paths(
        &mut self,
        paths: &[RepoPath],
    ) -> Result<Option<String>, EngineError> {
        let head = self.head_tree();
        // A rename or copy touched by one of the paths becomes two entries (deletion of the
        // old and addition of the new), so each of the two is unstaged on its own.
        let mut entries = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            match &entry.kind {
                EntryKind::Renamed { original, .. } | EntryKind::Copied { original, .. }
                    if paths
                        .iter()
                        .any(|p| matches(&entry.path, p) || matches(original, p)) =>
                {
                    entries.extend(split(entry));
                }
                _ => entries.push(entry.clone()),
            }
        }

        let mut selected = BTreeSet::new();
        let mut conflicts = Vec::new();
        for wanted in paths {
            let hits = Self::hits(&entries, &head, wanted, || EngineError::Git {
                command: "restore".to_owned(),
                stderr: format!(
                    "error: pathspec '{wanted}' did not match any file(s) known to git"
                ),
            })?;
            let mut known = head.keys().any(|p| matches(p, wanted));
            for i in hits {
                match entries[i].kind {
                    EntryKind::Unmerged(_) => {
                        conflicts.push(entries[i].path.clone());
                        known = true;
                    }
                    EntryKind::Untracked | EntryKind::Ignored => {}
                    _ => {
                        selected.insert(i);
                        known = true;
                    }
                }
            }
            if !known {
                return Err(EngineError::Git {
                    command: "restore".to_owned(),
                    stderr: format!(
                        "error: pathspec '{wanted}' did not match any file(s) known to git"
                    ),
                });
            }
        }
        if !conflicts.is_empty() {
            return Err(EngineError::unmerged(&conflicts));
        }
        let backup = self.backup(paths)?;

        let mut next = Vec::with_capacity(entries.len());
        for (i, entry) in entries.into_iter().enumerate() {
            if selected.contains(&i) {
                next.extend(self.unstage_entry(entry));
            } else {
                next.push(entry);
            }
        }
        next.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        self.entries = next;
        Ok(backup)
    }

    /// The entry after unstaging the file (a tracked file, not conflicted).
    fn unstage_entry(&mut self, mut entry: StatusEntry) -> Option<StatusEntry> {
        let mut info = tracked_info(&entry).cloned()?;
        if !entry.index.is_change() {
            return Some(entry);
        }
        self.keep_worktree(&entry, info.mode_index, info.oid_index);
        let absent = entry.worktree == Change::Deleted || info.mode_worktree == 0;
        if info.mode_head == 0 {
            // It was not in HEAD: it becomes untracked again, if it is still in the worktree.
            return (!absent).then(|| untracked(entry.path));
        }
        info.mode_index = info.mode_head;
        info.oid_index = info.oid_head;
        entry.worktree = if absent {
            info.mode_worktree = 0;
            Change::Deleted
        } else if info.mode_worktree == info.mode_head
            && self.worktree_is(&entry.path, info.oid_head)
        {
            Change::Unmodified
        } else if file_type(info.mode_worktree) != file_type(info.mode_head) {
            Change::TypeChanged
        } else {
            Change::Modified
        };
        entry.kind = EntryKind::Ordinary(info.clone());
        self.finish(entry, info)
    }
}

impl MemoryBackend {
    /// [`GitBackend::apply_lines`](crate::GitBackend::apply_lines): the patch of the chosen lines
    /// applied to the file's staged content, then state and diff recomputed as after a
    /// stage. The worktree does not change.
    pub(super) fn apply_line_patch(
        &mut self,
        selection: &LineSelection,
    ) -> Result<Option<String>, EngineError> {
        let diff = self.diff(&selection.diff_spec())?;
        let prepared = line_patch(&diff, selection)?;
        let target = prepared.target().clone();
        let not_in_index = || EngineError::Git {
            command: "apply".to_owned(),
            stderr: format!("error: {target}: does not exist in index"),
        };
        let i = self
            .entries
            .iter()
            .position(|e| e.path == target && tracked_info(e).is_some())
            .ok_or_else(not_in_index)?;
        let entry = self.entries[i].clone();
        let info = tracked_info(&entry).cloned().ok_or_else(not_in_index)?;
        // Without a staged version (`intent-to-add` file, or deleted in the stage) the patch creates the
        // file.
        let base = (info.mode_index != 0)
            .then(|| self.blobs.get(&info.oid_index).cloned())
            .flatten();
        if info.mode_index != 0 && base.is_none() {
            return Err(EngineError::GitWithReason {
                command: "apply".to_owned(),
                reason: msg!("engine-memory-unknown-staged", target = target),
            });
        }
        let content = apply_patch(base.as_deref(), &prepared.patch).map_err(|e| {
            EngineError::NotAllowed(msg!(
                "engine-lines-patch-failed",
                target = target,
                error = e
            ))
        })?;
        let backup = self.backup(&prepared.touched(selection))?;

        self.keep_worktree(&entry, info.mode_index, info.oid_index);
        let mut info = info;
        match content {
            Some(content) => {
                if info.mode_index == 0 {
                    info.mode_index = prepared
                        .patch
                        .new_mode
                        .or(Some(info.mode_worktree).filter(|m| *m != 0))
                        .unwrap_or(FILE_MODE);
                }
                // As in git, the same content has the same id: if it comes back as in HEAD, the id
                // of HEAD (the `MemoryBackend` ids are fake and do not come from the content).
                info.oid_index = if self.blobs.get(&info.oid_head) == Some(&content) {
                    info.oid_head
                } else {
                    self.store_blob(content)
                };
            }
            None => {
                info.mode_index = 0;
                info.oid_index = ZERO;
            }
        }
        let mut entry = entry;
        let mut extra = None;
        let absent = entry.worktree == Change::Deleted || info.mode_worktree == 0;
        entry.worktree = if absent {
            info.mode_worktree = 0;
            if info.mode_index == 0 {
                Change::Unmodified
            } else {
                Change::Deleted
            }
        } else if info.mode_index == 0 {
            if info.mode_head == 0 {
                // Left the index and was never in HEAD: it becomes untracked again.
                self.entries[i] = untracked(entry.path);
                self.entries
                    .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
                return Ok(backup);
            }
            // Deleted in the stage but still in the worktree: also untracked, as for git.
            extra = Some(untracked(entry.path.clone()));
            info.mode_worktree = 0;
            Change::Unmodified
        } else if info.mode_worktree == info.mode_index
            && self.worktree_is(&entry.path, info.oid_index)
        {
            Change::Unmodified
        } else if file_type(info.mode_worktree) != file_type(info.mode_index) {
            Change::TypeChanged
        } else {
            Change::Modified
        };
        let next = self.finish(entry, info);
        self.entries.remove(i);
        self.entries.extend(next);
        self.entries.extend(extra);
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        Ok(backup)
    }
}

const FILE_MODE: u32 = 0o100644;

/// A staged rename (or copy) as two entries: the old path deleted in the stage (only
/// for the rename) and the new one added.
fn split(entry: &StatusEntry) -> Vec<StatusEntry> {
    let (info, original, renamed) = match &entry.kind {
        EntryKind::Renamed { info, original, .. } => (info, original, true),
        EntryKind::Copied { info, original, .. } => (info, original, false),
        _ => return vec![entry.clone()],
    };
    let mut out = Vec::with_capacity(2);
    if renamed {
        out.push(StatusEntry {
            path: original.clone(),
            index: Change::Deleted,
            worktree: Change::Unmodified,
            kind: EntryKind::Ordinary(TrackedInfo {
                submodule: info.submodule,
                mode_head: info.mode_head,
                mode_index: 0,
                mode_worktree: 0,
                oid_head: info.oid_head,
                oid_index: ZERO,
            }),
        });
    }
    out.push(StatusEntry {
        path: entry.path.clone(),
        index: Change::Added,
        worktree: entry.worktree,
        kind: EntryKind::Ordinary(TrackedInfo {
            mode_head: 0,
            oid_head: ZERO,
            ..info.clone()
        }),
    });
    out
}

/// Applies to a tree a file changed by a commit.
fn apply_change(tree: &mut BTreeMap<RepoPath, TreeEntry>, file: &ChangedFile) {
    if file.change == Change::Renamed {
        tree.remove(file.old_path());
    }
    if file.new_mode == 0 {
        tree.remove(&file.path);
    } else {
        tree.insert(file.path.clone(), (file.new_mode, file.new_oid));
    }
}

#[cfg(test)]
#[path = "write_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "lines_tests.rs"]
mod lines_tests;
