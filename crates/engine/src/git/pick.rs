//! Cherry-pick and revert of a commit (`commit.cherry_pick`, `commit.revert`,
//! `cherry_pick.continue`/`abort`, `revert.continue`/`abort`, S31): types and checks shared by the
//! backends (`impl-docs/git/cherry-pick.md`).
//!
//! The model is the S29 merge ([`super::merge`]): the command resolves the rev, picks the
//! parent (`mainline` for a merge commit) and checks the state ([`check_pick_state`]); the
//! backend saves the branch in a backup and executes
//! ([`GitOperations::pick`](super::GitOperations::pick)). **A conflict leaves the operation in
//! progress** (`CHERRY_PICK_HEAD`/`REVERT_HEAD`): it is concluded with `*.continue` (a commit with
//! [`CommitRequest::concluding`](super::CommitRequest::concluding)) or aborted with
//! `*.abort` ([`GitOperations::pick_abort`](super::GitOperations::pick_abort)), with a backup of
//! index and worktree first. An empty result (commit already applied) leaves nothing in
//! progress.

use super::{OperationState, check_no_operation, conflicted_paths};
use crate::{EngineError, Message};
use project_domain::{Commit, EntryKind, Oid, Operation, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// Cherry-pick or revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickKind {
    /// Applies the changes of the commit on top of HEAD (`git cherry-pick`), with its author.
    CherryPick,
    /// Applies the inverse changes of the commit (`git revert`).
    Revert,
}

impl PickKind {
    /// The operation in progress when it stops.
    pub fn operation(self) -> Operation {
        match self {
            PickKind::CherryPick => Operation::CherryPick,
            PickKind::Revert => Operation::Revert,
        }
    }

    /// The kind of the operation in progress, if it is a cherry-pick or a revert.
    pub fn from_operation(op: Operation) -> Option<Self> {
        match op {
            Operation::CherryPick => Some(PickKind::CherryPick),
            Operation::Revert => Some(PickKind::Revert),
            _ => None,
        }
    }

    /// The id of the command that starts the operation.
    pub fn command(self) -> &'static str {
        match self {
            PickKind::CherryPick => "commit.cherry_pick",
            PickKind::Revert => "commit.revert",
        }
    }

    /// The id of the command that concludes the operation in progress.
    pub fn continue_command(self) -> &'static str {
        match self {
            PickKind::CherryPick => "cherry_pick.continue",
            PickKind::Revert => "revert.continue",
        }
    }

    /// The id of the command that aborts the operation in progress.
    pub fn abort_command(self) -> &'static str {
        match self {
            PickKind::CherryPick => "cherry_pick.abort",
            PickKind::Revert => "revert.abort",
        }
    }

    /// The git subcommand, also used in the backup names (`cherry-pick-...`,
    /// `revert-abort-...`).
    pub fn git(self) -> &'static str {
        match self {
            PickKind::CherryPick => "cherry-pick",
            PickKind::Revert => "revert",
        }
    }

    /// The file in the git folder with the incoming commit.
    pub fn head_file(self) -> &'static str {
        match self {
            PickKind::CherryPick => "CHERRY_PICK_HEAD",
            PickKind::Revert => "REVERT_HEAD",
        }
    }

    /// The name for descriptions (`Cherry-pick`, `Revert`).
    pub fn title(self) -> &'static str {
        match self {
            PickKind::CherryPick => "Cherry-pick",
            PickKind::Revert => "Revert",
        }
    }
}

/// What `commit.cherry_pick`/`commit.revert` ask of the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRequest {
    pub kind: PickKind,
    /// The branch of HEAD (short name): the operation does not start with detached HEAD.
    pub branch: String,
    /// The value of the branch read earlier: the backend refuses if it changed.
    pub expected: Oid,
    /// The commit to apply or revert (the already resolved rev: git receives only the id).
    pub commit: Oid,
    /// The parent the changes are taken relative to: the one chosen with `mainline` for a
    /// merge commit, the only one otherwise, `None` for a root commit.
    pub parent: Option<Oid>,
    /// `-m <n>` for a merge commit (1 = first parent).
    pub mainline: Option<usize>,
    /// The commit message, as git writes it ([`pick_message`]): the `CliBackend` lets
    /// git write it (`--no-edit`), the `MemoryBackend` uses this one.
    pub message: String,
}

/// The parent chosen for `commit` with `mainline`: a merge commit requires it (1 = first
/// parent), a commit with a single parent (or root) does not. Errors as
/// [`EngineError::InvalidParams`] of `id`.
pub fn pick_parent(
    id: &'static str,
    commit: &Commit,
    mainline: Option<usize>,
) -> Result<Option<Oid>, EngineError> {
    let invalid = |reason: Message| EngineError::InvalidParams { id, reason };
    match (commit.parents.len(), mainline) {
        (0 | 1, None) => Ok(commit.parents.first().copied()),
        (0 | 1, Some(_)) => Err(invalid(msg!(
            "engine-pick-mainline-not-merge",
            commit = commit.id.short()
        ))),
        (n, None) => Err(invalid(msg!(
            "engine-pick-mainline-needed",
            commit = commit.id.short(),
            n = n
        ))),
        (n, Some(m)) if m == 0 || m > n => Err(invalid(msg!(
            "engine-pick-mainline-range",
            n = n,
            commit = commit.id.short(),
            m = m
        ))),
        (_, Some(m)) => Ok(Some(commit.parents[m - 1])),
    }
}

/// The commit message as git writes it with `--no-edit` (without `-x`): for a
/// cherry-pick that of the commit; for a revert `Revert "<subject>"` (or `Reapply "..."` for the
/// revert of a revert, as git since 2.43) and `This reverts commit <id>.` (with `, reversing
/// changes made to <parent>.` for a merge commit).
pub fn pick_message(kind: PickKind, commit: &Commit, parent: Option<&Oid>, merge: bool) -> String {
    match kind {
        PickKind::CherryPick => {
            if commit.body.is_empty() {
                commit.summary.clone()
            } else {
                format!("{}\n\n{}", commit.summary, commit.body)
            }
        }
        PickKind::Revert => {
            let title = match commit
                .summary
                .strip_prefix("Revert \"")
                .and_then(|s| s.strip_suffix('"'))
            {
                Some(inner) => format!("Reapply \"{inner}\""),
                None => format!("Revert \"{}\"", commit.summary),
            };
            match parent {
                Some(parent) if merge => format!(
                    "{title}\n\nThis reverts commit {}, reversing\nchanges made to {parent}.",
                    commit.id
                ),
                _ => format!("{title}\n\nThis reverts commit {}.", commit.id),
            }
        }
    }
}

/// The checks before `commit.cherry_pick`/`commit.revert`, the same for the backends, on the
/// state read now ([`EngineError::NotAllowed`], nothing changes):
///
/// - an operation in progress (even another cherry-pick);
/// - conflicted files;
/// - **any** change to tracked files (staged or not), as for a merge commit: a
///   cherry-pick stopped on conflict would mix local changes with its own, and the abort cannot
///   always rebuild them. Never autostash. Untracked files are fine.
pub fn check_pick_state(status: &Status, kind: PickKind) -> Result<(), EngineError> {
    let what = match kind {
        PickKind::CherryPick => msg!("engine-busy-cherry-pick"),
        PickKind::Revert => msg!("engine-busy-revert"),
    };
    check_no_operation(kind.command(), what, status.operation)?;
    let unmerged = conflicted_paths(status);
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-pick-conflicts",
            files = quoted(&unmerged),
            command = kind.command()
        )));
    }
    let changed: Vec<RepoPath> = status
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
            ) && (e.index.is_change() || e.worktree.is_change())
        })
        .map(|e| e.path.clone())
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    Err(EngineError::NotAllowed(msg!(
        "engine-pick-local-changes",
        files = quoted(&changed),
        command = kind.command()
    )))
}

/// The check common to `*.continue` and `*.abort`: an operation `kind` of a single commit
/// must be in progress. A sequencer with several commits started from the terminal (`.git/sequencer`,
/// [`OperationState::sequencer`]) is rejected: `git commit` would leave it half done and `git
/// cherry-pick --abort` would also roll back the commits of the sequence already applied.
pub fn check_picking(
    command: &str,
    kind: PickKind,
    status: &Status,
    state: &OperationState,
) -> Result<(), EngineError> {
    super::merge::check_in_progress(command, kind.operation(), status)?;
    if state.sequencer {
        return Err(EngineError::NotAllowed(msg!(
            "engine-pick-sequencer",
            command = command,
            git = kind.git()
        )));
    }
    Ok(())
}

/// The error of a cherry-pick or revert that would change nothing (commit already applied, or
/// already reverted), the same in both backends: [`EngineError::NotAllowed`].
pub fn pick_empty_error(kind: PickKind, commit: &Oid) -> EngineError {
    let what = match kind {
        PickKind::CherryPick => msg!("engine-pick-already-applied"),
        PickKind::Revert => msg!("engine-pick-already-reverted"),
    };
    EngineError::NotAllowed(msg!(
        "engine-pick-empty",
        command = kind.command(),
        commit = commit.short(),
        what = what
    ))
}

fn quoted(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::Signature;

    fn commit(parents: usize, summary: &str) -> Commit {
        let sig = Signature {
            name: "A".into(),
            email: "a@x".into(),
            time: 0,
            offset_min: 0,
        };
        Commit {
            id: Oid::from_bytes([9; Oid::LEN]),
            parents: (0..parents)
                .map(|i| Oid::from_bytes([i as u8 + 1; Oid::LEN]))
                .collect(),
            author: sig.clone(),
            committer: sig,
            summary: summary.into(),
            body: String::new(),
        }
    }

    #[test]
    fn mainline_rules() {
        let id = "commit.revert";
        assert!(pick_parent(id, &commit(0, "r"), None).unwrap().is_none());
        assert_eq!(
            pick_parent(id, &commit(1, "x"), None).unwrap(),
            Some(Oid::from_bytes([1; Oid::LEN]))
        );
        assert_eq!(
            pick_parent(id, &commit(2, "m"), Some(2)).unwrap(),
            Some(Oid::from_bytes([2; Oid::LEN]))
        );
        for (parents, mainline) in [(1, Some(1)), (2, None), (2, Some(0)), (2, Some(3))] {
            let err = pick_parent(id, &commit(parents, "c"), mainline).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{parents} {mainline:?}: {err}"
            );
        }
    }

    #[test]
    fn messages_like_git() {
        let c = commit(1, "Aggiunge a");
        let id = c.id;
        assert_eq!(
            pick_message(PickKind::Revert, &c, c.parents.first(), false),
            format!("Revert \"Aggiunge a\"\n\nThis reverts commit {id}.")
        );
        let r = commit(1, "Revert \"Aggiunge a\"");
        assert!(
            pick_message(PickKind::Revert, &r, None, false).starts_with("Reapply \"Aggiunge a\"")
        );
        let m = commit(2, "Merge");
        let p = m.parents[0];
        assert_eq!(
            pick_message(PickKind::Revert, &m, Some(&p), true),
            format!(
                "Revert \"Merge\"\n\nThis reverts commit {id}, reversing\nchanges made to {p}."
            )
        );
        let mut c = commit(1, "Title");
        c.body = "Body".into();
        assert_eq!(
            pick_message(PickKind::CherryPick, &c, None, false),
            "Title\n\nBody"
        );
    }
}
