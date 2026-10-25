//! Interactive rebase (`rebase.plan`, `rebase.start`, `rebase.continue`, `rebase.abort`, S32):
//! types and checks shared by the backends (`impl-docs/git/rebase.md`).
//!
//! The plan is pure data ([`RebasePlan`], in the domain). The command reads the range
//! `base..HEAD` ([`rebase_range`]), validates the plan ([`plan_error`]) and checks the state
//! ([`check_rebase_state`]); the backend saves the branch in a backup and executes
//! ([`GitBackend::rebase`]). **A stop (conflict, failed hook or
//! signing) leaves the rebase in progress**: what `rebase.continue` needs to conclude it (branch,
//! value before, backup, messages still to apply) is the [`RebaseRecord`], kept by the backend
//! next to git's own data and removed with it. The pull with rebase uses the same record.

use super::{GitBackend, check_no_operation, conflicted_paths, short};
use crate::{EngineError, Message, quoted_list};
use project_domain::{Commit, EntryKind, Oid, PlanError, PlanErrorReason, RebasePlan, Status};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// The file, in git's rebase folder (`<git dir>/rebase-merge/`), with the [`RebaseRecord`]:
/// git removes it with the folder when the rebase ends or is aborted.
pub const REBASE_RECORD: &str = "project-rebase.json";

/// The most commits `rebase.plan`/`rebase.start` accept in the range.
pub const MAX_REBASE_COMMITS: usize = 2000;

/// What the app knows of a rebase it started and that stopped: `rebase.continue` reads it to
/// apply the remaining messages and to record the concluded rebase in the journal. A rebase
/// without a record was started outside the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebaseRecord {
    /// The command that started it: `rebase.start` or `remote.pull`.
    pub command: String,
    /// The branch being rewritten (short name).
    pub branch: String,
    /// The value of the branch before the rebase.
    pub before: Oid,
    /// The commit the branch is rebuilt on: the plan's `base`, or the upstream for the pull.
    pub onto: Oid,
    /// The backup ref of the branch as it was.
    pub backup: String,
    /// The messages of the plan's `break` stops, in order (empty for the pull).
    #[serde(default)]
    pub messages: Vec<String>,
    /// For each of `messages`, the commit whose step the `break` follows: the backend applies a
    /// message only if the step executed before the `break` is that commit's.
    #[serde(default)]
    pub message_commits: Vec<Oid>,
    /// How many of `messages` were already applied (or skipped).
    #[serde(default)]
    pub applied: usize,
    /// The commits of the range being rewritten (for the description).
    #[serde(default)]
    pub commits: usize,
    /// The commits the plan produces (`None` for the pull, where git drops those already
    /// upstream): checked when the rebase concludes.
    #[serde(default)]
    pub produces: Option<usize>,
    /// For the pull: the remote-tracking branch (short name, e.g. `origin/main`).
    #[serde(default)]
    pub upstream: Option<String>,
}

impl RebaseRecord {
    /// The record as saved in [`REBASE_RECORD`].
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("the record is serializable")
    }

    /// The record read from [`REBASE_RECORD`]; `None` if it is not valid.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }
}

/// What `rebase.start` asks of the backend ([`GitBackend::rebase`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseRequest {
    /// The branch of HEAD (short name): the rebase does not start with detached HEAD.
    pub branch: String,
    /// The value of the branch read earlier: the backend refuses if it changed.
    pub expected: Oid,
    /// The plan, already validated against `commits`.
    pub plan: RebasePlan,
    /// The range `plan.base..expected` read by the command, oldest first: the backend reads it
    /// again and refuses if it changed.
    pub commits: Vec<Oid>,
}

impl RebaseRequest {
    /// The record of the rebase started by this request, with the backup `backup`.
    pub fn record(&self, backup: &str) -> RebaseRecord {
        RebaseRecord {
            command: "rebase.start".to_owned(),
            branch: self.branch.clone(),
            before: self.expected,
            onto: self.plan.base,
            backup: backup.to_owned(),
            messages: self.plan.todo().messages,
            message_commits: self
                .plan
                .steps
                .iter()
                .filter(|s| s.message.is_some())
                .map(|s| s.commit)
                .collect(),
            applied: 0,
            commits: self.commits.len(),
            produces: Some(self.plan.result().len()),
            upstream: None,
        }
    }
}

/// The plan with git's default message made explicit for every group with a `squash` and no
/// message of its own: `message_of(commit)` (the original messages) of the `pick`/`reword` and
/// `squash` steps of the group, joined by a blank line, on the group's last step. Git would
/// otherwise build it from its comment template, whose cleanup depends on `commit.cleanup` (with
/// `verbatim`/`whitespace` the `#` lines stay in the message, with `strip` a `#` line of the
/// original messages goes). Used by the `CliBackend`; the `MemoryBackend` joins the same way.
pub fn explicit_squash_messages(
    plan: &RebasePlan,
    mut message_of: impl FnMut(&Oid) -> Result<String, EngineError>,
) -> Result<RebasePlan, EngineError> {
    use project_domain::RebaseAction;
    let mut out = plan.clone();
    let kept: Vec<usize> = (0..plan.steps.len())
        .filter(|&i| plan.steps[i].action != RebaseAction::Drop)
        .collect();
    let mut k = 0;
    while k < kept.len() {
        let start = k;
        k += 1;
        while k < kept.len() && plan.steps[kept[k]].action.is_combine() {
            k += 1;
        }
        let group = &kept[start..k];
        let last = *group.last().expect("a group has a step");
        let squashes = group
            .iter()
            .any(|&i| plan.steps[i].action == RebaseAction::Squash);
        if !squashes || plan.steps[last].message.is_some() {
            continue;
        }
        let mut parts = Vec::new();
        for &i in group {
            if plan.steps[i].action != RebaseAction::Fixup {
                parts.push(message_of(&plan.steps[i].commit)?.trim().to_owned());
            }
        }
        out.steps[last].message = Some(parts.join("\n\n"));
    }
    Ok(out)
}

/// The checks before `rebase.start`, the same for the backends, on the state read now
/// ([`EngineError::NotAllowed`], nothing changes): an operation in progress, conflicted files,
/// **any** change to tracked files (staged or not: never autostash; untracked files are fine).
pub fn check_rebase_state(status: &Status) -> Result<(), EngineError> {
    check_no_operation("rebase.start", msg!("engine-busy-rebase"), status.operation)?;
    let unmerged = conflicted_paths(status);
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-rebase-conflicts",
            files = quoted_list(&unmerged)
        )));
    }
    let changed: Vec<String> = status
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
            ) && (e.index.is_change() || e.worktree.is_change())
        })
        .map(|e| e.path.to_string())
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    Err(EngineError::NotAllowed(msg!(
        "engine-rebase-local-changes",
        files = quoted_list(&changed)
    )))
}

/// A rebase must be in progress (for `rebase.continue` and `rebase.abort`), otherwise
/// [`EngineError::NotAllowed`].
pub fn check_rebasing(command: &str, status: &Status) -> Result<(), EngineError> {
    super::check_in_progress(command, project_domain::Operation::Rebase, status)
}

/// The commits of `base..head`, oldest first, for `rebase.plan` and `rebase.start` (and again in
/// the backends). Refused ([`EngineError::NotAllowed`]): `base` equal to `head` or not one of its
/// ancestors, merge commits in the range, more than [`MAX_REBASE_COMMITS`] commits.
pub fn rebase_range(
    repo: &dyn GitBackend,
    id: &str,
    head: &Oid,
    base: &Oid,
) -> Result<Vec<Commit>, EngineError> {
    if head == base {
        return Err(EngineError::NotAllowed(msg!(
            "engine-rebase-base-is-head",
            base = short(base),
            id = id
        )));
    }
    let not_ancestor = || {
        EngineError::NotAllowed(msg!(
            "engine-rebase-not-ancestor",
            base = short(base),
            id = id
        ))
    };
    if repo.unmerged_count(base, &[*head])? != 0 {
        return Err(not_ancestor());
    }
    let found = repo.missing_commits(head, &[*base], MAX_REBASE_COMMITS + 1)?;
    if found.len() > MAX_REBASE_COMMITS {
        return Err(EngineError::NotAllowed(msg!(
            "engine-rebase-too-many",
            max = MAX_REBASE_COMMITS,
            id = id
        )));
    }
    let merges: Vec<String> = found
        .iter()
        .filter(|c| c.parents.len() > 1)
        .map(|c| short(&c.id))
        .collect();
    if !merges.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-rebase-merges",
            commits = quoted_list(&merges),
            id = id
        )));
    }
    // Without merges the range is a chain: from HEAD along the parents down to `base`.
    let mut by_id: HashMap<Oid, Commit> = found.into_iter().map(|c| (c.id, c)).collect();
    let mut chain = Vec::with_capacity(by_id.len());
    let mut next = *head;
    while next != *base {
        let commit = by_id.remove(&next).ok_or_else(not_ancestor)?;
        next = commit.parents.first().copied().ok_or_else(not_ancestor)?;
        chain.push(commit);
    }
    chain.reverse();
    Ok(chain)
}

/// The commits the plan rewrites or drops: those of `commits` (oldest first) from the first step
/// that differs from a plain `pick` in the original order (git keeps the ones before as they are).
pub fn rewritten_commits<'a>(plan: &RebasePlan, commits: &'a [Oid]) -> &'a [Oid] {
    let first = plan
        .steps
        .iter()
        .zip(commits)
        .position(|(s, c)| {
            s.action != project_domain::RebaseAction::Pick || s.commit != *c || s.message.is_some()
        })
        .unwrap_or(plan.steps.len().min(commits.len()));
    &commits[first..]
}

/// The [`EngineError::InvalidParams`] of `id` for a plan that is not valid: one key per reason,
/// with the step (from 1) when the error concerns one.
pub fn plan_error(id: &'static str, error: &PlanError) -> EngineError {
    let reason = match &error.reason {
        PlanErrorReason::UnknownCommit(c) => {
            msg!("engine-rebase-plan-unknown-commit", commit = short(c))
        }
        PlanErrorReason::DuplicateCommit(c) => {
            msg!("engine-rebase-plan-duplicate-commit", commit = short(c))
        }
        PlanErrorReason::MissingCommit(c) => {
            msg!("engine-rebase-plan-missing-commit", commit = short(c))
        }
        PlanErrorReason::AllDropped => msg!("engine-rebase-plan-all-dropped"),
        PlanErrorReason::NothingToCombine => msg!("engine-rebase-plan-nothing-to-combine"),
        PlanErrorReason::MissingMessage => msg!("engine-rebase-plan-missing-message"),
        PlanErrorReason::UnexpectedMessage => msg!("engine-rebase-plan-unexpected-message"),
        PlanErrorReason::RewordInGroup => msg!("engine-rebase-plan-reword-in-group"),
        PlanErrorReason::EmptyMessage => msg!("engine-rebase-plan-empty-message"),
        PlanErrorReason::NulInMessage => msg!("engine-rebase-plan-nul-in-message"),
    };
    let reason: Message = match error.step {
        Some(i) => msg!("engine-rebase-plan-step", step = i + 1, reason = reason),
        None => reason,
    };
    EngineError::InvalidParams { id, reason }
}

/// The checks the backends repeat right before writing `request`: the state
/// ([`check_rebase_state`]), the range read again equal to the one of the command, the plan
/// valid against it.
pub fn check_rebase_request(
    repo: &dyn GitBackend,
    request: &RebaseRequest,
) -> Result<(), EngineError> {
    let range: Vec<Oid> =
        rebase_range(repo, "rebase.start", &request.expected, &request.plan.base)?
            .iter()
            .map(|c| c.id)
            .collect();
    if range != request.commits {
        return Err(EngineError::NotAllowed(msg!("engine-rebase-range-changed")));
    }
    request
        .plan
        .validate(&range)
        .map_err(|e| plan_error("rebase.start", &e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{RebaseAction, RebaseStep};

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; Oid::LEN])
    }

    fn step(action: RebaseAction, n: u8) -> RebaseStep {
        RebaseStep {
            action,
            commit: oid(n),
            message: None,
        }
    }

    #[test]
    fn rewritten_from_the_first_change() {
        let commits = [oid(1), oid(2), oid(3)];
        let mut plan = RebasePlan::pick_all(oid(0), &commits);
        assert!(rewritten_commits(&plan, &commits).is_empty());
        plan.steps[2] = step(RebaseAction::Fixup, 3);
        assert_eq!(rewritten_commits(&plan, &commits), [oid(3)]);
        plan.steps.swap(0, 1);
        assert_eq!(rewritten_commits(&plan, &commits), commits);
    }

    #[test]
    fn plan_errors_name_the_step() {
        let e = plan_error(
            "rebase.start",
            &PlanError {
                step: Some(1),
                reason: PlanErrorReason::MissingMessage,
            },
        );
        assert!(matches!(e, EngineError::InvalidParams { .. }));
        assert!(e.to_string().contains("step 2"), "{e}");
        let e = plan_error(
            "rebase.start",
            &PlanError {
                step: None,
                reason: PlanErrorReason::AllDropped,
            },
        );
        assert!(!e.to_string().contains("step"), "{e}");
    }
}
