//! `remote.delete_branch`: deletes a branch from the remote (`impl-docs/git/sync.md`).

use super::remote::{changes, unexpected};
use super::{Out, to_json};
use crate::commands::parse_params;
use crate::git::short;
use crate::{
    EngineError, Message, MessageArg, RemoteBranchDelete, Session, branch_ref, check_branch_name,
    check_remote_name, tracking_ref, unknown_remote,
};
use project_domain::{Oid, RefKind};
use serde::Deserialize;
use serde_json::{Value, json};

const ID: &str = "remote.delete_branch";

/// Commits listed at most in the confirmation.
const LOST_LIMIT: usize = 10;

/// Parameters of `remote.delete_branch`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteParams {
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
/// Result: `{"remote", "branch", "tracking", "deleted": oid, "only_here": n, "lost": [Commit],
/// "upstream_of": [branch], "backup", "summary", "messages": [string], "incomplete": bool, "updated": [{"name",
/// "before", "after"}] | null, "warnings": [string], "status": Status | null, "refs": [RefInfo] |
/// null}`: `only_here` and `lost` are the commits of the deleted branch that no other ref here
/// reaches (only the backup keeps them), `upstream_of` the local branches with it as upstream.
pub(super) fn remote_delete_branch(s: &mut Session, p: Value) -> Out {
    let DeleteParams {
        remote,
        branch,
        confirm,
        expected,
    } = parse_params(ID, p)?;
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
                        id = short(&c.id),
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
    let mut out = json!({
        "remote": remote,
        "branch": branch,
        "tracking": tracking,
        "deleted": theirs.to_string(),
        "only_here": only_here,
        "lost": to_json(&lost),
        "upstream_of": upstream_of,
        "backup": deleted.backup,
        "summary": deleted.summary,
        "messages": deleted.messages,
        "incomplete": deleted.incomplete,
    });
    let incomplete = deleted.incomplete.then(|| {
        msg!(
            "engine-remote-branch-incomplete",
            remote = remote.as_str(),
            branch = branch.as_str(),
            backup = deleted.backup.as_str()
        )
    });
    match repo.refs() {
        Ok(after) => {
            let updated = changes(&refs_before, &after);
            let warnings: Vec<Message> = incomplete
                .into_iter()
                .chain(updated.iter().filter_map(unexpected))
                .collect();
            out["updated"] = to_json(updated);
            out["warnings"] = to_json(warnings);
            out["refs"] = to_json(after);
        }
        Err(e) => {
            out["updated"] = Value::Null;
            out["warnings"] = to_json(incomplete.into_iter().collect::<Vec<_>>());
            out["refs"] = Value::Null;
            out["refs_error"] = Value::String(e.to_string());
        }
    }
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
    Ok(out)
}
