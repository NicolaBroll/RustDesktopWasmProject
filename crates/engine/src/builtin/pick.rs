//! Cherry-pick and revert of a commit (S31): `commit.cherry_pick`, `commit.revert`,
//! `cherry_pick.continue`, `cherry_pick.abort`, `revert.continue`, `revert.abort`. Types and
//! checks common to the backends in `git/pick.rs`; contract in `impl-docs/git/cherry-pick.md`.
//!
//! Like the merge of S29: a concluded operation (right away or with `*.continue`) enters the journal and
//! `edit.undo` undoes it with `reset --keep`; stopped on a conflict it **stays in progress** and does not enter
//! the journal; `*.abort` always asks for `"confirm": true` and first saves index and worktree.

use super::merge::{abort_confirm, check_message, label};
use super::{JournalState, Out, reread, to_json};
use crate::commands::parse_params;
use crate::{
    CommitRequest, EngineError, HeadState, JournalEntry, LogQuery, MergeAbortResult,
    MergeContinueResult, Message, PickKind, PickRequest, Session, branch_ref, check_pick_state,
    check_picking, clean_merge_message, conflicted_paths, pick_message, pick_parent,
};
use project_domain::{Head, Oid, RefInfo, RepoPath, Status};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result of `commit.cherry_pick` and `commit.revert`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickStartResult {
    /// The rev given in the parameters, as given.
    pub rev: String,
    /// The commit `rev` resolved to.
    pub commit: Oid,
    /// The first line of its message.
    pub summary: String,
    /// The parent (from 1) of a merge commit, as given; `null` if not given.
    pub mainline: Option<usize>,
    /// The active branch (short name).
    pub branch: String,
    /// The branch before the operation.
    pub before: Oid,
    /// The branch after the operation (the previous one, if `in_progress`).
    pub after: Oid,
    /// The operation stopped (conflicts, a hook) and stays in progress.
    pub in_progress: bool,
    /// The conflicted files of the stopped operation (empty otherwise).
    pub conflicts: Vec<RepoPath>,
    /// The git text of the stopped operation (empty otherwise).
    pub message: String,
    /// The backup ref of the branch before the operation
    /// (`refs/project/backup/<cherry-pick|revert>-...`).
    pub backup: String,
    /// Warnings after the operation (e.g. a result that is not a new commit on the branch).
    pub warnings: Vec<Message>,
    /// The status re-read after the operation; `null` if it could not be re-read (see
    /// `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The refs re-read after the operation; `null` if they could not be re-read (see
    /// `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Parameters of `commit.cherry_pick` and `commit.revert`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    /// The commit: an id (also abbreviated), a branch, a tag or another rev.
    rev: String,
    /// For a merge commit: the parent (from 1) relative to which to take the changes.
    #[serde(default)]
    mainline: Option<usize>,
}

/// Parameters of `*.continue`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueParams {
    #[serde(default)]
    message: Option<String>,
}

/// Parameters of `*.abort`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbortParams {
    #[serde(default)]
    confirm: bool,
}

pub(super) fn commit_cherry_pick(s: &mut Session, p: Value) -> Out {
    pick_start(s, p, PickKind::CherryPick)
}

pub(super) fn commit_revert(s: &mut Session, p: Value) -> Out {
    pick_start(s, p, PickKind::Revert)
}

pub(super) fn cherry_pick_continue(s: &mut Session, p: Value) -> Out {
    pick_continue(s, p, PickKind::CherryPick)
}

pub(super) fn revert_continue(s: &mut Session, p: Value) -> Out {
    pick_continue(s, p, PickKind::Revert)
}

pub(super) fn cherry_pick_abort(s: &mut Session, p: Value) -> Out {
    pick_abort(s, p, PickKind::CherryPick)
}

pub(super) fn revert_abort(s: &mut Session, p: Value) -> Out {
    pick_abort(s, p, PickKind::Revert)
}

/// Applies (`commit.cherry_pick`) or undoes (`commit.revert`) the commit `rev` on the active branch
/// with a new commit: for cherry-pick with the original author and message, for revert
/// with `Revert "<subject>"` and `This reverts commit <id>.`. A merge commit needs `mainline`
/// (from 1); `mainline` on another commit or outside the parents → [`EngineError::InvalidParams`].
///
/// Refused before writing ([`EngineError::NotAllowed`], nothing changes): detached HEAD or
/// branch without commits, operation in progress, conflicted files, any change to tracked
/// files ([`check_pick_state`]); a rev that does not exist → [`EngineError::UnknownRev`]; an
/// empty result (commit already applied or already undone) → [`EngineError::NotAllowed`], nothing
/// in progress. Before writing the backend saves the branch in
/// `refs/project/backup/<cherry-pick|revert>-...`.
///
/// **Conflicts**: the operation stays in progress (`in_progress`, `conflicts`), to be concluded with
/// `*.continue` or aborted with `*.abort`; nothing in the journal. Once concluded it enters the journal:
/// `edit.undo` brings the branch, with index and worktree, back to where it was.
///
/// Result: [`PickStartResult`].
fn pick_start(s: &mut Session, p: Value, kind: PickKind) -> Out {
    let id = kind.command();
    let StartParams { rev, mainline } = parse_params(id, p)?;
    if rev.trim().is_empty() || rev.contains('\0') {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-pick-rev-empty"),
        });
    }
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let (branch, before) = match &status.head {
        Head::Branch {
            name,
            target: Some(target),
        } => (name.clone(), *target),
        Head::Branch { name, target: None } => {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pick-unborn",
                name = name.escape_debug(),
                id = id
            )));
        }
        Head::Detached(_) => {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pick-detached",
                id = id
            )));
        }
    };
    check_pick_state(&status, kind)?;
    let commit = repo
        .log(&LogQuery::page(0, 1).from_rev(rev.clone()))?
        .into_iter()
        .next()
        .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?;
    let parent = pick_parent(id, &commit, mainline)?;
    let message = pick_message(kind, &commit, parent.as_ref(), commit.parents.len() > 1);
    let refs = repo.refs()?;
    let source = label(&commit.id, &refs);
    let outcome = repo.pick(&PickRequest {
        kind,
        branch: branch.clone(),
        expected: before,
        commit: commit.id,
        parent,
        mainline,
        message,
    })?;
    let mut warnings = outcome.warnings.clone();
    // In the journal only the expected result: a new commit on top of the previous branch.
    let as_expected = !outcome.in_progress
        && outcome.after != before
        && repo
            .log(&LogQuery::page(0, 1).from_rev(outcome.after.to_string()))
            .ok()
            .and_then(|c| c.first().map(|c| c.parents == [before]))
            .unwrap_or(false);
    if !outcome.in_progress && !as_expected {
        warnings.push(msg!(
            "engine-pick-unexpected-result",
            after = outcome.after,
            command = id,
            backup = outcome.backup
        ));
    }
    if as_expected {
        journal.record(JournalEntry::pick(
            id,
            kind,
            &branch_ref(&branch),
            before,
            outcome.after,
            HeadState::from_head(&status.head),
            &source,
            Some(outcome.backup.clone()),
        ));
    }
    let (status, status_error) = reread(repo.status());
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(PickStartResult {
        rev,
        commit: commit.id,
        summary: commit.summary,
        mainline,
        branch,
        before,
        after: outcome.after,
        in_progress: outcome.in_progress,
        conflicts: outcome.conflicts,
        message: outcome.message,
        backup: outcome.backup,
        warnings,
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

/// Concludes the cherry-pick or revert in progress with the commit (`git commit` like
/// `commit.create`: hooks and signing according to the configuration; for cherry-pick git keeps
/// the author of the original commit), with `message` or the one prepared by git (`MERGE_MSG` without
/// the `# Conflicts:` block). Refused ([`EngineError::NotAllowed`], nothing changes) without
/// the operation `kind` in progress, with a sequence of several commits started from a terminal or if
/// conflicted files remain. It enters the journal like the concluded operation; with detached HEAD
/// (started from a terminal) as a commit.
///
/// Result: [`MergeContinueResult`] (the same shape as `merge.continue`).
fn pick_continue(s: &mut Session, p: Value, kind: PickKind) -> Out {
    let id = kind.continue_command();
    let ContinueParams { message } = parse_params(id, p)?;
    check_message(id, &message)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let state = repo.operation_state()?;
    check_picking(id, kind, &status, &state)?;
    let conflicts = conflicted_paths(&status);
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-pick-continue-conflicts",
            files = conflicts
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", "),
            id = id
        )));
    }
    let refs = repo.refs()?;
    let incoming = state.incoming.first().copied();
    let original = match incoming {
        Some(oid) => repo
            .log(&LogQuery::page(0, 1).from_rev(oid.to_string()))
            .ok()
            .and_then(|c| c.into_iter().next()),
        None => None,
    };
    let message = match message {
        Some(message) => message,
        None => state
            .message
            .as_deref()
            .map(clean_merge_message)
            .filter(|m| !m.trim().is_empty())
            .or_else(|| {
                original
                    .as_ref()
                    .map(|c| pick_message(kind, c, c.parents.first(), false))
            })
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-pick-no-message", id = id)))?,
    };
    let outcome = repo
        .commit(&CommitRequest::new(message.clone()).concluding(kind.operation()))
        .map_err(|e| match e {
            // Resolved as HEAD was: nothing to commit, and the operation stays in progress.
            EngineError::NothingToCommit => EngineError::NotAllowed(msg!(
                "engine-pick-continue-empty",
                abort = kind.abort_command()
            )),
            e => e,
        })?;
    let source = incoming.map_or_else(|| kind.head_file().to_owned(), |o| label(&o, &refs));
    match (&outcome.branch, outcome.previous) {
        (Some(branch), Some(previous)) => journal.record(JournalEntry::pick(
            id,
            kind,
            branch,
            previous,
            outcome.commit.id,
            HeadState::from_head(&status.head),
            &source,
            None,
        )),
        _ => match JournalEntry::commit(&outcome, false) {
            Some(entry) => journal.record(entry),
            None => 0,
        },
    };
    let (status, status_error) = reread(repo.status());
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(MergeContinueResult {
        commit: outcome.commit,
        previous: outcome.previous,
        branch: outcome.branch,
        backup: outcome.backup,
        also_on: outcome.also_on,
        message,
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

/// Aborts the cherry-pick or revert in progress (`git cherry-pick --abort`, `git revert
/// --abort`): index and worktree go back to HEAD for the operation's files. It **always** asks for
/// `"confirm": true` (without: [`EngineError::ConfirmRequired`] with the files, nothing changes) and the
/// backend first saves index and worktree in `refs/project/backup/<cherry-pick|revert>-abort-...`
/// (like a stash). Without the operation `kind` in progress, or with a sequence of several commits started
/// from a terminal → [`EngineError::NotAllowed`]. It does not enter the journal.
///
/// Result: [`MergeAbortResult`] (the same shape as `merge.abort`).
fn pick_abort(s: &mut Session, p: Value, kind: PickKind) -> Out {
    let id = kind.abort_command();
    let AbortParams { confirm } = parse_params(id, p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let state = repo.operation_state()?;
    check_picking(id, kind, &status, &state)?;
    if !confirm {
        return Err(abort_confirm(id, &status, kind.git(), kind.git()));
    }
    let aborted = repo.pick_abort(kind)?;
    let (status, status_error) = reread(repo.status());
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(MergeAbortResult {
        backup: aborted.backup,
        files: aborted.files,
        warnings: aborted.warnings,
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
