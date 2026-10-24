//! Merge and the "in progress" state (S29): `merge.start`, `merge.abort`, `merge.continue` and the reading
//! `repo.operation` for the UI banner. Types and checks common to the backends in
//! `git/merge.rs`; contract in `impl-docs/git/merge.md`.
//!
//! A concluded merge (fast-forward or merge commit, right away or with `merge.continue`) enters the
//! journal and `edit.undo` undoes it like a pull of the active branch. A merge stopped on a conflict
//! **stays in progress** and does not enter the journal (the branch has not moved); `merge.abort` always asks
//! for `"confirm": true` and first saves index and worktree in a backup.

use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    CommitRequest, EngineError, GitBackend, HeadState, JournalEntry, LogQuery, MergeMode,
    MergeRequest, MessageArg, Session, branch_ref, check_merge_state, check_merging,
    clean_merge_message, conflicted_paths, merge_source,
};
use project_domain::{Head, Oid, Operation, RefInfo, RefKind, RepoPath};
use serde::Deserialize;
use serde_json::{Value, json};

/// Parameters of `merge.start`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartParams {
    /// What to merge: a local or remote branch, a tag, an id or another rev.
    rev: String,
    #[serde(default)]
    no_ff: bool,
    #[serde(default)]
    ff_only: bool,
    #[serde(default)]
    message: Option<String>,
}

/// Parameters of `merge.abort`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbortParams {
    #[serde(default)]
    confirm: bool,
}

/// Parameters of `merge.continue`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContinueParams {
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// The optional message of `merge.start`/`merge.continue`: non-empty and without NUL (like
/// `commit.create`), otherwise [`EngineError::InvalidParams`] with the command id.
pub(super) fn check_message(id: &'static str, message: &Option<String>) -> Result<(), EngineError> {
    let Some(message) = message else {
        return Ok(());
    };
    CommitRequest::new(message.clone())
        .check_message()
        .map_err(|e| match e {
            EngineError::InvalidParams { reason, .. } => EngineError::InvalidParams { id, reason },
            other => other,
        })
}

/// Adds to `out` the re-read state and refs (`null` with `*_error` if they cannot be re-read:
/// the operation is done and must not be repeated) and the journal state.
pub(super) fn finish(out: &mut Value, repo: &dyn GitBackend, journal: &crate::Journal) {
    let state = journal_state(journal);
    for key in ["can_undo", "can_redo", "next_undo", "next_redo"] {
        out[key] = state[key].clone();
    }
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
    match repo.refs() {
        Ok(refs) => out["refs"] = to_json(refs),
        Err(e) => {
            out["refs"] = Value::Null;
            out["refs_error"] = Value::String(e.to_string());
        }
    }
}

/// The short name of the first local branch, remote branch or tag that points to `oid` (in
/// this order), otherwise the abbreviated id: to describe an incoming commit.
pub(super) fn label(oid: &Oid, refs: &[RefInfo]) -> String {
    for (kind, prefix) in [
        (RefKind::LocalBranch, "refs/heads/"),
        (RefKind::RemoteBranch, "refs/remotes/"),
        (RefKind::Tag, "refs/tags/"),
    ] {
        if let Some(r) = refs.iter().find(|r| r.kind == kind && r.target == *oid) {
            return r.name.strip_prefix(prefix).unwrap_or(&r.name).to_owned();
        }
    }
    crate::git::short(oid)
}

/// Merges `rev` into the active branch (`git merge`): fast-forward if the branch has no commits that
/// `rev` lacks (and there is no `no_ff`), otherwise a merge commit with `message` or git's default
/// message (`Merge branch 'x'`, ...). `ff_only`: fast-forward only (diverged →
/// [`EngineError::NotAllowed`]); `no_ff` and `ff_only` together → [`EngineError::InvalidParams`].
///
/// Refused before writing ([`EngineError::NotAllowed`], nothing changes): detached HEAD or
/// branch without commits, operation in progress, conflicted files, for a merge commit any
/// change to tracked files ([`check_merge_state`]); a rev that does not exist →
/// [`EngineError::UnknownRev`]. Nothing to do if the branch already contains `rev` (`up_to_date`).
/// Before writing the backend saves the branch in `refs/project/backup/merge-...`.
///
/// **Conflicts**: the merge stays in progress (`in_progress`, `conflicts`), to be concluded with
/// `merge.continue` after resolving the files (`index.stage`) or aborted with `merge.abort`;
/// nothing in the journal. A concluded merge enters the journal: `edit.undo` brings the branch, with
/// index and worktree, back to where it was.
///
/// Result: `{"rev", "source": {"kind", "name", "full"}, "target", "branch", "strategy":
/// "fast_forward" | "merge" | "up_to_date", "before", "after", "ahead", "behind",
/// "in_progress", "conflicts": [path], "message", "backup": string | null, "warnings",
/// "status", "refs", "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn merge_start(s: &mut Session, p: Value) -> Out {
    const ID: &str = "merge.start";
    let StartParams {
        rev,
        no_ff,
        ff_only,
        message,
    } = parse_params(ID, p)?;
    if no_ff && ff_only {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-merge-no-ff-and-ff-only"),
        });
    }
    if rev.trim().is_empty() || rev.contains('\0') {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-merge-rev-empty"),
        });
    }
    check_message(ID, &message)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let (branch, before) = match &status.head {
        Head::Branch {
            name,
            target: Some(target),
        } => (name.clone(), *target),
        Head::Branch { name, target: None } => {
            return Err(EngineError::NotAllowed(msg!(
                "engine-merge-unborn",
                name = name.escape_debug()
            )));
        }
        Head::Detached(_) => {
            return Err(EngineError::NotAllowed(msg!("engine-merge-detached")));
        }
    };
    // Before everything else: an operation in progress or a conflict stops it anyway.
    check_merge_state(&status, &MergeMode::FastForward)?;
    let target = repo
        .log(&LogQuery::page(0, 1).from_rev(rev.clone()))?
        .first()
        .map(|c| c.id)
        .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?;
    let refs = repo.refs()?;
    let source = merge_source(&rev, &refs, &target);
    let behind = repo.unmerged_count(&target, &[before])?;
    let ahead = repo.unmerged_count(&before, &[target])?;
    let mut out = json!({
        "rev": rev,
        "source": to_json(&source),
        "target": target.to_string(),
        "branch": branch,
        "before": before.to_string(),
        "ahead": ahead,
        "behind": behind,
        "in_progress": false,
        "conflicts": [],
        "message": "",
        "warnings": [],
    });
    if behind == 0 {
        out["strategy"] = json!("up_to_date");
        out["after"] = json!(before.to_string());
        out["backup"] = Value::Null;
        finish(&mut out, repo, journal);
        return Ok(out);
    }
    let mode = if ahead == 0 && !no_ff {
        MergeMode::FastForward
    } else if ff_only {
        return Err(EngineError::NotAllowed(msg!(
            "engine-merge-diverged-ff-only",
            source = source.name.escape_debug(),
            branch = branch.escape_debug(),
            ahead = ahead,
            behind = behind
        )));
    } else {
        MergeMode::Merge { no_ff }
    };
    check_merge_state(&status, &mode)?;
    let message = message.unwrap_or_else(|| source.message(&branch));
    let outcome = repo.merge(&MergeRequest {
        branch: branch.clone(),
        expected: before,
        target,
        target_name: source.name.clone(),
        mode,
        message,
    })?;
    let mut warnings = outcome.warnings.clone();
    // In the journal only if the result is the merge's: the fast-forward commit, or a
    // merge commit on top of the previous branch. Otherwise another process (or a hook) has
    // moved the branch in the meantime, and an undo would also bring back its work.
    let as_expected = !outcome.in_progress
        && outcome.after != before
        && match mode {
            MergeMode::FastForward => outcome.after == target,
            MergeMode::Merge { .. } => repo
                .log(&LogQuery::page(0, 1).from_rev(outcome.after.to_string()))
                .ok()
                .and_then(|c| c.first().map(|c| c.parents.first() == Some(&before)))
                .unwrap_or(false),
        };
    if !outcome.in_progress && outcome.after != before && !as_expected {
        warnings.push(msg!(
            "engine-merge-unexpected-result",
            after = outcome.after,
            backup = outcome.backup
        ));
    }
    if as_expected {
        // The branch moved: the entry goes into the journal even if something cannot be re-read afterwards.
        journal.record(JournalEntry::merge(
            ID,
            &branch_ref(&branch),
            before,
            outcome.after,
            HeadState::from_head(&status.head),
            mode.name(),
            &source.name,
            Some(outcome.backup.clone()),
        ));
    }
    out["strategy"] = json!(mode.name());
    out["after"] = json!(outcome.after.to_string());
    out["backup"] = json!(outcome.backup);
    out["in_progress"] = json!(outcome.in_progress);
    out["conflicts"] = to_json(&outcome.conflicts);
    out["message"] = json!(outcome.message);
    out["warnings"] = to_json(&warnings);
    finish(&mut out, repo, journal);
    Ok(out)
}

/// Aborts the merge in progress (`git merge --abort`): index and worktree go back to HEAD for the
/// merge's files, and the resolutions made in the conflicted files, the staged changes and those
/// made after the merge started would be lost. For this it **always** asks for `"confirm":
/// true` (without: [`EngineError::ConfirmRequired`] with the files, nothing changes) and the backend first saves
/// index and worktree in `refs/project/backup/merge-abort-...` (like a stash: `git stash
/// apply <ref>` puts them back). Without a merge in progress → [`EngineError::NotAllowed`]. It does not enter
/// the journal (recovery is the backup).
///
/// Result: `{"backup", "files": [path], "warnings", "status", "refs", "can_undo",
/// "can_redo", "next_undo", "next_redo"}`.
pub(super) fn merge_abort(s: &mut Session, p: Value) -> Out {
    const ID: &str = "merge.abort";
    let AbortParams { confirm } = parse_params(ID, p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_merging(ID, &status)?;
    if !confirm {
        return Err(abort_confirm(ID, &status, "merge", "merge"));
    }
    let aborted = repo.merge_abort()?;
    let mut out = json!({
        "backup": aborted.backup,
        "files": to_json(&aborted.files),
        "warnings": to_json(&aborted.warnings),
    });
    finish(&mut out, repo, journal);
    Ok(out)
}

/// The [`EngineError::ConfirmRequired`] error of `merge.abort`, `cherry_pick.abort` and
/// `revert.abort` without `"confirm": true`: what would be lost (the conflicted files and those
/// changed) and the backup that would be saved first (`refs/project/backup/<backup>-abort-...`).
/// `what` is the name of the operation (`merge`, `cherry-pick`, `revert`).
pub(super) fn abort_confirm(
    id: &'static str,
    status: &project_domain::Status,
    what: &str,
    backup: &str,
) -> EngineError {
    let conflicts = conflicted_paths(status);
    let changed: Vec<String> = status
        .entries
        .iter()
        .filter(|e| e.is_staged() || e.is_unstaged())
        .map(|e| format!("`{}`", e.path))
        .collect();
    let mut lost = Vec::new();
    if !conflicts.is_empty() {
        lost.push(msg!(
            "engine-merge-abort-lose-conflicts",
            files = crate::quoted_list(&conflicts)
        ));
    }
    if !changed.is_empty() {
        lost.push(msg!(
            "engine-merge-abort-lose-changes",
            operation = what,
            files = changed.join(", ")
        ));
    }
    if lost.is_empty() {
        lost.push(msg!("engine-merge-abort-lose-operation", operation = what));
    }
    EngineError::ConfirmRequired {
        command: id,
        reason: msg!(
            "engine-merge-abort-confirm",
            lost = MessageArg::Joined(lost, "; ".into()),
            backup = backup
        ),
        expected: None,
    }
}

/// Concludes the merge in progress with the merge commit (parents HEAD and `MERGE_HEAD`, `git commit`
/// like `commit.create`: hooks and signing according to the configuration), with `message` or the one
/// prepared by git (`MERGE_MSG`, without the `# Conflicts:` block). Refused
/// ([`EngineError::NotAllowed`], nothing changes) without a merge in progress or if conflicted files
/// remain (listed: they must be resolved and staged with `index.stage`). It enters the journal as
/// a concluded merge; with detached HEAD (a merge started from a terminal) as a commit.
///
/// Result: `{"commit": Commit, "previous", "branch", "message", "status", "refs",
/// "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn merge_continue(s: &mut Session, p: Value) -> Out {
    const ID: &str = "merge.continue";
    let ContinueParams { message } = parse_params(ID, p)?;
    check_message(ID, &message)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_merging(ID, &status)?;
    let conflicts = conflicted_paths(&status);
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-merge-continue-conflicts",
            files = conflicts
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    let state = repo.operation_state()?;
    let refs = repo.refs()?;
    let sources: Vec<String> = state.incoming.iter().map(|o| label(o, &refs)).collect();
    let message = match message {
        Some(message) => message,
        None => state
            .message
            .as_deref()
            .map(clean_merge_message)
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "Merge {}",
                    state
                        .incoming
                        .iter()
                        .map(|o| format!("commit '{o}'"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }),
    };
    let outcome = repo.commit(&CommitRequest::new(message.clone()))?;
    match (&outcome.branch, outcome.previous) {
        (Some(branch), Some(previous)) => journal.record(JournalEntry::merge(
            ID,
            branch,
            previous,
            outcome.commit.id,
            HeadState::from_head(&status.head),
            "merge",
            &if sources.is_empty() {
                "MERGE_HEAD".to_owned()
            } else {
                sources.join(", ")
            },
            None,
        )),
        _ => match JournalEntry::commit(&outcome, false) {
            Some(entry) => journal.record(entry),
            None => 0,
        },
    };
    let mut out = to_json(&outcome);
    out["message"] = json!(message);
    finish(&mut out, repo, journal);
    Ok(out)
}

/// The operation in progress, for the UI banner: `{"operation": "Merge" | "Rebase" |
/// "CherryPick" | "Revert" | "Bisect" | "Am" | null, "head": Head, "conflicts": [path],
/// "incoming": [{"id", "label", "summary": string | null, "refs": [full name]}], "message":
/// string | null, "onto": oid | null, "rebase_branch": string | null, "ready": bool,
/// "continue": id | null, "abort": id | null}`.
///
/// `incoming`: the incoming commits (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`) with the refs
/// that point to them; `message`: the message `merge.continue` would use (`MERGE_MSG` without the
/// conflicts block); `ready`: no more conflicted files; `continue`/`abort`: the commands
/// that conclude or abort the operation from here (merge, cherry-pick and revert; `null` for the
/// others and for a sequence of several commits started from a terminal).
pub(super) fn repo_operation(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("repo.operation", p)?;
    let repo = s.repo()?;
    let status = repo.status()?;
    let state = repo.operation_state()?;
    let refs = repo.refs()?;
    let conflicts: Vec<RepoPath> = conflicted_paths(&status);
    let incoming: Vec<Value> = state
        .incoming
        .iter()
        .map(|oid| {
            let summary = repo
                .log(&LogQuery::page(0, 1).from_rev(oid.to_string()))
                .ok()
                .and_then(|c| c.first().map(|c| c.summary.clone()));
            let names: Vec<&str> = refs
                .iter()
                .filter(|r| r.target == *oid && r.kind != RefKind::Stash)
                .map(|r| r.name.as_str())
                .collect();
            json!({
                "id": oid.to_string(),
                "label": label(oid, &refs),
                "summary": summary,
                "refs": names,
            })
        })
        .collect();
    let operation = status.operation;
    // The commands that conclude or abort the operation from here: merge, and cherry-pick and
    // revert of a single commit (a sequence started from a terminal stays with the terminal).
    let (continue_id, abort_id) = match operation {
        Some(Operation::Merge) => (Some("merge.continue"), Some("merge.abort")),
        Some(op) => match crate::PickKind::from_operation(op) {
            Some(kind) if !state.sequencer => {
                (Some(kind.continue_command()), Some(kind.abort_command()))
            }
            _ => (None, None),
        },
        None => (None, None),
    };
    Ok(json!({
        "operation": to_json(operation),
        "head": to_json(&status.head),
        "conflicts": to_json(&conflicts),
        "incoming": incoming,
        "message": state.message.as_deref().map(clean_merge_message),
        "onto": state.onto.map(|o| o.to_string()),
        "rebase_branch": state.rebase_branch,
        "ready": operation.is_some() && conflicts.is_empty(),
        "continue": continue_id,
        "abort": abort_id,
    }))
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
