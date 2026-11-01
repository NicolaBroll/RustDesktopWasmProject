//! Linked worktrees (`worktree.list`, `worktree.add`, `worktree.remove`,
//! `impl-docs/git/worktrees.md`): the types the backends exchange and the refusals both apply.
//! Not to be confused with the commands on the files of the working tree (`worktree.discard`,
//! `worktree.clean`, `worktree.ignore`), which share the `worktree.` prefix.

use super::clone::check_path;
use crate::{EngineError, Message};
use project_domain::{Oid, Operation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kind of the backup ref of a detached HEAD saved by `worktree.remove`:
/// `refs/project/backup/worktree-<time>`, in the repository shared by all worktrees.
pub const WORKTREE_BACKUP_KIND: &str = "worktree";

/// A worktree of the repository, as `worktree.list` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    /// Absolute path, as git records it (text: a non-UTF-8 path is shown with replacement
    /// characters and cannot be removed from here).
    pub path: String,
    /// The main worktree (the first one git lists): never removed.
    pub main: bool,
    /// The worktree open in this session.
    pub current: bool,
    /// The main entry of a bare repository (no files).
    pub bare: bool,
    /// The commit checked out; `null` on a branch without commits.
    pub head: Option<Oid>,
    /// The branch checked out, full name (`refs/heads/<name>`); `null` when detached.
    pub branch: Option<String>,
    pub detached: bool,
    /// Locked (`git worktree lock`): never removed from here.
    pub locked: bool,
    /// The reason given to `git worktree lock --reason`, if any.
    pub lock_reason: Option<String>,
    /// Its folder no longer exists: `git worktree prune` would drop the entry.
    pub prunable: bool,
}

/// What `worktree.add` checks out in the new worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeTarget {
    /// An existing local branch (short name), checked out nowhere else.
    Branch(String),
    /// A commit, on a detached HEAD.
    Detached(Oid),
}

/// The request [`GitWorktrees::add_worktree`](crate::GitWorktrees::add_worktree) receives: the
/// destination is checked by the backend (absent or empty folder, as `repo.clone`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeAdd {
    /// As given: absolute, or relative to the root of the open worktree.
    pub path: String,
    pub target: WorktreeTarget,
}

/// The worktree created by [`GitWorktrees::add_worktree`](crate::GitWorktrees::add_worktree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeAdded {
    /// Absolute path of the new worktree (what `repo.open` takes).
    pub path: String,
    /// git's lines (`Preparing worktree ...`, `HEAD is now at ...`).
    pub messages: Vec<String>,
    /// Something unexpected after a successful creation (a `post-checkout` hook that failed).
    pub warnings: Vec<Message>,
}

/// Parameters of `worktree.remove`, passed as they are to
/// [`GitWorktrees::remove_worktree`](crate::GitWorktrees::remove_worktree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorktreeRemove {
    /// The path of the worktree, as `worktree.list` gives it (or another spelling of the same
    /// folder).
    pub path: String,
    /// Required: the folder of the worktree is deleted.
    #[serde(default)]
    pub confirm: bool,
}

/// Result of `worktree.remove` (from the backend).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeRemoved {
    /// The path of the removed worktree, as git listed it.
    pub path: String,
    /// `refs/project/backup/worktree-...`: its detached HEAD had commits on no branch, tag or
    /// remote branch; `null` otherwise.
    pub backup: Option<String>,
    /// git's non-empty output lines.
    pub messages: Vec<String>,
}

/// What a linked worktree holds that `git worktree remove` would delete: the backends read it
/// inside the worktree, [`removal_refusal`] judges it. Paths are for display.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeContents {
    /// Merge, rebase, cherry-pick... in progress in that worktree.
    pub operation: Option<Operation>,
    /// Tracked files modified, staged or conflicted (submodules included).
    pub changes: Vec<String>,
    pub untracked: Vec<String>,
    /// Ignored files: git deletes them without asking.
    pub ignored: Vec<String>,
    /// Files marked `--assume-unchanged`, or `--skip-worktree` and present: git status does not
    /// say whether they changed.
    pub hidden: Vec<String>,
}

impl WorktreeAdd {
    /// [`check_worktree_path`].
    pub fn check(&self) -> Result<(), EngineError> {
        check_worktree_path(&self.path)
    }
}

/// [`EngineError::InvalidParams`] for an empty `worktree.add` path or one with a NUL or a
/// newline (git's worktree list is line based).
pub fn check_worktree_path(path: &str) -> Result<(), EngineError> {
    check_path("worktree.add", path)?;
    if path.contains('\n') {
        return Err(EngineError::InvalidParams {
            id: "worktree.add",
            reason: msg!("engine-worktree-path-newline"),
        });
    }
    Ok(())
}

/// The path matches no worktree of the repository.
pub fn unknown_worktree(path: &str) -> EngineError {
    EngineError::NotAllowed(msg!("engine-worktree-unknown", path = path))
}

/// The branch is already checked out in a worktree (git refuses it too).
pub fn branch_in_worktree(branch: &str, path: &str) -> EngineError {
    EngineError::NotAllowed(msg!(
        "engine-worktree-branch-in-use",
        branch = branch,
        path = path
    ))
}

/// Why `worktree.remove` refuses `info` (with `contents` read inside it; `None` when its folder
/// no longer exists), in this order: main, current, locked, operation, changes, untracked,
/// ignored, hidden files. `None`: it can be removed.
pub fn removal_refusal(
    info: &WorktreeInfo,
    contents: Option<&WorktreeContents>,
) -> Option<Message> {
    let path = info.path.as_str();
    if info.main {
        return Some(msg!("engine-worktree-main", path = path));
    }
    if info.current {
        return Some(msg!("engine-worktree-current", path = path));
    }
    if info.locked {
        return Some(
            match info.lock_reason.as_deref().filter(|r| !r.is_empty()) {
                Some(reason) => msg!(
                    "engine-worktree-locked-reason",
                    path = path,
                    reason = reason
                ),
                None => msg!("engine-worktree-locked", path = path),
            },
        );
    }
    let contents = contents?;
    if let Some(op) = contents.operation {
        return Some(msg!(
            "engine-worktree-operation",
            path = path,
            operation = format!("{op:?}")
        ));
    }
    let (kind, files) = [
        ("changes", &contents.changes),
        ("untracked", &contents.untracked),
        ("ignored", &contents.ignored),
        ("hidden", &contents.hidden),
    ]
    .into_iter()
    .find(|(_, files)| !files.is_empty())?;
    let (count, first) = (files.len(), files[0].as_str());
    Some(match kind {
        "changes" => msg!(
            "engine-worktree-changes",
            path = path,
            count = count,
            first = first
        ),
        "untracked" => msg!(
            "engine-worktree-untracked",
            path = path,
            count = count,
            first = first
        ),
        "ignored" => msg!(
            "engine-worktree-ignored",
            path = path,
            count = count,
            first = first
        ),
        _ => msg!(
            "engine-worktree-hidden",
            path = path,
            count = count,
            first = first
        ),
    })
}

/// `worktree.remove` without `confirm`: what it would do.
pub fn removal_needs_confirm(path: &str, backup: bool) -> EngineError {
    EngineError::ConfirmRequired {
        command: "worktree.remove",
        reason: msg!(
            "engine-worktree-remove-confirm",
            path = path,
            backup = if backup { "yes" } else { "no" }
        ),
        expected: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linked() -> WorktreeInfo {
        WorktreeInfo {
            path: "/w".into(),
            main: false,
            current: false,
            bare: false,
            head: None,
            branch: None,
            detached: true,
            locked: false,
            lock_reason: None,
            prunable: false,
        }
    }

    fn key(m: Option<Message>) -> Option<String> {
        m.map(|m| m.key().to_owned())
    }

    #[test]
    fn refusals_in_order() {
        let clean = WorktreeContents::default();
        assert_eq!(removal_refusal(&linked(), Some(&clean)), None);
        assert_eq!(removal_refusal(&linked(), None), None);
        let mut main = linked();
        main.main = true;
        main.locked = true;
        assert_eq!(
            key(removal_refusal(&main, None)).as_deref(),
            Some("engine-worktree-main")
        );
        let mut locked = linked();
        locked.locked = true;
        assert_eq!(
            key(removal_refusal(&locked, Some(&clean))).as_deref(),
            Some("engine-worktree-locked")
        );
        let full = WorktreeContents {
            operation: None,
            changes: vec![],
            untracked: vec!["a".into()],
            ignored: vec!["b".into()],
            hidden: vec![],
        };
        assert_eq!(
            key(removal_refusal(&linked(), Some(&full))).as_deref(),
            Some("engine-worktree-untracked")
        );
        let ignored = WorktreeContents {
            ignored: vec!["b".into()],
            ..Default::default()
        };
        let text = removal_refusal(&linked(), Some(&ignored)).unwrap();
        assert_eq!(text.key(), "engine-worktree-ignored");
        let op = WorktreeContents {
            operation: Some(Operation::Rebase),
            changes: vec!["x".into()],
            ..Default::default()
        };
        assert_eq!(
            key(removal_refusal(&linked(), Some(&op))).as_deref(),
            Some("engine-worktree-operation")
        );
    }

    #[test]
    fn add_checks_the_path() {
        let add = |p: &str| WorktreeAdd {
            path: p.into(),
            target: WorktreeTarget::Detached(Oid::from_bytes([1; Oid::LEN])),
        };
        assert!(add("../w").check().is_ok());
        assert!(add("").check().is_err());
        assert!(add("a\nb").check().is_err());
    }
}
