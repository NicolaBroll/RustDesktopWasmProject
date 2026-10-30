//! `remote.push` (S26): publishes a local branch to the remote (`impl-docs/git/sync.md`).

use super::remote::{RefUpdate, RefsAfter};
use super::{Out, reread, to_json};
use crate::{
    EngineError, Message, MessageArg, PushKind, PushRequest, Session, branch_ref,
    check_branch_name, check_remote_name, default_remote, tracking_ref, unknown_remote,
};
use project_domain::{Commit, Head, Oid, RefInfo, Status};
use serde::{Deserialize, Serialize};

/// Where the remote of `remote.push` comes from (`remote_source` of [`RemotePushResult`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushRemoteSource {
    /// The `remote` parameter.
    Param,
    /// `branch.<name>.pushRemote`.
    PushRemote,
    /// `remote.pushDefault`.
    PushDefault,
    /// The remote of the branch's upstream.
    Upstream,
    /// `origin`.
    Origin,
    /// The only configured remote.
    Only,
}

/// Result of `remote.push`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemotePushResult {
    /// The remote pushed to.
    pub remote: String,
    /// Where the remote comes from.
    pub remote_source: PushRemoteSource,
    /// The local branch published (short name).
    pub branch: String,
    /// The branch on the remote (short name).
    pub remote_branch: String,
    /// How the branch on the remote was updated.
    pub kind: PushKind,
    /// The git summary (`a1b2c3d..e4f5a6b`, `[new branch]`, ...).
    pub summary: String,
    /// The local remote-tracking branch before the push; `null` if there was none.
    pub before: Option<Oid>,
    /// The published commit.
    pub after: Oid,
    /// The upstream was set by this push (first push).
    pub set_upstream: bool,
    /// The push was forced (`force`, always with lease).
    pub forced: bool,
    /// The commits a force removed from the remote (at most 10; empty without force).
    pub lost: Vec<Commit>,
    /// The backup ref created before a force (`refs/project/backup/push-...`); `null` if none.
    pub backup: Option<String>,
    /// The local refs changed (the local remote-tracking branch), in name order; `null` if the
    /// refs could not be re-read (see `refs_error`).
    pub updated: Option<Vec<RefUpdate>>,
    /// The git lines that are not progress (`To <url>`, warnings, hook output).
    pub messages: Vec<String>,
    /// Unexpected refs moved by the push; empty if the refs could not be re-read.
    pub warnings: Vec<Message>,
    /// The status re-read after the push; `null` if it could not be re-read (see `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The refs re-read after the push; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
}

const ID: &str = "remote.push";

/// Commits listed at most in the confirmation of a force.
const LOST_LIMIT: usize = 10;

/// Parameters of `remote.push`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PushParams {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
    #[serde(default)]
    force: bool,
    #[serde(default)]
    confirm: bool,
    #[serde(default)]
    expected: Option<String>,
}

/// A true boolean value of git config.
fn config_true(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "" | "true" | "yes" | "on" | "1"
    )
}

/// Publishes the branch `branch` (or the one of HEAD) to the remote, with an explicit refspec
/// `refs/heads/<branch>:refs/heads/<branch>`: never tags or other branches, whatever
/// `push.default`, `remote.<name>.push` or `push.followTags` say.
///
/// Remote, as `git push` does: `remote`, then `branch.<name>.pushRemote`, `remote.pushDefault`, the
/// upstream's remote, `origin` or the only remote (several remotes without `origin`:
/// [`EngineError::NotAllowed`] asking for `remote`); `remote_source` in the result says which.
/// A remote named in the configuration that is not configured, or a mirror remote
/// (`remote.<name>.mirror`) → [`EngineError::NotAllowed`]. Destination: on the upstream's
/// remote the upstream's branch (with a name different from the branch it is refused, like
/// `push.default=simple`), elsewhere the same name, with the local remote-tracking branch derived from the
/// fetch refspecs ([`tracking_ref`]). **First push** (no upstream): the upstream is set
/// (`--set-upstream`) if the push succeeds; an existing upstream never changes.
///
/// Force: only with `"force": true`, and always as `--force-with-lease=refs/heads/<b>:<oid>`
/// (never `--force` or `+refspec`): if the remote moved in the meantime the push is
/// rejected. If the force would remove from the remote commits that the local branch lacks, also
/// `"confirm": true` with `"expected"` is needed, the full id of the local remote-tracking branch shown by the
/// confirmation ([`EngineError::ConfirmRequired`] with the commits and the phrase `"expected": "<id>"`);
/// if `expected` is not the value read now, `ConfirmRequired` again and nothing published. The
/// lease uses `expected` (or the value read now). Before the push the local remote-tracking branch is saved
/// in `refs/project/backup/push-...`. `confirm` without `expected`, `confirm`/`expected` without
/// `force` → [`EngineError::InvalidParams`].
///
/// It does not go into the journal: a push cannot be undone from here (the remote belongs to others); the result says
/// what changed. Slow command like fetch.
///
/// Result: [`RemotePushResult`]: `before` is the local remote-tracking branch before the push, `after`
/// the published commit, `lost` the commits removed from the remote by a force, `updated` the
/// local refs changed (the local remote-tracking branch).
pub(super) fn remote_push(
    s: &mut Session,
    PushParams {
        remote,
        branch,
        force,
        confirm,
        expected,
    }: PushParams,
) -> Out {
    if let Some(remote) = &remote {
        check_remote_name(ID, remote)?;
    }
    if let Some(branch) = &branch {
        check_branch_name(ID, branch)?;
    }
    if (confirm || expected.is_some()) && !force {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-push-confirm-without-force"),
        });
    }
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
    let status = repo.status()?;
    let branch = match branch {
        Some(branch) => branch,
        None => match &status.head {
            Head::Branch { name, .. } => name.clone(),
            Head::Detached(_) => {
                return Err(EngineError::NotAllowed(msg!("engine-push-detached")));
            }
        },
    };
    let full = branch_ref(&branch);
    let refs_before = repo.refs()?;
    let local = refs_before
        .iter()
        .find(|r| r.name == full)
        .map(|r| r.target)
        .ok_or_else(|| {
            EngineError::NotAllowed(msg!(
                "engine-push-no-branch",
                branch = branch.escape_debug()
            ))
        })?;
    let remotes = repo.remotes()?;
    let upstream = repo.upstream(&branch)?;

    // The remote, as `git push` chooses it: `remote`, then `branch.<name>.pushRemote`, then
    // `remote.pushDefault`, then the upstream's remote, then `origin` or the only remote.
    let last = |values: Vec<String>| values.into_iter().last();
    let (remote, remote_source) = match remote {
        Some(r) => (r, PushRemoteSource::Param),
        None => {
            if let Some(r) = last(repo.config_values(&format!("branch.{branch}.pushRemote"))?) {
                (r, PushRemoteSource::PushRemote)
            } else if let Some(r) = last(repo.config_values("remote.pushDefault")?) {
                (r, PushRemoteSource::PushDefault)
            } else {
                match &upstream {
                    Some(up) if up.remote == "." => {
                        return Err(EngineError::NotAllowed(msg!(
                            "engine-push-local-upstream",
                            branch = branch.escape_debug(),
                            merge = up.merge
                        )));
                    }
                    Some(up) => (up.remote.clone(), PushRemoteSource::Upstream),
                    None => {
                        let without = Status {
                            upstream: None,
                            ..status
                        };
                        let r = default_remote(ID, &without, &remotes)?;
                        let source = if r == "origin" {
                            PushRemoteSource::Origin
                        } else {
                            PushRemoteSource::Only
                        };
                        (r, source)
                    }
                }
            }
        }
    };
    if matches!(
        remote_source,
        PushRemoteSource::PushRemote | PushRemoteSource::PushDefault
    ) {
        let key = if remote_source == PushRemoteSource::PushRemote {
            format!("branch.{branch}.pushRemote")
        } else {
            "remote.pushDefault".to_owned()
        };
        if check_remote_name(ID, &remote).is_err() || !remotes.contains(&remote) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-push-bad-remote-config",
                key = key,
                remote = remote.escape_debug(),
                remotes = remotes
                    .iter()
                    .map(|r| format!("`{r}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    if !remotes.contains(&remote) {
        return Err(unknown_remote(&remote, &remotes));
    }
    let mirror = last(repo.config_values(&format!("remote.{remote}.mirror"))?);
    if mirror.as_deref().is_some_and(config_true) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-push-mirror",
            remote = remote.escape_debug()
        )));
    }
    // Branch on the remote, whether to set the upstream and the local remote-tracking branch that mirrors it.
    let (remote_branch, set_upstream, tracking) = match &upstream {
        Some(up) if up.remote == remote => {
            let target = up.merge_branch().ok_or_else(|| {
                EngineError::NotAllowed(msg!(
                    "engine-push-upstream-not-branch",
                    branch = branch.escape_debug(),
                    merge = up.merge
                ))
            })?;
            (target.to_owned(), false, up.tracking.clone())
        }
        _ => {
            let refspecs = repo.config_values(&format!("remote.{remote}.fetch"))?;
            (
                branch.clone(),
                upstream.is_none(),
                tracking_ref(&refspecs, &full),
            )
        }
    };
    if remote_branch != branch {
        return Err(EngineError::NotAllowed(msg!(
            "engine-push-upstream-other-name",
            branch = branch.escape_debug(),
            remote = remote.escape_debug(),
            remote_branch = remote_branch.escape_debug()
        )));
    }
    let remote_value = tracking
        .as_ref()
        .and_then(|t| refs_before.iter().find(|r| r.name == *t))
        .map(|r| r.target);

    // The remote's commits (as the local remote-tracking branch knows them) that a force would remove.
    let (lost, lost_count) = match (force, remote_value) {
        (true, Some(theirs)) => {
            let count = repo.unmerged_count(&theirs, &[local])?;
            let commits = if count > 0 {
                repo.missing_commits(&theirs, &[local], LOST_LIMIT)?
            } else {
                Vec::new()
            };
            (commits, count)
        }
        _ => (Vec::new(), 0),
    };
    let shown = match tracking.as_deref() {
        Some(name) => msg!("engine-other", text = name),
        None => msg!("engine-push-no-tracking"),
    };
    let describe_lost = || {
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
        let more = lost_count.saturating_sub(lost.len());
        let empty = names.is_empty();
        let commits = MessageArg::Joined(names, ", ".into());
        match (empty, more) {
            (_, 0) => msg!("engine-other", text = commits),
            (true, more) => msg!("engine-push-lost-others", n = more),
            (false, more) => msg!("engine-push-lost-and-others", commits = commits, n = more),
        }
    };
    // The confirmation holds only for the commit seen: `expected` must be the local remote-tracking branch
    // read now.
    if let Some(expected) = expected
        && Some(expected) != remote_value
    {
        let reason = match remote_value {
            Some(now) => msg!(
                "engine-push-moved",
                shown = shown,
                expected = expected,
                now = now,
                n = lost_count,
                branch = branch,
                commits = describe_lost()
            ),
            None => msg!(
                "engine-push-moved-gone",
                shown = shown,
                expected = expected,
                n = lost_count,
                branch = branch,
                commits = describe_lost()
            ),
        };
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason,
            expected: remote_value.map(|o| o.to_string()),
        });
    }
    if force && lost_count > 0 && !confirm {
        let theirs = remote_value.expect("lost_count > 0 only with a local remote-tracking branch");
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-push-force-confirm",
                remote = remote,
                remote_branch = remote_branch,
                shown = shown,
                theirs = theirs,
                n = lost_count,
                branch = branch,
                commits = describe_lost()
            ),
            expected: Some(theirs.to_string()),
        });
    }
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let request = PushRequest {
        remote: remote.clone(),
        branch: branch.clone(),
        remote_branch: remote_branch.clone(),
        set_upstream,
        lease: force.then_some(expected.or(remote_value)),
        backup: (force && lost_count > 0).then_some(remote_value).flatten(),
    };
    let outcome = repo.push(&request, &job)?;
    let after = RefsAfter::new(&refs_before, repo.refs(), Vec::new());
    let (status, status_error) = reread(repo.status());
    Ok(to_json(RemotePushResult {
        remote,
        remote_source,
        branch,
        remote_branch,
        kind: outcome.kind,
        summary: outcome.summary,
        before: remote_value,
        after: local,
        set_upstream,
        forced: force,
        lost,
        backup: outcome.backup,
        updated: after.updated,
        messages: outcome.messages,
        warnings: after.warnings,
        status,
        status_error,
        refs: after.refs,
        refs_error: after.refs_error,
    }))
}
