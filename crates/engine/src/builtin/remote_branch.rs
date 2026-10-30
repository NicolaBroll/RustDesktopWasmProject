//! `remote.delete_branch`: deletes a branch from the remote (`impl-docs/git/sync.md`).

use super::push::{Lost, config_true, parse_expected};
use super::remote::{RefUpdate, RefsAfter};
use super::{Out, reread, to_json};
use crate::{
    EngineError, GitBackend, Message, RemoteBranchDelete, Session, branch_ref, check_branch_name,
    check_remote_name, tracking_ref, unknown_remote,
};
use project_domain::{Commit, Oid, RefInfo, RefKind, Status};
use schemars::JsonSchema;
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

/// Parameters of `remote.delete_branch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteDeleteBranchParams {
    /// The remote.
    pub remote: String,
    /// The name of the branch on the remote.
    pub branch: String,
    /// Always required to delete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
    /// The full id of the remote-tracking branch shown in the confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
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
    RemoteDeleteBranchParams {
        remote,
        branch,
        confirm,
        expected,
    }: RemoteDeleteBranchParams,
) -> Out {
    check_remote_name(ID, &remote)?;
    check_branch_name(ID, &branch)?;
    if confirm && expected.is_none() {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-push-confirm-without-expected"),
        });
    }
    let expected = parse_expected(ID, expected.as_deref())?;
    let job = s.job().clone();
    let repo = s.repo_mut()?;
    check_remote(repo, &remote)?;
    let refspecs = repo.config_values(&format!("remote.{remote}.fetch"))?;
    let tracking = tracking_ref(&refspecs, &branch_ref(&branch)).ok_or_else(|| {
        EngineError::NotAllowed(msg!(
            "engine-remote-branch-no-tracking",
            remote = remote.escape_debug(),
            branch = branch.escape_debug()
        ))
    })?;
    let refs_before = repo.refs()?;
    let seen = Seen::read(repo, &refs_before, &remote, tracking)?;
    seen.check_confirmation(&remote, &branch, expected, confirm)?;
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let request = RemoteBranchDelete {
        remote: remote.clone(),
        branch: branch.clone(),
        expected: seen.theirs,
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
        tracking: seen.tracking,
        deleted: seen.theirs,
        only_here: seen.lost.count,
        lost: seen.lost.commits,
        upstream_of: seen.upstream_of,
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

/// The remote must be configured and not a mirror.
fn check_remote(repo: &dyn GitBackend, remote: &str) -> Result<(), EngineError> {
    let remotes = repo.remotes()?;
    if !remotes.iter().any(|r| r == remote) {
        return Err(unknown_remote(remote, &remotes));
    }
    let mirror = repo
        .config_values(&format!("remote.{remote}.mirror"))?
        .into_iter()
        .last();
    if mirror.as_deref().is_some_and(config_true) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-branch-mirror",
            remote = remote.escape_debug()
        )));
    }
    Ok(())
}

/// The remote branch as the local remote-tracking branch shows it: what the confirmation is about.
struct Seen {
    tracking: String,
    /// The commit of the remote-tracking branch.
    theirs: Oid,
    /// The local branches with it as upstream.
    upstream_of: Vec<String>,
    /// The commits that, once the branch is deleted, only the backup would keep.
    lost: Lost,
}

impl Seen {
    fn read(
        repo: &dyn GitBackend,
        refs: &[RefInfo],
        remote: &str,
        tracking: String,
    ) -> Result<Self, EngineError> {
        let theirs = refs
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
        let upstream_of = refs
            .iter()
            .filter(|r| r.kind == RefKind::LocalBranch)
            .filter(|r| r.upstream.as_ref().is_some_and(|up| up.name == tracking))
            .map(|r| r.short_name().to_owned())
            .collect();
        let others: Vec<Oid> = refs
            .iter()
            .filter(|r| r.name != tracking)
            .map(|r| r.target)
            .collect();
        let lost = Lost::read(repo, theirs, &others)?;
        Ok(Self {
            tracking,
            theirs,
            upstream_of,
            lost,
        })
    }

    /// The commits only here and the branches with it as upstream, for the confirmation.
    fn details(&self) -> (Message, Message) {
        let only = if self.lost.count == 0 {
            msg!("engine-remote-branch-only-none")
        } else {
            msg!(
                "engine-remote-branch-only",
                n = self.lost.count,
                commits = self.lost.describe()
            )
        };
        let upstreams = if self.upstream_of.is_empty() {
            msg!("engine-other", text = "")
        } else {
            let names = self
                .upstream_of
                .iter()
                .map(|b| format!("`{b}`"))
                .collect::<Vec<_>>()
                .join(", ");
            msg!(
                "engine-remote-branch-upstream-of",
                n = self.upstream_of.len(),
                branches = names
            )
        };
        (only, upstreams)
    }

    /// Deleting always needs `confirm`, and the confirmation holds only for the commit seen.
    fn check_confirmation(
        &self,
        remote: &str,
        branch: &str,
        expected: Option<Oid>,
        confirm: bool,
    ) -> Result<(), EngineError> {
        if let Some(expected) = expected
            && expected != self.theirs
        {
            let (only, upstreams) = self.details();
            return Err(EngineError::ConfirmRequired {
                command: ID,
                reason: msg!(
                    "engine-remote-branch-moved",
                    tracking = self.tracking.as_str(),
                    expected = expected,
                    now = self.theirs,
                    only = only,
                    upstreams = upstreams
                ),
                expected: Some(self.theirs.to_string()),
            });
        }
        if !confirm {
            let (only, upstreams) = self.details();
            return Err(EngineError::ConfirmRequired {
                command: ID,
                reason: msg!(
                    "engine-remote-branch-confirm",
                    remote = remote,
                    branch = branch,
                    tracking = self.tracking.as_str(),
                    theirs = self.theirs,
                    only = only,
                    upstreams = upstreams
                ),
                expected: Some(self.theirs.to_string()),
            });
        }
        Ok(())
    }
}
