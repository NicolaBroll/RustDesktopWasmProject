//! Reset of the current branch (`branch.reset`, `impl-docs/git/reset.md`).
//!
//! `branch.reset {"target", "mode"}` moves the branch of HEAD (or a detached HEAD) to another
//! commit, like `git reset --soft|--mixed|--hard`. The types and the checks shared by the
//! backends are here, so `MemoryBackend` and `CliBackend` refuse the same cases:
//!
//! - **soft** moves only the ref: index and worktree stay.
//! - **mixed** also rewrites the index as the target's tree; the worktree stays. The staged
//!   versions that exist only in the index are saved first in `refs/project/backup/index-...`,
//!   as stage and unstage do on their own.
//! - **hard** also rewrites the worktree of the tracked files. The local changes it would
//!   overwrite ([`reset_files`]) are saved first with `git stash create` (and byte for byte, as
//!   `worktree.discard` does) in `refs/project/backup/reset-...`; if the backup fails nothing is
//!   written. Untracked and ignored files are never touched: if one is where a file of the
//!   target would go, the reset is refused.
//!
//! The ref move goes through the same checks as undo and redo ([`RefRestore`]): HEAD and the
//! branch must still hold the values read by the command (the write passes them to git as
//! the old values), and the commits that would no longer be reachable get a backup ref.

use crate::{EngineError, Message, MessageArg, RefRestore};
use project_domain::{Change, EntryKind, Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// The three modes of `git reset`.
///
/// JSON: `"soft"`, `"mixed"`, `"hard"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetMode {
    /// Only the ref.
    Soft,
    /// The ref and the index.
    Mixed,
    /// The ref, the index and the worktree of the tracked files.
    Hard,
}

impl ResetMode {
    /// The name used in JSON, in the reflog and in the messages.
    pub fn as_str(self) -> &'static str {
        match self {
            ResetMode::Soft => "soft",
            ResetMode::Mixed => "mixed",
            ResetMode::Hard => "hard",
        }
    }
}

/// The reset in a journal entry: the mode (undo and redo use it again) and, for a `hard`
/// with local changes, the commit of `git stash create` that holds them (undo does not put
/// them back: it says where they are).
///
/// JSON: `{"mode", "stash": oid | null}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetRecord {
    pub mode: ResetMode,
    #[serde(default)]
    pub stash: Option<Oid>,
}

/// What `branch.reset` (or the undo/redo of one) asks of
/// [`GitBackend::reset`](crate::GitBackend::reset).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetRequest {
    pub mode: ResetMode,
    /// The ref move: with HEAD on a branch a single [`RefMove`](crate::RefMove) of that branch
    /// (expected value = the commit read by the command) and HEAD the same before and after;
    /// with detached HEAD no refs, and HEAD from the commit read to the target. `command` is
    /// `branch.reset`, `edit.undo` or `edit.redo`.
    pub refs: RefRestore,
    /// Only for `hard`: the confirmed plan ([`reset_files`] on the state read by the command). If
    /// the plan read by the backend before writing is different, [`EngineError::NotAllowed`] and
    /// nothing changes.
    pub files: Vec<RepoPath>,
    /// Undo and redo: nothing may need a backup of index or worktree. `mixed` refuses if the
    /// index has staged changes or conflicts, `hard` if there are local changes to tracked files
    /// ([`strict_conflicts`]): [`EngineError::UndoConflict`] and nothing changes.
    pub strict: bool,
}

impl ResetRequest {
    /// The branch moved (full name, `None` with detached HEAD), the commit expected now and the
    /// target, if the request has one of the two valid shapes (see [`ResetRequest::refs`]).
    pub fn shape(&self) -> Result<(Option<String>, Oid, Oid), EngineError> {
        use crate::HeadState;
        let r = &self.refs;
        match (&r.head_expected, &r.head_target, r.refs.as_slice()) {
            (HeadState::Branch(a), HeadState::Branch(b), [m]) if a == b && m.name == *a => {
                match (m.expected, m.target) {
                    (Some(e), Some(t)) => Ok((Some(a.clone()), e, t)),
                    _ => Err(self.invalid()),
                }
            }
            (HeadState::Detached(e), HeadState::Detached(t), []) => Ok((None, *e, *t)),
            _ => Err(self.invalid()),
        }
    }

    fn invalid(&self) -> EngineError {
        EngineError::NotAllowed(msg!(
            "engine-reset-bad-request",
            command = self.refs.command
        ))
    }
}

/// Outcome of [`GitBackend::reset`](crate::GitBackend::reset).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResetOutcome {
    /// `hard`: the files whose local changes were overwritten (and saved in `backup`).
    pub files: Vec<RepoPath>,
    /// `hard` with local changes: the ref `refs/project/backup/reset-...` with the worktree and
    /// index before the reset (on `raw` if there is one, otherwise on `stash`).
    pub backup: Option<String>,
    /// The commit of `git stash create` (`git stash apply <stash>` puts the changes back).
    pub stash: Option<Oid>,
    /// As [`DiscardOutcome::raw`](crate::DiscardOutcome::raw): the files whose bytes are not
    /// those git would recreate from `stash`, saved without filters in a child commit.
    pub raw: Option<Oid>,
    /// `mixed`: the ref `refs/project/backup/index-...` with the staged versions that existed
    /// only in the index.
    pub index_backup: Option<String>,
    /// The backup refs of the commits that would no longer be reachable (`reset-...`, or
    /// `undo-...`/`redo-...` for undo and redo).
    pub backups: Vec<String>,
    /// Problems found after the write (a ref moved by another process at that instant).
    pub warnings: Vec<Message>,
}

/// The files whose local changes a `hard` reset overwrites, in path order: every tracked file
/// with staged or unstaged changes (also the original of a rename) and every conflicted file;
/// submodules are skipped (the reset does not enter them). Shared by the backends.
///
/// Refused ([`EngineError::NotAllowed`], nothing changes) with `intent-to-add` files (`git add
/// -N`): the reset would delete them and `git stash create` cannot save them.
pub fn reset_files(status: &Status) -> Result<Vec<RepoPath>, EngineError> {
    let mut files = Vec::new();
    let mut intent = Vec::new();
    for entry in &status.entries {
        match &entry.kind {
            EntryKind::Ordinary(info)
            | EntryKind::Renamed { info, .. }
            | EntryKind::Copied { info, .. } => {
                if info.submodule.is_some() {
                    continue;
                }
                if entry.worktree == Change::Added && info.mode_index == 0 {
                    intent.push(entry.path.clone());
                    continue;
                }
                if !(entry.index.is_change() || entry.worktree.is_change()) {
                    continue;
                }
                files.push(entry.path.clone());
                if let EntryKind::Renamed { original, .. } = &entry.kind {
                    files.push(original.clone());
                }
            }
            EntryKind::Unmerged(info) => {
                if info.submodule.is_none() {
                    files.push(entry.path.clone());
                }
            }
            EntryKind::Untracked | EntryKind::Ignored => {}
        }
    }
    if !intent.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-reset-intent-to-add",
            files = list(&intent)
        )));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// [`EngineError::NotAllowed`] if the plan computed now (`now`) is not the confirmed one.
pub fn check_same_reset_files(now: &[RepoPath], expected: &[RepoPath]) -> Result<(), EngineError> {
    if now == expected {
        return Ok(());
    }
    Err(EngineError::NotAllowed(msg!(
        "engine-reset-files-changed",
        confirmed = list(expected),
        now = list(now)
    )))
}

/// For undo and redo ([`ResetRequest::strict`]): what would need a backup, one line per file
/// (empty: nothing). `mixed`: staged changes and conflicts (the index is rewritten); `hard`: the
/// local changes of [`reset_files`] (the worktree is rewritten); `soft`: nothing.
pub fn strict_conflicts(status: &Status, mode: ResetMode) -> Result<Vec<Message>, EngineError> {
    Ok(match mode {
        ResetMode::Soft => Vec::new(),
        ResetMode::Mixed => status
            .entries
            .iter()
            .filter(|e| match &e.kind {
                EntryKind::Unmerged(_) => true,
                EntryKind::Untracked | EntryKind::Ignored => false,
                _ => e.index.is_change(),
            })
            .map(|e| msg!("engine-reset-undo-staged", path = e.path))
            .collect(),
        ResetMode::Hard => reset_files(status)?
            .iter()
            .map(|p| msg!("engine-reset-undo-changed", path = p))
            .collect(),
    })
}

/// All the paths of the status (to give stage's index backup the whole repo).
pub fn status_paths(status: &Status) -> Vec<RepoPath> {
    status.entries.iter().map(|e| e.path.clone()).collect()
}

/// The error when HEAD or the branch no longer hold the expected values (`conflicts`, one line
/// each): [`EngineError::UndoConflict`] for undo and redo, [`EngineError::NotAllowed`] for
/// `branch.reset` (moved from outside between the read and the write, or since the confirmation).
pub fn reset_moved(request: &ResetRequest, conflicts: Vec<Message>) -> EngineError {
    if request.strict {
        EngineError::UndoConflict {
            command: request.refs.command,
            refs: conflicts,
        }
    } else {
        EngineError::NotAllowed(msg!(
            "engine-reset-moved",
            changes = MessageArg::Joined(conflicts, "; ".into())
        ))
    }
}

fn list(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, HeadState, MemoryBackend, RefMove};

    #[test]
    fn files_of_the_sample_and_json() {
        let repo = MemoryBackend::sample();
        let status = repo.status().unwrap();
        let files = reset_files(&status).unwrap();
        // Every tracked change and conflict of the sample, no untracked files.
        for f in &files {
            let e = status.entries.iter().find(|e| e.path == *f);
            assert!(
                e.is_none_or(|e| !matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored)),
                "{f}"
            );
        }
        assert!(check_same_reset_files(&files, &files).is_ok());
        assert!(check_same_reset_files(&files, &[]).is_err() || files.is_empty());
        assert_eq!(serde_json::to_value(ResetMode::Hard).unwrap(), "hard");
        assert!(
            strict_conflicts(&status, ResetMode::Soft)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn request_shapes() {
        let a = Oid::from_bytes([0xaa; Oid::LEN]);
        let b = Oid::from_bytes([0xbb; Oid::LEN]);
        let main = "refs/heads/main".to_owned();
        let mut request = ResetRequest {
            mode: ResetMode::Soft,
            refs: RefRestore {
                command: "branch.reset",
                head_expected: HeadState::Branch(main.clone()),
                head_target: HeadState::Branch(main.clone()),
                refs: vec![RefMove {
                    name: main.clone(),
                    expected: Some(a),
                    target: Some(b),
                }],
                reflog: String::new(),
            },
            files: Vec::new(),
            strict: false,
        };
        assert_eq!(request.shape().unwrap(), (Some(main), a, b));
        request.refs.refs[0].name = "refs/heads/other".to_owned();
        assert!(request.shape().is_err());
        request.refs.refs.clear();
        assert!(request.shape().is_err());
        request.refs.head_expected = HeadState::Detached(a);
        request.refs.head_target = HeadState::Detached(b);
        assert_eq!(request.shape().unwrap(), (None, a, b));
    }
}
