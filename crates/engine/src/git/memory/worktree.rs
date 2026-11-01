//! Linked worktrees on the [`MemoryBackend`] (`impl-docs/git/worktrees.md`): a minimal model.
//! The main worktree is the repo itself (at [`MEMORY_MAIN_WORKTREE`]); the linked ones are entries with
//! the contents the test sets ([`MemoryBackend::add_linked_worktree`]). A relative path is
//! joined to [`MEMORY_MAIN_WORKTREE`] without normalization. A branch checked out in a linked worktree
//! is not deleted (`branch.delete`, undo of `worktree.add`), as with git.

use super::{HEADS, HeadRef, MemoryBackend};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, WORKTREE_BACKUP_KIND, WorktreeAdd, WorktreeAdded, WorktreeContents,
    WorktreeInfo, WorktreeRemove, WorktreeRemoved, WorktreeTarget, branch_in_worktree,
    destination_not_empty, removal_needs_confirm, removal_refusal, unknown_worktree,
};
use project_domain::Oid;

/// Path of the main worktree of every in-memory repo.
pub const MEMORY_MAIN_WORKTREE: &str = "/repo";

/// A linked worktree of the in-memory repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWorktree {
    pub info: WorktreeInfo,
    /// What it holds; `None`: its folder is gone (prunable).
    pub contents: Option<WorktreeContents>,
    /// Its detached HEAD has commits on no ref: `worktree.remove` saves a backup.
    pub unreferenced: bool,
}

impl MemoryBackend {
    /// Adds a linked worktree as it is (the test sets branch, lock, contents...).
    pub fn add_linked_worktree(&mut self, worktree: MemoryWorktree) {
        self.linked_worktrees.push(worktree);
    }

    /// The linked worktrees, in creation order.
    pub fn linked_worktrees(&self) -> &[MemoryWorktree] {
        &self.linked_worktrees
    }

    /// A path that exists and is not empty: `worktree.add` refuses it.
    pub fn occupy_path(&mut self, path: &str) {
        self.occupied_paths.insert(path.to_owned());
    }

    /// The linked worktree whose current branch is `full`, if any.
    pub(super) fn worktree_of_branch(&self, full: &str) -> Option<&WorktreeInfo> {
        self.linked_worktrees
            .iter()
            .map(|w| &w.info)
            .find(|w| w.branch.as_deref() == Some(full))
    }

    pub(super) fn memory_worktrees(&self) -> Vec<WorktreeInfo> {
        let (branch, head) = match &self.head {
            HeadRef::Branch(name) => {
                let full = format!("{HEADS}{name}");
                let head = self.refs.get(&full).map(|r| r.target);
                (Some(full), head)
            }
            HeadRef::Detached(oid) => (None, Some(*oid)),
        };
        let main = WorktreeInfo {
            path: MEMORY_MAIN_WORKTREE.to_owned(),
            main: true,
            current: true,
            bare: false,
            detached: branch.is_none(),
            branch,
            head,
            locked: false,
            lock_reason: None,
            prunable: false,
        };
        let linked = self.linked_worktrees.iter().map(|w| {
            let mut info = w.info.clone();
            if let Some(full) = &info.branch {
                info.head = self.refs.get(full).map(|r| r.target);
            }
            info.prunable = w.contents.is_none() && !info.locked;
            info
        });
        std::iter::once(main).chain(linked).collect()
    }

    pub(super) fn memory_add_worktree(
        &mut self,
        request: &WorktreeAdd,
    ) -> Result<WorktreeAdded, EngineError> {
        request.check()?;
        let path = if request.path.starts_with('/') {
            request.path.clone()
        } else {
            format!("{MEMORY_MAIN_WORKTREE}/{}", request.path)
        };
        let taken = path == MEMORY_MAIN_WORKTREE
            || self.occupied_paths.contains(&path)
            || self.linked_worktrees.iter().any(|w| w.info.path == path);
        if taken {
            return Err(destination_not_empty(&request.path));
        }
        let (branch, head) = match &request.target {
            WorktreeTarget::Branch(name) => {
                let full = format!("{HEADS}{name}");
                let Some(target) = self.refs.get(&full).map(|r| r.target) else {
                    return Err(EngineError::UnknownRev(name.clone()));
                };
                if let Some(w) = self
                    .memory_worktrees()
                    .into_iter()
                    .find(|w| w.branch.as_deref() == Some(full.as_str()))
                {
                    return Err(branch_in_worktree(name, &w.path));
                }
                (Some(full), Some(target))
            }
            WorktreeTarget::Detached(oid) => {
                if !self.commits.contains_key(oid) {
                    return Err(EngineError::UnknownRev(oid.to_string()));
                }
                (None, Some(*oid))
            }
        };
        self.linked_worktrees.push(MemoryWorktree {
            info: WorktreeInfo {
                path: path.clone(),
                main: false,
                current: false,
                bare: false,
                detached: branch.is_none(),
                branch,
                head,
                locked: false,
                lock_reason: None,
                prunable: false,
            },
            contents: Some(WorktreeContents::default()),
            unreferenced: false,
        });
        Ok(WorktreeAdded {
            path,
            messages: Vec::new(),
            warnings: Vec::new(),
        })
    }

    pub(super) fn memory_remove_worktree(
        &mut self,
        request: &WorktreeRemove,
    ) -> Result<WorktreeRemoved, EngineError> {
        let listed = self.memory_worktrees();
        let Some(info) = listed.iter().find(|w| w.path == request.path) else {
            return Err(unknown_worktree(&request.path));
        };
        let linked = self
            .linked_worktrees
            .iter()
            .position(|w| w.info.path == info.path);
        let contents = linked.and_then(|i| self.linked_worktrees[i].contents.as_ref());
        if let Some(reason) = removal_refusal(info, contents) {
            return Err(EngineError::NotAllowed(reason));
        }
        let index = linked.expect("only linked worktrees pass the refusals");
        let worktree = &self.linked_worktrees[index];
        let saved: Option<Oid> = (worktree.unreferenced && worktree.info.detached)
            .then_some(worktree.info.head)
            .flatten();
        if !request.confirm {
            return Err(removal_needs_confirm(&info.path, saved.is_some()));
        }
        let backup = saved.map(|oid| {
            let name = format!(
                "{BACKUP_PREFIX}{WORKTREE_BACKUP_KIND}-{}",
                self.commit_backups.len() + 1
            );
            self.commit_backups.push((name.clone(), oid));
            name
        });
        let removed = self.linked_worktrees.remove(index);
        Ok(WorktreeRemoved {
            path: removed.info.path,
            backup,
            messages: Vec::new(),
        })
    }
}
