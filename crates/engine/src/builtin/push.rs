//! `remote.push` (S26): publishes a local branch to the remote (`impl-docs/git/sync.md`).

use super::remote::{changes, unexpected};
use super::{Out, to_json};
use crate::commands::parse_params;
use crate::git::short;
use crate::{
    EngineError, Message, MessageArg, PushRequest, Session, branch_ref, check_branch_name,
    check_remote_name, default_remote, tracking_ref, unknown_remote,
};
use project_domain::{Head, Oid};
use serde::Deserialize;
use serde_json::{Value, json};

const ID: &str = "remote.push";

/// Commits listed at most in the confirmation of a force.
const LOST_LIMIT: usize = 10;

/// Parameters of `remote.push`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushParams {
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
/// Result: `{"remote", "remote_source": "param" | "push_remote" | "push_default" |
/// "upstream" | "origin" | "only", "branch", "remote_branch", "kind": "new" | "fast_forward" | "forced"
/// | "up_to_date", "summary", "before": oid | null, "after": oid, "set_upstream": bool,
/// "forced": bool, "lost": [Commit], "backup": string | null, "updated": [{"name", "before",
/// "after"}] | null, "messages": [string], "warnings": [string], "status": Status | null,
/// "refs": [RefInfo] | null}`: `before` is the local remote-tracking branch before the push, `after` the
/// published commit, `lost` the commits removed from the remote by a force, `updated` the local refs
/// changed (the local remote-tracking branch).
pub(super) fn remote_push(s: &mut Session, p: Value) -> Out {
    let PushParams {
        remote,
        branch,
        force,
        confirm,
        expected,
    } = parse_params(ID, p)?;
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
        Some(r) => (r, "param"),
        None => {
            if let Some(r) = last(repo.config_values(&format!("branch.{branch}.pushRemote"))?) {
                (r, "push_remote")
            } else if let Some(r) = last(repo.config_values("remote.pushDefault")?) {
                (r, "push_default")
            } else {
                match &upstream {
                    Some(up) if up.remote == "." => {
                        return Err(EngineError::NotAllowed(msg!(
                            "engine-push-local-upstream",
                            branch = branch.escape_debug(),
                            merge = up.merge
                        )));
                    }
                    Some(up) => (up.remote.clone(), "upstream"),
                    None => {
                        let without = project_domain::Status {
                            upstream: None,
                            ..status.clone()
                        };
                        let r = default_remote(ID, &without, &remotes)?;
                        let source = if r == "origin" { "origin" } else { "only" };
                        (r, source)
                    }
                }
            }
        }
    };
    if remote_source == "push_remote" || remote_source == "push_default" {
        let key = if remote_source == "push_remote" {
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
                    id = short(&c.id),
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
    let mut out = json!({
        "remote": remote,
        "remote_source": remote_source,
        "branch": branch,
        "remote_branch": remote_branch,
        "kind": to_json(outcome.kind),
        "summary": outcome.summary,
        "before": remote_value.map(|o| o.to_string()),
        "after": local.to_string(),
        "set_upstream": set_upstream,
        "forced": force,
        "lost": to_json(&lost),
        "backup": outcome.backup,
        "messages": outcome.messages,
    });
    match repo.refs() {
        Ok(after) => {
            let updated = changes(&refs_before, &after);
            let warnings: Vec<Message> = updated.iter().filter_map(unexpected).collect();
            out["updated"] = to_json(updated);
            out["warnings"] = to_json(warnings);
            out["refs"] = to_json(after);
        }
        Err(e) => {
            out["updated"] = Value::Null;
            out["warnings"] = json!([]);
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
