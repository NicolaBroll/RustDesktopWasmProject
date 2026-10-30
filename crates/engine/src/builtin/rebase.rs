//! Interactive rebase (S32): `rebase.plan`, `rebase.start`, `rebase.continue`, `rebase.abort`.
//! Types and checks common to the backends in `git/rebase.rs`; contract in
//! `impl-docs/git/rebase.md`.
//!
//! Like the cherry-pick of S31: a concluded rebase (right away or with `rebase.continue`) enters the
//! journal and `edit.undo` undoes it with `reset --keep`; stopped (conflict, hook, signing) it
//! **stays in progress** and does not enter the journal; `rebase.abort` always asks for
//! `"confirm": true` and first saves index and worktree. `rebase.continue` also concludes the
//! rebase of a stopped `remote.pull`, with the pull's journal entry.

use super::merge::{MergeAbortParams, abort_confirm};
use super::{JournalState, Out, reread, to_json};
use crate::git::check_in_progress;
use crate::{
    EngineError, GitBackend, JournalEntry, LogQuery, MergeAbortResult, Message, RebaseRecord,
    RebaseRequest, Session, branch_ref, check_rebase_state, conflicted_paths, plan_error,
    quoted_list, rebase_range, rewritten_commits,
};
use project_domain::{
    Head, Oid, Operation, RebaseAction, RebasePlan, RebaseStep, RefInfo, RefKind, RepoPath, Status,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// One step of the default plan of `rebase.plan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebasePlanStep {
    /// Always [`RebaseAction::Pick`] in the default plan.
    pub action: RebaseAction,
    /// The commit of the range.
    pub commit: Oid,
    /// The first line of its message.
    pub summary: String,
}

/// Result of `rebase.plan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebasePlanResult {
    /// The rev `base` resolved to a commit.
    pub base: Oid,
    /// The active branch (short name).
    pub branch: String,
    /// The commits of `base..HEAD`, oldest first, every one `pick`.
    pub steps: Vec<RebasePlanStep>,
    /// The commits of the range reachable from a remote-tracking branch.
    pub published: Vec<Oid>,
}

/// Result of `rebase.start`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RebaseStartResult {
    /// The rev `base` resolved to a commit.
    pub base: Oid,
    /// The rewritten branch (short name).
    pub branch: String,
    /// The branch before the rebase.
    pub before: Oid,
    /// The branch after the rebase (where it stopped, if `in_progress`).
    pub after: Oid,
    /// The rebase stopped (conflicts, a hook, signing) and stays in progress.
    pub in_progress: bool,
    /// The conflicted files of the stopped rebase (empty otherwise).
    pub conflicts: Vec<RepoPath>,
    /// The git text of the stopped rebase (empty otherwise).
    pub message: String,
    /// The backup ref of the branch before the rebase (`refs/project/backup/rebase-...`).
    pub backup: String,
    /// Warnings after the rebase (e.g. a commit count different from the plan's).
    pub warnings: Vec<Message>,
    /// The status re-read after the rebase; `null` if it could not be re-read (see
    /// `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The refs re-read after the rebase; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `rebase.continue`. `command`, `before` and `backup` are `null` for a rebase started
/// from the terminal (no record of the client).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RebaseContinueResult {
    /// The command that started the rebase: `rebase.start` or `remote.pull` (kept as text: it is
    /// read back from the record saved in the git folder).
    pub command: Option<String>,
    /// The commit the branch is rebuilt on; `null` if unknown.
    pub base: Option<Oid>,
    /// The rewritten branch (short name); `null` if unknown.
    pub branch: Option<String>,
    /// The branch before the rebase.
    pub before: Option<Oid>,
    /// The branch after this step (where it stopped again, if `in_progress`).
    pub after: Oid,
    /// The rebase stopped again and stays in progress.
    pub in_progress: bool,
    /// The conflicted files of the stopped rebase (empty otherwise).
    pub conflicts: Vec<RepoPath>,
    /// The git text of the stopped rebase (empty otherwise).
    pub message: String,
    /// The backup ref of the branch before the rebase (`refs/project/backup/rebase-...` or
    /// `pull-...`).
    pub backup: Option<String>,
    /// Warnings after the step (e.g. a step resolved to nothing and dropped).
    pub warnings: Vec<Message>,
    /// The status re-read after the step; `null` if it could not be re-read (see
    /// `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The refs re-read after the step; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Parameters of `rebase.plan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebasePlanParams {
    /// The commit the branch is rebuilt on (excluded): an id, a branch, a tag or another rev.
    pub base: String,
}

/// Parameters of `rebase.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebaseStartParams {
    /// The commit the branch is rebuilt on (excluded), as in `rebase.plan`.
    pub base: String,
    /// The plan: oldest first, every commit of `base..HEAD` once.
    pub steps: Vec<RebaseStep>,
    /// Rewrite also published commits.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoParams {}

/// The branch of HEAD (short name) and its commit; detached HEAD or a branch without commits →
/// [`EngineError::NotAllowed`].
fn head_branch(id: &str, head: &Head) -> Result<(String, Oid), EngineError> {
    match head {
        Head::Branch {
            name,
            target: Some(target),
        } => Ok((name.clone(), *target)),
        Head::Branch { name, target: None } => Err(EngineError::NotAllowed(msg!(
            "engine-rebase-unborn",
            name = name.escape_debug(),
            id = id
        ))),
        Head::Detached(_) => Err(EngineError::NotAllowed(msg!(
            "engine-rebase-detached",
            id = id
        ))),
    }
}

/// `base` resolved to a commit ([`EngineError::UnknownRev`] if it does not exist).
fn resolve_base(repo: &dyn GitBackend, id: &'static str, base: &str) -> Result<Oid, EngineError> {
    if base.trim().is_empty() || base.contains('\0') {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-rebase-base-empty"),
        });
    }
    repo.log(&LogQuery::page(0, 1).from_rev(base.to_owned()))?
        .into_iter()
        .next()
        .map(|c| c.id)
        .ok_or_else(|| EngineError::UnknownRev(base.to_owned()))
}

/// The commits of `ids` (the range `base..head`) reachable from a remote-tracking branch.
fn published(
    repo: &dyn GitBackend,
    head: &Oid,
    base: &Oid,
    ids: &[Oid],
) -> Result<Vec<Oid>, EngineError> {
    let mut exclude: Vec<Oid> = repo
        .refs()?
        .into_iter()
        .filter(|r| r.kind == RefKind::RemoteBranch)
        .map(|r| r.target)
        .collect();
    if exclude.is_empty() {
        return Ok(Vec::new());
    }
    exclude.push(*base);
    let local: HashSet<Oid> = repo
        .missing_commits(head, &exclude, ids.len() + 1)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    Ok(ids.iter().filter(|c| !local.contains(c)).copied().collect())
}

/// The default plan of the range `base..HEAD` of the active branch: every commit `pick`, oldest first,
/// with `published` the commits reachable from a remote-tracking branch. Refused
/// ([`EngineError::NotAllowed`]): detached HEAD, `base` not an ancestor of HEAD or equal to it,
/// merge commits in the range; `base` that does not exist → [`EngineError::UnknownRev`].
///
/// Result: [`RebasePlanResult`].
pub(super) fn rebase_plan(s: &mut Session, RebasePlanParams { base }: RebasePlanParams) -> Out {
    const ID: &str = "rebase.plan";
    let repo = s.repo()?;
    let (branch, head) = head_branch(ID, &repo.head()?)?;
    let base = resolve_base(repo, ID, &base)?;
    let commits = rebase_range(repo, ID, &head, &base)?;
    let ids: Vec<Oid> = commits.iter().map(|c| c.id).collect();
    let published = published(repo, &head, &base, &ids)?;
    let steps = commits
        .into_iter()
        .map(|c| RebasePlanStep {
            action: RebaseAction::Pick,
            commit: c.id,
            summary: c.summary,
        })
        .collect();
    Ok(to_json(RebasePlanResult {
        base,
        branch,
        steps,
        published,
    }))
}

/// The checks after a concluded rebase that go in `warnings`: the commits the plan produces
/// (`record.produces`) must be the ones between `onto` and the branch (git drops a step resolved to
/// nothing); for the pull, the branch must contain the upstream.
fn check_result(repo: &dyn GitBackend, record: &RebaseRecord, after: &Oid) -> Vec<Message> {
    let mut warnings = Vec::new();
    let full = branch_ref(&record.branch);
    if let Some(expected) = record.produces {
        match repo.unmerged_count(after, &[record.onto]) {
            Ok(found) if found == expected => {}
            Ok(found) => warnings.push(msg!(
                "engine-rebase-count-mismatch",
                found = found,
                expected = expected,
                backup = record.backup
            )),
            Err(e) => warnings.push(msg!("engine-rebase-not-checked", error = e)),
        }
    }
    if let Some(upstream) = &record.upstream
        && repo.unmerged_count(&record.onto, &[*after]).ok() != Some(0)
    {
        warnings.push(msg!(
            "engine-pull-upstream-missing",
            full = full,
            after = after,
            name = upstream
        ));
    }
    warnings
}

/// Rewrites the commits `base..HEAD` of the active branch according to `steps` (oldest first, every
/// commit of the range exactly once: `pick`, `reword` with `message`, `squash`/`fixup` into the
/// commit before, optionally with the `message` of the combined commit on the last of the group,
/// `drop`). Invalid plan → [`EngineError::InvalidParams`]; a plan that changes nothing, detached
/// HEAD, operation in progress, conflicted files, any change to tracked files, merges in the
/// range, `base` not an ancestor → [`EngineError::NotAllowed`]. Rewriting commits already
/// published (from the first changed step onward) needs `"confirm": true`
/// ([`EngineError::ConfirmRequired`] naming them). Before writing the backend saves the branch
/// in `refs/project/backup/rebase-...`.
///
/// **Stops** (conflict, failed hook or signing): the rebase stays in progress (`in_progress`,
/// `conflicts`, `message`), to be concluded with `rebase.continue` or aborted with `rebase.abort`;
/// nothing in the journal. Concluded it enters the journal: `edit.undo` brings the branch, with index
/// and worktree, back to where it was.
///
/// Result: [`RebaseStartResult`].
pub(super) fn rebase_start(
    s: &mut Session,
    RebaseStartParams {
        base,
        steps,
        confirm,
    }: RebaseStartParams,
) -> Out {
    const ID: &str = "rebase.start";
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let (branch, before) = head_branch(ID, &status.head)?;
    check_rebase_state(&status)?;
    let base = resolve_base(repo, ID, &base)?;
    let commits: Vec<Oid> = rebase_range(repo, ID, &before, &base)?
        .iter()
        .map(|c| c.id)
        .collect();
    let plan = RebasePlan { base, steps };
    plan.validate(&commits).map_err(|e| plan_error(ID, &e))?;
    if plan.is_noop(&commits) {
        return Err(EngineError::NotAllowed(msg!("engine-rebase-noop")));
    }
    let rewritten = rewritten_commits(&plan, &commits);
    if !confirm {
        let at_risk: Vec<String> = published(repo, &before, &base, &commits)?
            .iter()
            .filter(|c| rewritten.contains(c))
            .map(Oid::short)
            .collect();
        if !at_risk.is_empty() {
            return Err(EngineError::ConfirmRequired {
                command: ID,
                reason: msg!("engine-rebase-published", commits = quoted_list(&at_risk)),
                expected: None,
            });
        }
    }
    let request = RebaseRequest {
        branch: branch.clone(),
        expected: before,
        plan,
        commits,
    };
    let outcome = repo.rebase(&request)?;
    let mut warnings = outcome.warnings.clone();
    if !outcome.in_progress {
        let record = request.record(&outcome.backup);
        let on_branch = matches!(repo.head(), Ok(Head::Branch { name, .. }) if name == branch);
        if on_branch && outcome.after != before {
            warnings.extend(check_result(repo, &record, &outcome.after));
            journal.record(JournalEntry::rebase(
                ID,
                &branch_ref(&branch),
                before,
                outcome.after,
                record.commits,
                Some(outcome.backup.clone()),
            ));
        } else {
            warnings.push(msg!(
                "engine-rebase-unexpected-result",
                after = outcome.after,
                backup = outcome.backup
            ));
        }
    }
    let (status, status_error) = reread(repo.status());
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(RebaseStartResult {
        base,
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

/// Goes on with the rebase in progress after the conflicts are resolved and staged (`git rebase
/// --continue`: the commit stopped on keeps its message), then the rest of the plan with its
/// messages. A step resolved to nothing (as HEAD) is dropped, as git does (`warnings`). Refused
/// ([`EngineError::NotAllowed`]) without a rebase in progress or with conflicted files. Stopped
/// again → `in_progress`. Concluded → journal entry: the rebase's, or the pull's for a stopped
/// `remote.pull`; a rebase started from the terminal concludes without a journal entry (warning).
///
/// Result: [`RebaseContinueResult`] (`command`/`before`/`backup` `null` for a rebase started from
/// the terminal).
pub(super) fn rebase_continue(s: &mut Session, NoParams {}: NoParams) -> Out {
    const ID: &str = "rebase.continue";
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_in_progress(ID, Operation::Rebase, &status)?;
    let conflicts = conflicted_paths(&status);
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-rebase-continue-conflicts",
            files = quoted_list(&conflicts)
        )));
    }
    let state = repo.operation_state()?;
    let record = state.rebase.clone();
    let outcome = repo.rebase_continue()?;
    let mut warnings = outcome.warnings.clone();
    if !outcome.in_progress {
        match &record {
            Some(record) => {
                let full = branch_ref(&record.branch);
                let on_branch =
                    matches!(repo.head(), Ok(Head::Branch { name, .. }) if name == record.branch);
                if on_branch && outcome.after != record.before {
                    warnings.extend(check_result(repo, record, &outcome.after));
                    let entry = match &record.upstream {
                        Some(upstream) => JournalEntry::pull(
                            &full,
                            record.before,
                            outcome.after,
                            crate::HeadState::Branch(full.clone()),
                            crate::Integration::Rebase,
                            upstream,
                            record.backup.clone(),
                        ),
                        None => JournalEntry::rebase(
                            ID,
                            &full,
                            record.before,
                            outcome.after,
                            record.commits,
                            Some(record.backup.clone()),
                        ),
                    };
                    journal.record(entry);
                } else {
                    warnings.push(msg!(
                        "engine-rebase-unexpected-result",
                        after = outcome.after,
                        backup = record.backup
                    ));
                }
            }
            None => warnings.push(msg!("engine-rebase-terminal")),
        }
    }
    let branch = match &record {
        Some(r) => Some(r.branch.clone()),
        None => state
            .rebase_branch
            .as_deref()
            .map(|b| crate::branch_short(b).to_owned()),
    };
    let (status, status_error) = reread(repo.status());
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(RebaseContinueResult {
        command: record.as_ref().map(|r| r.command.clone()),
        base: record.as_ref().map(|r| r.onto).or(state.onto),
        branch,
        before: record.as_ref().map(|r| r.before),
        after: outcome.after,
        in_progress: outcome.in_progress,
        conflicts: outcome.conflicts,
        message: outcome.message,
        backup: record.map(|r| r.backup),
        warnings,
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

/// Aborts the rebase in progress (`git rebase --abort`): the branch, index and worktree go back to
/// before the rebase (also one started by `remote.pull` or from the terminal). It **always** asks
/// for `"confirm": true` (without: [`EngineError::ConfirmRequired`] with the files, nothing
/// changes) and the backend first saves index and worktree in `refs/project/backup/rebase-abort-...`
/// (like a stash, on the commit the rebase had reached). Without a rebase in progress →
/// [`EngineError::NotAllowed`]. It does not enter the journal.
///
/// Result: [`MergeAbortResult`] (the same shape as `merge.abort`).
pub(super) fn rebase_abort(s: &mut Session, MergeAbortParams { confirm }: MergeAbortParams) -> Out {
    const ID: &str = "rebase.abort";
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_in_progress(ID, Operation::Rebase, &status)?;
    if !confirm {
        return Err(abort_confirm(ID, &status, "rebase", "rebase"));
    }
    let aborted = repo.rebase_abort()?;
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
#[path = "rebase_tests.rs"]
mod tests;
