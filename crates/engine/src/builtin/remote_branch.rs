//! `remote.delete_branch`: deletes a branch from the remote (`impl-docs/git/sync.md`).

use super::remote::{RefUpdate, RefsAfter};
use super::{Out, reread, to_json};
use crate::{
    EngineError, Message, MessageArg, RemoteBranchDelete, Session, branch_ref, check_branch_name,
    check_remote_name, tracking_ref, unknown_remote,
};
use project_domain::{Commit, Oid, RefInfo, RefKind, Status};
use serde::{Deserialize, Serialize};

/// Result of `remote.delete_branch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteDeleteBranchResult {
    /// The remote the branch was deleted from.
    pub remote: String,
    /// The branch on the remote (short name).
    pub branch: String,
    /// The local remote-tracking branch (`refs/remotes/origin/<branch>`).
    pub tracking: String,
    /// The commit the branch had (the value of the remote-tracking branch confirmed).
    pub deleted: Oid,
    /// How many commits of the deleted branch no other ref here reaches (only the backup keeps
    /// them).
    pub only_here: usize,
    /// Those commits (at most 10).
    pub lost: Vec<Commit>,
    /// The local branches that have the deleted branch as upstream.
    pub upstream_of: Vec<String>,
    /// The backup ref on the deleted commit (`refs/project/backup/push-delete-...`).
    pub backup: String,
    /// The git summary (`[deleted]`).
    pub summary: String,
    /// The git lines that are not progress (`To <url>`, warnings, hook output).
    pub messages: Vec<String>,
    /// Deleted only on some of the push URLs: the branch may still be on the remote.
    pub incomplete: bool,
    /// The local refs changed (the remote-tracking branch removed), in name order; `null` if the
    /// refs could not be re-read (see `refs_error`).
    pub updated: Option<Vec<RefUpdate>>,
    /// The warning of an incomplete deletion, then unexpected refs moved.
    pub warnings: Vec<Message>,
    /// The status re-read after the deletion; `null` if it could not be re-read (see
    /// `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The refs re-read after the deletion; `null` if they could not be re-read (see
    /// `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
}

const ID: &str = "remote.delete_branch";

/// Commits listed at most in the confirmation.
const LOST_LIMIT: usize = 10;

/// Parameters of `remote.delete_branch`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteParams {
    remote: String,
    branch: String,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    expected: Option<String>,
}

/// Deletes the branch `branch` from the remote `remote` (`git push <remote> :refs/heads/<branch>`):
/// the branch disappears for everyone who uses the remote, so it **always** needs `"confirm": true`
/// with `"expected"`, the full id of the local remote-tracking branch shown by the confirmation.
/// Without them [`EngineError::ConfirmRequired`] says the commit, the commits that are in no other
/// ref here and the local branches that have it as upstream, and nothing changes; an `expected`
/// that is not the value read now asks again.
///
/// The local remote-tracking branch must exist (derived from the fetch refspecs with
/// [`tracking_ref`]): it is what the user saw and what goes into the backup; otherwise
/// [`EngineError::NotAllowed`] asking for a fetch. Refused also for a remote that is not configured
/// or is a mirror. The deletion is always `--force-with-lease=refs/heads/<branch>:<expected>`: if
/// the branch moved on the remote (or is already gone) git rejects it and nothing changes
/// ([`EngineError::PushRejected`] with `stale_lease`). Before the push the commit is saved in
/// `refs/project/backup/push-delete-...`. With several push URLs a deletion that succeeds only on some
/// of them is a success with `"incomplete": true` and a warning (the backup stays).
///
/// It does not go into the journal, like `remote.push`: the remote belongs to others. To put the
/// branch back, push the backup again. The upstream of the local branches stays configured (git
/// shows it as gone). Slow command like the push.
///
/// Result: [`RemoteDeleteBranchResult`]: `only_here` and `lost` are the commits of the deleted branch
/// that no other ref here reaches (only the backup keeps them), `upstream_of` the local branches
/// with it as upstream.
pub(super) fn remote_delete_branch(
    s: &mut Session,
    DeleteParams {
        remote,
        branch,
        confirm,
        expected,
    }: DeleteParams,
) -> Out {
    check_remote_name(ID, &remote)?;
    check_branch_name(ID, &branch)?;
    if confirm && expected.is_none() {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-push-confirm-without-expected"),
        });
    }
    let expected = expected
        .map(|hex| {
            let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
            full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
                .flatten()
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: msg!(
                        "engine-push-expected-hex",
                        len = Oid::HEX_LEN,
                        value = hex.escape_debug()
                    ),
                })
        })
        .transpose()?;
    let job = s.job().clone();
    let repo = s.repo_mut()?;
    let remotes = repo.remotes()?;
    if !remotes.contains(&remote) {
        return Err(unknown_remote(&remote, &remotes));
    }
    let mirror = repo
        .config_values(&format!("remote.{remote}.mirror"))?
        .into_iter()
        .last();
    if mirror.as_deref().is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "" | "true" | "yes" | "on" | "1"
        )
    }) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-branch-mirror",
            remote = remote.escape_debug()
        )));
    }
    let refspecs = repo.config_values(&format!("remote.{remote}.fetch"))?;
    let tracking = tracking_ref(&refspecs, &branch_ref(&branch)).ok_or_else(|| {
        EngineError::NotAllowed(msg!(
            "engine-remote-branch-no-tracking",
            remote = remote.escape_debug(),
            branch = branch.escape_debug()
        ))
    })?;
    let refs_before = repo.refs()?;
    let theirs = refs_before
        .iter()
        .find(|r| r.name == tracking)
        .map(|r| r.target)
        .ok_or_else(|| {
            EngineError::NotAllowed(msg!(
                "engine-remote-branch-unknown",
                tracking = tracking.as_str(),
                remote = remote.escape_debug()
            ))
        })?;
    let upstream_of: Vec<String> = refs_before
        .iter()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .filter(|r| r.upstream.as_ref().is_some_and(|up| up.name == tracking))
        .map(|r| r.short_name().to_owned())
        .collect();
    // The commits that, once the branch is deleted, only the backup would keep.
    let others: Vec<Oid> = refs_before
        .iter()
        .filter(|r| r.name != tracking)
        .map(|r| r.target)
        .collect();
    let only_here = repo.unmerged_count(&theirs, &others)?;
    let lost = if only_here > 0 {
        repo.missing_commits(&theirs, &others, LOST_LIMIT)?
    } else {
        Vec::new()
    };

    let details = || {
        let only = if only_here == 0 {
            msg!("engine-remote-branch-only-none")
        } else {
            let names: Vec<Message> = lost
                .iter()
                .map(|c| {
                    msg!(
                        "engine-push-lost-commit",
                        id = c.id.short(),
                        summary = c.summary
                    )
                })
                .collect();
            let more = only_here.saturating_sub(lost.len());
            let empty = names.is_empty();
            let commits = MessageArg::Joined(names, ", ".into());
            let commits = match (empty, more) {
                (_, 0) => msg!("engine-other", text = commits),
                (true, more) => msg!("engine-push-lost-others", n = more),
                (false, more) => msg!("engine-push-lost-and-others", commits = commits, n = more),
            };
            msg!(
                "engine-remote-branch-only",
                n = only_here,
                commits = commits
            )
        };
        let upstreams = if upstream_of.is_empty() {
            msg!("engine-other", text = "")
        } else {
            let names = upstream_of
                .iter()
                .map(|b| format!("`{b}`"))
                .collect::<Vec<_>>()
                .join(", ");
            msg!(
                "engine-remote-branch-upstream-of",
                n = upstream_of.len(),
                branches = names
            )
        };
        (only, upstreams)
    };
    // The confirmation holds only for the commit seen.
    if let Some(expected) = expected
        && expected != theirs
    {
        let (only, upstreams) = details();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-remote-branch-moved",
                tracking = tracking.as_str(),
                expected = expected,
                now = theirs,
                only = only,
                upstreams = upstreams
            ),
            expected: Some(theirs.to_string()),
        });
    }
    if !confirm {
        let (only, upstreams) = details();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-remote-branch-confirm",
                remote = remote.as_str(),
                branch = branch.as_str(),
                tracking = tracking.as_str(),
                theirs = theirs,
                only = only,
                upstreams = upstreams
            ),
            expected: Some(theirs.to_string()),
        });
    }
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let request = RemoteBranchDelete {
        remote: remote.clone(),
        branch: branch.clone(),
        expected: theirs,
    };
    let deleted = repo.delete_remote_branch(&request, &job)?;
    let incomplete = deleted.incomplete.then(|| {
        msg!(
            "engine-remote-branch-incomplete",
            remote = remote.as_str(),
            branch = branch.as_str(),
            backup = deleted.backup.as_str()
        )
    });
    let after = RefsAfter::new(&refs_before, repo.refs(), incomplete.into_iter().collect());
    let (status, status_error) = reread(repo.status());
    Ok(to_json(RemoteDeleteBranchResult {
        remote,
        branch,
        tracking,
        deleted: theirs,
        only_here,
        lost,
        upstream_of,
        backup: deleted.backup,
        summary: deleted.summary,
        messages: deleted.messages,
        incomplete: deleted.incomplete,
        updated: after.updated,
        warnings: after.warnings,
        status,
        status_error,
        refs: after.refs,
        refs_error: after.refs_error,
    }))
}
