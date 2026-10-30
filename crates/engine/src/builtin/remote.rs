//! Commands of the remotes (S25): `remote.fetch`. Slow command (§6): progress and
//! cancellation arrive from [`Session::job`].

use super::{Out, to_json};
use crate::{
    EngineError, FetchRequest, Message, Session, check_branch_name, check_remote_name,
    default_remote, unknown_remote,
};
use project_domain::{Oid, RefInfo};
use project_domain::{REMOTES_PREFIX, TAGS_PREFIX};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Result of `remote.fetch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteFetchResult {
    /// The remote fetched from (the one given, or the default one).
    pub remote: String,
    /// The only branch of the remote fetched; `null` for the whole remote.
    pub branch: Option<String>,
    /// `--prune` was requested.
    pub prune: bool,
    /// The refs changed by the fetch, in name order; `null` if the refs could not be re-read
    /// (see `refs_error`).
    pub updated: Option<Vec<RefUpdate>>,
    /// The git lines that are not progress (`From <url>`, ` * [new branch] ...`), in order.
    pub messages: Vec<String>,
    /// Refs outside `refs/remotes/` and `refs/tags/` moved by the fetch (only with a refspec
    /// configured on purpose); empty if the refs could not be re-read.
    pub warnings: Vec<Message>,
    /// The backup refs (`refs/project/backup/prune-...`) of the pruned remote-tracking branches.
    pub backups: Vec<String>,
    /// The refs re-read after the fetch; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
}

/// The refs re-read after a fetch or push, compared with those from before: the fields `updated`,
/// `warnings`, `refs` and `refs_error` of the results.
pub(super) struct RefsAfter {
    /// The changed refs; `None` if the refs could not be re-read.
    pub updated: Option<Vec<RefUpdate>>,
    /// The warnings given, then one for each unexpected changed ref ([`unexpected`]).
    pub warnings: Vec<Message>,
    /// The refs re-read.
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read.
    pub refs_error: Option<String>,
}

impl RefsAfter {
    /// Compares `after` (the re-read) with `before`; `warnings` come first in the warnings.
    pub(super) fn new(
        before: &[RefInfo],
        after: Result<Vec<RefInfo>, EngineError>,
        mut warnings: Vec<Message>,
    ) -> Self {
        match after {
            Ok(after) => {
                let updated = changes(before, &after);
                warnings.extend(updated.iter().filter_map(unexpected));
                Self {
                    updated: Some(updated),
                    warnings,
                    refs: Some(after),
                    refs_error: None,
                }
            }
            Err(e) => Self {
                updated: None,
                warnings,
                refs: None,
                refs_error: Some(e.to_string()),
            },
        }
    }
}

/// Parameters of `remote.fetch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteFetchParams {
    /// The configured remote; without: upstream, `origin` or the only one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// Only this branch of the remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Also remove the remote-tracking branches gone from the remote; not with `branch`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prune: bool,
}

/// A ref changed by a fetch or a push.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefUpdate {
    /// The full name of the ref (`refs/remotes/origin/main`).
    pub name: String,
    /// The commit before; `null` if the ref did not exist.
    pub before: Option<Oid>,
    /// The commit after; `null` if the ref no longer exists (a remote-tracking branch removed by
    /// `prune` or a configured `fetch.prune`).
    pub after: Option<Oid>,
}

/// The value of each ref for the before/after comparison: for an annotated tag the tag
/// object counts too (a tag moved to another object with the same commit has changed).
fn values(refs: &[RefInfo]) -> BTreeMap<&str, (Oid, Option<Oid>)> {
    refs.iter()
        .map(|r| (r.name.as_str(), (r.target, r.tag_object)))
        .collect()
}

/// The refs that differ between `before` and `after`, in name order. The reported value is the
/// commit (for
/// an annotated tag, the one pointed to).
pub(super) fn changes(before: &[RefInfo], after: &[RefInfo]) -> Vec<RefUpdate> {
    let (old, new) = (values(before), values(after));
    let mut names: Vec<&str> = old.keys().chain(new.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .filter(|name| old.get(name) != new.get(name))
        .map(|name| RefUpdate {
            name: name.to_owned(),
            before: old.get(name).map(|v| v.0),
            after: new.get(name).map(|v| v.0),
        })
        .collect()
}

/// Warning for a ref outside `refs/remotes/` and `refs/tags/` moved by the fetch: it only happens
/// with a refspec configured on purpose (e.g. a mirror to `refs/heads/`). The previous value
/// stays in that ref's reflog.
pub(super) fn unexpected(update: &RefUpdate) -> Option<Message> {
    let expected = update.name.starts_with(REMOTES_PREFIX) || update.name.starts_with(TAGS_PREFIX);
    if expected {
        return None;
    }
    let show = |v: Option<Oid>| {
        v.map_or_else(
            || msg!("engine-ref-missing"),
            |id| Message::plain(id.to_string()),
        )
    };
    Some(msg!(
        "engine-remote-fetch-unexpected-ref",
        name = update.name,
        before = show(update.before),
        after = show(update.after)
    ))
}

/// Fetches from the remote `remote` (see [`GitRemotes::fetch`](crate::GitRemotes::fetch)): new
/// commits, remote-tracking branches and tags as the configuration says; with `branch` only that
/// branch of the
/// remote. `remote` is the name of a **configured** remote (never a URL or a path:
/// [`EngineError::NotAllowed`] if there is none); if missing, the one of the active branch's
/// upstream,
/// otherwise `origin`, otherwise the only remote. It does not touch local branches, index,
/// worktree:
/// it does not go into the journal of operations (the remote-tracking branches mirror the remote; a
/// fetch after
/// an undo would bring them back to where they were). With `prune` (not together with `branch`) it
/// also removes
/// the remote-tracking branches whose branch the remote no longer has (`--prune`); without it the
/// configuration's `fetch.prune` / `remote.<name>.prune` applies. A removed remote-tracking branch
/// whose
/// commits no other ref reaches is first saved in `refs/project/backup/prune-...` (in `backups`).
///
/// Slow command: progress goes to the [`JobContext`](crate::JobContext) of
/// [`Registry::run_with`](crate::Registry::run_with); cancelled, it gives
/// [`EngineError::Cancelled`] (the refs already updated by git stay).
///
/// Result: [`RemoteFetchResult`].
pub(super) fn remote_fetch(
    s: &mut Session,
    RemoteFetchParams {
        remote,
        branch,
        prune,
    }: RemoteFetchParams,
) -> Out {
    const ID: &str = "remote.fetch";
    if let Some(remote) = &remote {
        check_remote_name(ID, remote)?;
    }
    if let Some(branch) = &branch {
        check_branch_name(ID, branch)?;
        if prune {
            return Err(EngineError::InvalidParams {
                id: ID,
                reason: msg!("engine-remote-fetch-prune-branch"),
            });
        }
    }
    let job = s.job().clone();
    let repo = s.repo_mut()?;
    let remotes = repo.remotes()?;
    let remote = match remote {
        Some(remote) if remotes.contains(&remote) => remote,
        Some(remote) => return Err(unknown_remote(&remote, &remotes)),
        None => default_remote(ID, &repo.status()?, &remotes)?,
    };
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let before = repo.refs()?;
    let request = FetchRequest {
        remote: remote.clone(),
        branch: branch.clone(),
        prune,
    };
    let outcome = repo.fetch(&request, &job)?;
    let after = RefsAfter::new(&before, repo.refs(), Vec::new());
    Ok(to_json(RemoteFetchResult {
        remote,
        branch,
        prune,
        updated: after.updated,
        messages: outcome.messages,
        warnings: after.warnings,
        backups: outcome.backups,
        refs: after.refs,
        refs_error: after.refs_error,
    }))
}

#[cfg(test)]
mod tests {
    use crate::{
        CancelToken, EngineError, FetchFailure, JobContext, MemoryBackend, MemoryOpener, Progress,
        Registry, Session,
    };
    use project_domain::Oid;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::default();
        opener.insert("r", repo);
        let mut s = Session::new(Arc::new(opener));
        let r = Registry::builtin();
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        (r, s)
    }

    fn sample() -> (Registry, Session) {
        opened_with(MemoryBackend::sample())
    }

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    fn target(out: &Value, name: &str) -> Option<String> {
        out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .map(|r| r["target"].as_str().unwrap().to_owned())
    }

    #[test]
    fn fetch_brings_new_commits_and_reports_the_updated_refs() {
        let (r, mut s) = sample();
        let out = r.run("remote.fetch", &mut s, json!({})).unwrap();
        assert_eq!(out["remote"], "origin");
        assert_eq!(out["branch"], Value::Null);
        assert_eq!(
            out["updated"],
            json!([{
                "name": "refs/remotes/origin/feature/graph",
                "before": oid(9).to_string(),
                "after": oid(13).to_string(),
            }])
        );
        assert_eq!(out["warnings"], json!([]));
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0], "From https://example.com/project.git");
        assert!(
            messages[1]
                .as_str()
                .unwrap()
                .contains("feature/graph -> origin/feature/graph")
        );
        assert_eq!(
            target(&out, "refs/remotes/origin/feature/graph").unwrap(),
            oid(13).to_string()
        );
        // The new commit is in the repo, and `feature/graph` is behind by one.
        let log = r
            .run(
                "repo.log",
                &mut s,
                json!({ "rev": "origin/feature/graph", "limit": 1 }),
            )
            .unwrap();
        assert_eq!(log["commits"][0]["summary"], "Lanes: one color per branch");
        // Only on `origin/feature/graph`: the graph shows it with `all`, not from HEAD.
        let mut summary = |params: Value| {
            r.run("repo.log", &mut s, params).unwrap()["commits"][0]["summary"].clone()
        };
        assert_eq!(
            summary(json!({ "all": true, "limit": 1 })),
            "Lanes: one color per branch"
        );
        assert_eq!(summary(json!({ "limit": 1 })), "Commit panel");
        let local = out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "refs/heads/feature/graph")
            .unwrap();
        assert_eq!(local["upstream"]["behind"], 1);
        // It does not go into the journal: nothing to undo.
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_undo"], false);

        // The second fetch brings nothing.
        let again = r
            .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
            .unwrap();
        assert_eq!(again["updated"], json!([]));
        assert_eq!(again["messages"], json!([]));
    }

    #[test]
    fn fetch_of_one_branch() {
        let (r, mut s) = sample();
        let out = r
            .run(
                "remote.fetch",
                &mut s,
                json!({ "remote": "origin", "branch": "main" }),
            )
            .unwrap();
        assert_eq!(out["branch"], "main");
        assert_eq!(out["updated"], json!([]));
        let out = r
            .run("remote.fetch", &mut s, json!({ "branch": "feature/graph" }))
            .unwrap();
        assert_eq!(
            out["updated"][0]["name"],
            "refs/remotes/origin/feature/graph"
        );

        let err = r
            .run("remote.fetch", &mut s, json!({ "branch": "missing" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(err.to_string().contains("missing"), "{err}");
    }

    #[test]
    fn a_new_remote_branch_and_a_new_remote() {
        let mut repo = MemoryBackend::sample();
        repo.add_remote("fork", "/srv/fork.git");
        repo.set_branch_on_remote("fork", "-x", oid(12));
        let (r, mut s) = opened_with(repo);
        let out = r
            .run("remote.fetch", &mut s, json!({ "remote": "fork" }))
            .unwrap();
        assert_eq!(
            out["updated"],
            json!([{ "name": "refs/remotes/fork/-x", "before": null, "after": oid(12).to_string() }])
        );
        assert!(
            out["messages"][1]
                .as_str()
                .unwrap()
                .contains("[new branch]")
        );
    }

    #[test]
    fn unknown_and_invalid_remotes_never_reach_the_backend() {
        let (r, mut s) = sample();
        // A name that is not a configured remote (git would use it as a URL or path).
        for remote in ["other", "tmp/repo", "-x", "--upload-pack=x"] {
            let err = r
                .run("remote.fetch", &mut s, json!({ "remote": remote }))
                .unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{remote}: {err}");
            assert!(err.to_string().contains("`origin`"), "{err}");
        }
        for remote in ["", "a b", "a..b", "https://x/y:z", "/tmp/repo", "../x"] {
            let err = r
                .run("remote.fetch", &mut s, json!({ "remote": remote }))
                .unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{remote}: {err}"
            );
        }
        for branch in ["", "a..b", "refs/heads/main"] {
            let err = r
                .run("remote.fetch", &mut s, json!({ "branch": branch }))
                .unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{branch}: {err}"
            );
        }
        let err = r
            .run(
                "remote.fetch",
                &mut s,
                json!({ "prune": true, "branch": "main" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
        let err = r
            .run("remote.fetch", &mut s, json!({ "force": true }))
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }

    #[test]
    fn default_remote_without_origin() {
        let mut repo = MemoryBackend::new();
        let (r, mut s) = opened_with(repo.clone());
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(err.to_string().contains("has no remotes"), "{err}");

        repo.add_remote("a", "/a");
        repo.add_remote("b", "/b");
        let (r, mut s) = opened_with(repo.clone());
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        // With the active branch's upstream on `b`, `b` is used.
        repo.set_upstream("main", "b/main");
        let (r, mut s) = opened_with(repo);
        let out = r.run("remote.fetch", &mut s, json!({})).unwrap();
        assert_eq!(out["remote"], "b");
    }

    #[test]
    fn credential_and_network_errors_are_distinct() {
        let mut repo = MemoryBackend::sample();
        repo.fail_fetch("origin", Some(FetchFailure::Auth("HTTP 401".into())));
        let (r, mut s) = opened_with(repo.clone());
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(
            matches!(&err, EngineError::Auth { remote, message } if remote == "origin" && message == "HTTP 401"),
            "{err}"
        );

        repo.fail_fetch(
            "origin",
            Some(FetchFailure::Unreachable("Could not resolve host".into())),
        );
        let (r, mut s) = opened_with(repo);
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(
            matches!(err, EngineError::RemoteUnreachable { .. }),
            "{err}"
        );
        // Nothing changed.
        let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
        let graph_tip = refs
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "refs/remotes/origin/feature/graph")
            .unwrap();
        assert_eq!(graph_tip["target"], oid(9).to_string());
    }

    #[test]
    fn progress_goes_to_the_job_and_cancel_stops_the_fetch() {
        let (r, mut s) = sample();
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let job = JobContext::new().with_progress(move |p| sink.lock().unwrap().push(p.clone()));
        r.run_with("remote.fetch", &mut s, json!({}), job).unwrap();
        let seen = seen.lock().unwrap();
        assert!(!seen.is_empty());
        let last = seen.last().unwrap();
        assert_eq!(last.phase, "Receiving objects");
        assert_eq!(last.percent, Some(100));
        assert!(last.done);
        // After the command the session goes back to an empty context.
        assert!(!s.job().is_cancelled());

        let (r, mut s) = sample();
        let token = CancelToken::new();
        token.cancel();
        let err = r
            .run_with(
                "remote.fetch",
                &mut s,
                json!({}),
                JobContext::new().with_cancel(token),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::Cancelled { .. }), "{err}");
        let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
        assert!(
            refs.as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "refs/remotes/origin/feature/graph"
                    && r["target"] == oid(9).to_string())
        );
    }

    #[test]
    fn needs_an_open_repo() {
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NoRepo), "{err}");
    }
}
