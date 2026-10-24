//! `remote.pull` (S26): fetch of the upstream's branch, then fast-forward, merge or rebase onto the
//! local one, driven by the engine (`impl-docs/git/sync.md`).
//!
//! Why not `git pull`: with two separate steps the command knows what arrived before
//! writing, decides the strategy with the user's configuration, checks the repo state
//! (no `--autostash`, no merge or rebase with local changes), saves the backup, and the
//! fetch stays cancellable while the integration is not interrupted halfway.

use super::remote::{changes, unexpected};
use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, FetchRequest, HeadState, JournalEntry, Message, PullMode, PullPolicy, PullRequest,
    PullStrategy, Session, StrategySource, branch_ref, check_branch_name, check_pull_state,
    check_remote_name, merge_message, pull_mode, unknown_remote,
};
use project_domain::Head;
use serde::Deserialize;
use serde_json::{Value, json};

const ID: &str = "remote.pull";

/// Parameters of `remote.pull`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PullParams {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    strategy: Option<PullStrategy>,
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
/// 2. fetch of only the upstream's branch ([`GitBackend::fetch`](crate::GitBackend::fetch):
///    progress and cancellation from the [`JobContext`](crate::JobContext), errors `Auth`,
///    `RemoteUnreachable`, `Cancelled`); a cancellation requested during the fetch stops the
///    command even if the fetch finished, before integrating;
/// 3. the strategy ([`pull_mode`]): nothing to do if the branch already contains the upstream;
/// 4. the integration ([`GitBackend::pull`](crate::GitBackend::pull)): state checks
///    ([`check_pull_state`]), branch backup, then merge/rebase. A stopped **merge**
///    (conflicts, hooks, signing) **stays in progress** as with `merge.start`: it is not an error, the
///    result has `in_progress: true` and the files in `conflicts`, the branch has not moved, the
///    backup remains; it is concluded with `merge.continue` or aborted with `merge.abort`. A stopped
///    **rebase** is aborted and gives [`EngineError::PullConflict`] (until there are
///    `rebase.continue`/`rebase.abort`, S32).
///
/// The branch move goes into the journal: `edit.undo` brings it back to where it was (with index and
/// worktree, like `git reset --keep`, if it is the active branch). The fetch does not (§7), neither does a stopped
/// merge (the entry arrives with `merge.continue`).
///
/// Result: `{"branch", "remote", "upstream", "strategy": "fast_forward" | "merge" | "rebase"
/// | "up_to_date", "strategy_source": "param" | "config" | "default", "before", "after",
/// "ahead", "behind", "in_progress", "conflicts": [path], "message", "backup": string |
/// null, "fetch": {"updated", "messages"}, "warnings", "status": Status | null, "refs":
/// [RefInfo] | null, "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn remote_pull(s: &mut Session, p: Value) -> Out {
    let PullParams {
        remote,
        branch,
        strategy,
    } = parse_params(ID, p)?;
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
    let fetch = json!({ "updated": to_json(&updated), "messages": fetched.messages });
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
    let mut out = json!({
        "branch": branch,
        "remote": upstream.remote,
        "upstream": tracking,
        "strategy_source": to_json(policy.source),
        "before": before.to_string(),
        "ahead": ahead,
        "behind": behind,
        "in_progress": false,
        "conflicts": [],
        "message": "",
        "fetch": fetch,
    });
    let (after, backup) = match mode {
        None => {
            out["strategy"] = json!("up_to_date");
            (before, None)
        }
        Some(mode) => {
            out["strategy"] = json!(mode.name());
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
                    mode.name(),
                    tracking.strip_prefix("refs/remotes/").unwrap_or(&tracking),
                    outcome.backup.clone(),
                ));
            }
            warnings.extend(outcome.warnings);
            out["in_progress"] = json!(outcome.in_progress);
            out["conflicts"] = to_json(&outcome.conflicts);
            out["message"] = json!(outcome.message);
            (outcome.after, Some(outcome.backup))
        }
    };
    let state = journal_state(journal);
    out["after"] = json!(after.to_string());
    out["backup"] = json!(backup);
    out["warnings"] = to_json(warnings);
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
    Ok(out)
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
