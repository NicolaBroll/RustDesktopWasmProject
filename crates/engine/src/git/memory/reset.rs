//! `branch.reset` and its undo/redo on the [`MemoryBackend`], with the same checks as the
//! `CliBackend` ([`reset_files`], [`strict_conflicts`], [`check_same_reset_files`]).
//!
//! The worktree backup of a `hard` is a [`WorktreeStash`] in
//! [`MemoryBackend::worktree_backups`] with the ref `refs/project/backup/reset-<n>`; the index
//! backup of a `mixed` is the one of stage and unstage (`refs/project/backup/index-<n>`). An
//! error changes nothing: the work is done on a copy.

use super::MemoryBackend;
use super::discard::{StashedFile, WorktreeStash};
use super::write::tracked_info;
use crate::git::{
    BACKUP_PREFIX, ResetMode, ResetOutcome, ResetRequest, check_same_reset_files, reset_files,
    reset_moved, status_paths, strict_conflicts,
};
use crate::{EngineError, GitRead};
use project_domain::{Change, EntryKind, RepoPath, StatusEntry};
use std::collections::{BTreeMap, BTreeSet};

impl MemoryBackend {
    /// [`GitRefs::reset`].
    pub(super) fn reset_head(
        &mut self,
        request: &ResetRequest,
    ) -> Result<ResetOutcome, EngineError> {
        let (_, _, target) = request.shape()?;
        request.refs.check(self.operation)?;
        let head = self.head_state();
        let conflicts = request
            .refs
            .conflicts(&head, |name| Ok(self.refs.get(name).map(|r| r.target)))?;
        if !conflicts.is_empty() {
            return Err(reset_moved(request, conflicts));
        }
        if !self.commits.contains_key(&target) {
            return Err(EngineError::UnknownRev(target.to_string()));
        }
        let status = self.status()?;
        if request.strict {
            let conflicts = strict_conflicts(&status, request.mode)?;
            if !conflicts.is_empty() {
                return Err(EngineError::UndoConflict {
                    command: request.refs.command,
                    refs: conflicts,
                });
            }
        }
        let new_tree = self.tree_at(target);
        let mut work = self.clone();
        let mut outcome = ResetOutcome::default();
        match request.mode {
            ResetMode::Soft => {
                outcome.backups = work.restore(&request.refs)?;
            }
            ResetMode::Mixed => {
                if !request.strict {
                    outcome.index_backup = work.backup(&status_paths(&status))?;
                }
                // The conflicts are dropped from the index (`read-tree --reset`): what is in the
                // worktree stays, as an unstaged change or an untracked file.
                let unmerged: Vec<RepoPath> = work
                    .entries
                    .iter()
                    .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
                    .map(|e| e.path.clone())
                    .collect();
                work.entries
                    .retain(|e| !matches!(e.kind, EntryKind::Unmerged(_)));
                outcome.backups = work.restore(&request.refs)?;
                work.rebase_entries(&new_tree);
                for path in unmerged {
                    match work
                        .entries
                        .iter_mut()
                        .find(|e| e.path == path && tracked_info(e).is_some())
                    {
                        Some(entry) => entry.worktree = Change::Modified,
                        None => work.entries.push(StatusEntry {
                            path,
                            index: Change::Untracked,
                            worktree: Change::Untracked,
                            kind: EntryKind::Untracked,
                        }),
                    }
                }
            }
            ResetMode::Hard => {
                let files = reset_files(&status)?;
                if !request.strict {
                    check_same_reset_files(&files, &request.files)?;
                }
                let untracked: BTreeSet<String> = self
                    .entries
                    .iter()
                    .filter(|e| matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored))
                    .map(|e| e.path.to_string())
                    .collect();
                let (_, hits) = self.switch_plan(&new_tree);
                let in_the_way: Vec<String> = hits
                    .into_iter()
                    .filter(|p| untracked.contains(p))
                    .map(|p| format!("`{p}`"))
                    .collect();
                if !in_the_way.is_empty() {
                    return Err(EngineError::NotAllowed(msg!(
                        "engine-reset-in-the-way",
                        files = in_the_way.join(", ")
                    )));
                }
                if !files.is_empty() {
                    let n = work.worktree_backups.len() as u64 + 1;
                    let stash = MemoryBackend::fake_oid(0x5354_4153_4800_0000 + n);
                    let saved: BTreeMap<RepoPath, StashedFile> = files
                        .iter()
                        .filter_map(|p| {
                            let entry = status.entries.iter().find(|e| e.path == *p)?.clone();
                            let content = match entry.worktree {
                                Change::Deleted => None,
                                _ => self.worktree.get(p).cloned(),
                            };
                            Some((p.clone(), StashedFile { entry, content }))
                        })
                        .collect();
                    let name = format!("{BACKUP_PREFIX}reset-{n}");
                    work.worktree_backups.push(WorktreeStash {
                        name: name.clone(),
                        stash,
                        files: saved,
                    });
                    outcome.backup = Some(name);
                    outcome.stash = Some(stash);
                }
                let old_index = work.index_tree();
                work.entries
                    .retain(|e| matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored));
                outcome.backups = work.restore(&request.refs)?;
                for path in old_index.keys().chain(new_tree.keys()).chain(&files) {
                    work.worktree.remove(path);
                }
                work.rebase_entries(&new_tree);
                outcome.files = files;
            }
        }
        *self = work;
        Ok(outcome)
    }
}

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;
