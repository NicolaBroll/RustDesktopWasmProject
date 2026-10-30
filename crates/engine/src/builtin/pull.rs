//! `remote.pull` (S26): fetch of the upstream's branch, then fast-forward, merge or rebase onto the
//! local one, driven by the engine (`impl-docs/git/sync.md`).
//!
//! Why not `git pull`: with two separate steps the command knows what arrived before
//! writing, decides the strategy with the user's configuration, checks the repo state
//! (no `--autostash`, no merge or rebase with local changes), saves the backup, and the
//! fetch stays cancellable while the integration is not interrupted halfway.

use super::remote::{RefUpdate, changes, unexpected};
use super::{JournalState, Out, reread, to_json};
use crate::{
    EngineError, FetchRequest, HeadState, JournalEntry, Message, PullMode, PullPolicy, PullRequest,
    PullStrategy, Session, StrategySource, branch_ref, check_branch_name, check_pull_state,
    check_remote_name, merge_message, pull_mode, unknown_remote,
};
use project_domain::remote_short;
use project_domain::{Head, Oid, RefInfo, RepoPath, Status};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How `remote.pull` integrated the upstream (`strategy` of [`RemotePullResult`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullApplied {
    /// The branch already contained the upstream: nothing done.
    UpToDate,
    /// The branch advanced to the upstream.
    FastForward,
    /// A merge commit.
    Merge,
    /// The local commits rewritten on top of the upstream.
    Rebase,
}

impl From<&PullMode> for PullApplied {
    fn from(mode: &PullMode) -> Self {
        match mode {
            PullMode::FastForward => Self::FastForward,
            PullMode::Merge { .. } => Self::Merge,
            PullMode::Rebase { .. } => Self::Rebase,
        }
    }
}

/// The fetch done by `remote.pull` (`fetch` of [`RemotePullResult`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PullFetch {
    /// The refs changed by the fetch, in name order.
    pub updated: Vec<RefUpdate>,
    /// The git lines of the fetch that are not progress, in order.
    pub messages: Vec<String>,
}

/// Result of `remote.pull`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemotePullResult {
    /// The local branch pulled (short name).
    pub branch: String,
    /// The upstream's remote.
    pub remote: String,
    /// The upstream's remote-tracking branch (`refs/remotes/origin/main`).
    pub upstream: String,
    /// How the upstream was integrated.
    pub strategy: PullApplied,
    /// Where the strategy comes from.
    pub strategy_source: StrategySource,
    /// The branch before the pull.
    pub before: Oid,
    /// The branch after the pull (the one from before if nothing moved or the merge or rebase
    /// stopped).
    pub after: Oid,
    /// The branch's commits the upstream does not have.
    pub ahead: usize,
    /// The upstream's commits the branch does not have.
    pub behind: usize,
    /// The merge or rebase stopped and stays in progress.
    pub in_progress: bool,
    /// The conflicted files of the stopped merge or rebase (empty otherwise).
    pub conflicts: Vec<RepoPath>,
    /// The git text of the stopped merge or rebase (empty otherwise).
    pub message: String,
    /// The backup ref of the branch before the integration (`refs/project/backup/pull-...`);
    /// `null` when up to date.
    pub backup: Option<String>,
    /// The fetch of the upstream's branch.
    pub fetch: PullFetch,
    /// Unexpected refs moved by the fetch and the warnings of the integration.
    pub warnings: Vec<Message>,
    /// The status re-read after the pull; `null` if it could not be re-read (see `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The refs re-read after the pull; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

const ID: &str = "remote.pull";

/// Parameters of `remote.pull`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemotePullParams {
    /// The remote; without: the upstream's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The local branch; without: HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The strategy instead of the configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strategy: Option<PullStrategy>,
}

/// [`EngineError::NotAllowed`] for a branch without upstream, with how to set it.
fn no_upstream(branch: &str) -> EngineError {
    let b = branch.escape_debug();
    EngineError::NotAllowed(msg!("engine-pull-no-upstream", branch = b))
}

/// Fetches the branch's upstream (`branch`, or the one of HEAD) and integrates it: fast-forward,
/// merge or rebase according to the configuration (`branch.<name>.rebase`, `pull.rebase`, `pull.ff`)
/// or the `strategy` parameter; without configuration merge (see
/// [`PullSettings::policy`](crate::PullSettings::policy)). A branch that is not the one of HEAD
/// can only be fast-forwarded (fast-forward of the ref, index and worktree are not touched).
///
/// In this order, and nothing changes in the branch if a step fails:
///
/// 1. detached HEAD without `branch`, branch without commits, operation in progress, conflicted
///    files, branch without upstream (or with local upstream `.`), `remote` different from the
///    upstream's, invalid configuration → [`EngineError::NotAllowed`], nothing fetched;
/// 2. fetch of only the upstream's branch ([`GitRemotes::fetch`](crate::GitRemotes::fetch):
///    progress and cancellation from the [`JobContext`](crate::JobContext), errors `Auth`,
///    `RemoteUnreachable`, `Cancelled`); a cancellation requested during the fetch stops the
///    command even if the fetch finished, before integrating;
/// 3. the strategy ([`pull_mode`]): nothing to do if the branch already contains the upstream;
/// 4. the integration ([`GitRemotes::pull`](crate::GitRemotes::pull)): state checks
///    ([`check_pull_state`]), branch backup, then merge/rebase. A stopped **merge**
///    (conflicts, hooks, signing) **stays in progress** as with `merge.start`: it is not an error, the
///    result has `in_progress: true` and the files in `conflicts`, the branch has not moved, the
///    backup remains; it is concluded with `merge.continue` or aborted with `merge.abort`. A stopped
///    **rebase** stays in progress the same way: `rebase.continue` concludes it with the pull's
///    journal entry, `rebase.abort` brings the branch back.
///
/// The branch move goes into the journal: `edit.undo` brings it back to where it was (with index and
/// worktree, like `git reset --keep`, if it is the active branch). The fetch does not (§7), neither does a stopped
/// merge or rebase (the entry arrives with `merge.continue`/`rebase.continue`).
///
/// Result: [`RemotePullResult`].
pub(super) fn remote_pull(
    s: &mut Session,
    RemotePullParams {
        remote,
        branch,
        strategy,
    }: RemotePullParams,
) -> Out {
    if let Some(remote) = &remote {
        check_remote_name(ID, remote)?;
    }
    if let Some(branch) = &branch {
        check_branch_name(ID, branch)?;
    }
    let job = s.job().clone();
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let head_branch = match &status.head {
        Head::Branch { name, .. } => Some(name.clone()),
        Head::Detached(_) => None,
    };
    let branch = match branch {
        Some(branch) => branch,
        None => head_branch
            .clone()
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-pull-detached")))?,
    };
    let current = head_branch.as_deref() == Some(branch.as_str());
    let full = branch_ref(&branch);
    let refs = repo.refs()?;
    let before = refs
        .iter()
        .find(|r| r.name == full)
        .map(|r| r.target)
        .ok_or_else(|| {
            EngineError::NotAllowed(msg!(
                "engine-pull-no-branch-commits",
                branch = branch.escape_debug()
            ))
        })?;
    // Before fetching: an operation in progress or a conflict would stop the pull anyway.
    check_pull_state(&status, &PullMode::FastForward)?;

    let upstream = repo
        .upstream(&branch)?
        .ok_or_else(|| no_upstream(&branch))?;
    if upstream.remote == "." {
        return Err(EngineError::NotAllowed(msg!(
            "engine-pull-local-upstream",
            branch = branch.escape_debug(),
            merge = upstream.merge
        )));
    }
    if let Some(remote) = &remote
        && *remote != upstream.remote
    {
        return Err(EngineError::NotAllowed(msg!(
            "engine-pull-other-remote",
            branch = branch.escape_debug(),
            upstream = upstream.remote.escape_debug(),
            remote = remote.escape_debug()
        )));
    }
    let remotes = repo.remotes()?;
    if !remotes.contains(&upstream.remote) {
        return Err(unknown_remote(&upstream.remote, &remotes));
    }
    let merge_branch = upstream.merge_branch().ok_or_else(|| {
        EngineError::NotAllowed(msg!(
            "engine-pull-upstream-not-branch",
            branch = branch.escape_debug(),
            merge = upstream.merge
        ))
    })?;
    let tracking = upstream.tracking.clone().ok_or_else(|| {
        EngineError::NotAllowed(msg!(
            "engine-pull-no-refspec",
            remote = upstream.remote.escape_debug(),
            merge = upstream.merge
        ))
    })?;
    // The strategy is read before the fetch: an invalid configuration fetches nothing.
    let policy = if current {
        repo.pull_settings(&branch)?.policy(strategy)?
    } else {
        PullPolicy {
            strategy: PullStrategy::Merge,
            no_ff: false,
            rebase_merges: false,
            source: StrategySource::Default,
        }
    };

    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let fetched = repo
        .fetch(
            &FetchRequest {
                remote: upstream.remote.clone(),
                branch: Some(merge_branch.to_owned()),
                prune: false,
            },
            &job,
        )
        .map_err(|e| match e {
            EngineError::Cancelled { .. } => EngineError::Cancelled { command: ID },
            other => other,
        })?;
    let after_fetch = repo.refs()?;
    let updated = changes(&refs, &after_fetch);
    let mut warnings: Vec<Message> = updated.iter().filter_map(unexpected).collect();
    let fetch = PullFetch {
        updated,
        messages: fetched.messages,
    };
    let upstream_oid = after_fetch
        .iter()
        .find(|r| r.name == tracking)
        .map(|r| r.target)
        .ok_or_else(|| {
            EngineError::NotAllowed(msg!("engine-pull-tracking-missing", tracking = tracking))
        })?;
    // Cancelled during the fetch: no integration.
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }

    let ahead = repo.unmerged_count(&before, &[upstream_oid])?;
    let behind = repo.unmerged_count(&upstream_oid, &[before])?;
    let mode = pull_mode(&policy, &branch, current, ahead, behind)?;
    let (strategy, after, backup, in_progress, conflicts, message) = match mode {
        None => (
            PullApplied::UpToDate,
            before,
            None,
            false,
            Vec::new(),
            String::new(),
        ),
        Some(mode) => {
            let outcome = repo.pull(&PullRequest {
                branch: branch.clone(),
                current,
                expected: before,
                upstream: upstream_oid,
                upstream_name: tracking.clone(),
                mode,
                message: merge_message(&tracking, &branch),
            })?;
            // The branch moved: the entry goes into the journal even if something cannot be re-read afterwards.
            // A stopped merge does not: the branch has not moved, the entry arrives with `merge.continue`.
            if !outcome.in_progress && outcome.after != before {
                journal.record(JournalEntry::pull(
                    &full,
                    before,
                    outcome.after,
                    HeadState::from_head(&status.head),
                    (&mode).into(),
                    remote_short(&tracking),
                    outcome.backup.clone(),
                ));
            }
            warnings.extend(outcome.warnings);
            (
                (&mode).into(),
                outcome.after,
                Some(outcome.backup),
                outcome.in_progress,
                outcome.conflicts,
                outcome.message,
            )
        }
    };
    let (status, status_error) = reread(repo.status());
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(RemotePullResult {
        branch,
        remote: upstream.remote,
        upstream: tracking,
        strategy,
        strategy_source: policy.source,
        before,
        after,
        ahead,
        behind,
        in_progress,
        conflicts,
        message,
        backup,
        fetch,
        warnings,
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
