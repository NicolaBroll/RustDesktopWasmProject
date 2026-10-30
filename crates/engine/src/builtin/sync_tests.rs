//! Tests of `remote.pull` and `remote.push` on the `MemoryBackend` (the backends are also tested in
//! `git/memory/sync_tests.rs` and, with real git, in `crates/git/tests/{pull,push}.rs`).

use crate::{
    CancelToken, EngineError, FetchFailure, GitRead, JobContext, MemoryBackend, MemoryOpener,
    PullSettings, PushRejection, Registry, Session,
};
use project_domain::{Change, ChangedFile, Commit, Oid, RepoPath, Signature};
use serde_json::json;
use std::sync::Arc;

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn target(r: &Registry, s: &mut Session, name: &str) -> Option<String> {
    r.run("repo.refs", s, json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == name)
        .map(|x| x["target"].as_str().unwrap().to_owned())
}

fn who() -> Signature {
    Signature {
        name: "Carla".into(),
        email: "carla@example.com".into(),
        time: 1_700_000_000 + 20 * 3_600,
        offset_min: 0,
    }
}

/// The sample repo without local changes, with `origin/main` having a commit (20) after
/// `11` that `main` (at `12`) does not have: diverged branches. Commit 20 changes `file`, if given.
fn diverged(file: Option<&str>) -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo.insert_remote_commit(
        "origin",
        Commit {
            id: oid(20),
            parents: vec![oid(11)],
            author: who(),
            committer: who(),
            summary: "From the remote".into(),
            body: String::new(),
        },
    );
    if let Some(file) = file {
        repo.set_blob(oid(5_000), "from the remote\n");
        repo.set_changes(
            oid(20),
            vec![ChangedFile {
                path: RepoPath::from(file),
                change: Change::Added,
                original: None,
                score: None,
                old_mode: 0,
                new_mode: 0o100644,
                old_oid: Oid::from_bytes([0; Oid::LEN]),
                new_oid: oid(5_000),
            }],
        );
    }
    repo.set_branch_on_remote("origin", "main", oid(20));
    repo
}

#[test]
fn pull_of_a_branch_that_is_not_checked_out() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = r
        .run("remote.pull", &mut s, json!({ "branch": "feature/graph" }))
        .unwrap();
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(out["before"], oid(9).to_string());
    assert_eq!(out["after"], oid(13).to_string());
    assert_eq!(out["backup"], "refs/project/backup/pull-1");
    assert_eq!(out["upstream"], "refs/remotes/origin/feature/graph");
    assert_eq!(
        out["fetch"]["updated"][0]["name"],
        "refs/remotes/origin/feature/graph"
    );
    assert_eq!(
        target(&r, &mut s, "refs/heads/feature/graph").unwrap(),
        oid(13).to_string()
    );
    // HEAD and the sample's local changes are not touched.
    assert_eq!(out["status"]["head"]["Branch"]["name"], "main");
    assert_eq!(out["status"]["entries"].as_array().unwrap().len(), 4);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Pull of `feature/graph` (fast-forward")
    );
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/feature/graph").unwrap(),
        oid(9).to_string()
    );
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/feature/graph").unwrap(),
        oid(13).to_string()
    );

    // `main` already contains `origin/main`: nothing to do.
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(
        (out["ahead"].as_u64(), out["behind"].as_u64()),
        (Some(1), Some(0))
    );
}

#[test]
fn pull_merge_rebase_and_undo() {
    // Local changes: a merge does not start.
    let mut dirty = diverged(None);
    dirty.set_entries(MemoryBackend::sample().status().unwrap().entries);
    let (r, mut s) = opened_with(dirty);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("autostash"), "{err}");

    let (r, mut s) = opened_with(diverged(Some("fresh.txt")));
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["strategy_source"], "default");
    let merge = out["after"].as_str().unwrap().to_owned();
    let log = r.run("repo.log", &mut s, json!({ "limit": 1 })).unwrap();
    let head = &log["commits"][0];
    assert_eq!(head["id"], merge);
    assert_eq!(
        head["parents"],
        json!([oid(12).to_string(), oid(20).to_string()])
    );
    assert_eq!(
        head["summary"],
        "Merge remote-tracking branch 'origin/main'"
    );
    // The incoming file is in HEAD's tree and the state is clean.
    let show = r
        .run("commit.show", &mut s, json!({ "id": merge }))
        .unwrap();
    assert_eq!(show["files"][0]["path"], "fresh.txt");
    assert_eq!(out["status"]["entries"], json!([]));

    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        undo["status"]["head"]["Branch"]["target"],
        oid(12).to_string()
    );
    assert_eq!(undo["backups"], json!(["refs/project/backup/undo-1"]));
    assert_eq!(undo["status"]["entries"], json!([]));
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(target(&r, &mut s, "refs/heads/main").unwrap(), merge);

    // Rebase from the configuration: commit 12 rewritten on top of 20.
    let mut repo = diverged(Some("fresh.txt"));
    repo.set_pull_settings(PullSettings {
        rebase: Some("true".into()),
        ff: None,
    });
    let (r, mut s) = opened_with(repo);
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "rebase");
    assert_eq!(out["strategy_source"], "config");
    let log = r.run("repo.log", &mut s, json!({ "limit": 2 })).unwrap();
    assert_eq!(log["commits"][0]["summary"], "Commit panel");
    assert_eq!(log["commits"][0]["parents"], json!([oid(20).to_string()]));
    assert_eq!(log["commits"][1]["id"], oid(20).to_string());
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );

    // Fast-forward only: refused, but the fetch is done.
    let (r, mut s) = opened_with(diverged(None));
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "ff_only" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/main").unwrap(),
        oid(20).to_string()
    );
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
}

#[test]
fn pull_merge_conflict_stays_in_progress() {
    // 12 creates `src/bin/app.rs` (rename), and the remote creates it with other content.
    let (r, mut s) = opened_with(diverged(Some("src/bin/app.rs")));
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["src/bin/app.rs"]));
    assert!(
        out["message"].as_str().unwrap().contains("CONFLICT"),
        "{out}"
    );
    assert_eq!(out["before"], oid(12).to_string());
    assert_eq!(out["after"], oid(12).to_string());
    assert_eq!(out["backup"], "refs/project/backup/pull-1");
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["status"]["operation"], "Merge");
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
    let op = r.run("repo.operation", &mut s, json!({})).unwrap();
    assert_eq!(op["operation"], "Merge");
    assert_eq!(op["incoming"][0]["id"], oid(20).to_string());
    assert_eq!(op["incoming"][0]["label"], "origin/main");
    assert_eq!(op["message"], "Merge remote-tracking branch 'origin/main'");
    // Another pull is refused while the merge is in progress.
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // Resolved and staged: `merge.continue` makes the merge commit, which goes into the journal.
    r.run(
        "index.stage",
        &mut s,
        json!({ "paths": ["src/bin/app.rs"] }),
    )
    .unwrap();
    let done = r.run("merge.continue", &mut s, json!({})).unwrap();
    assert_eq!(
        done["commit"]["parents"],
        json!([oid(12).to_string(), oid(20).to_string()])
    );
    assert_eq!(
        done["message"],
        "Merge remote-tracking branch 'origin/main'"
    );
    assert_eq!(done["can_undo"], true);
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
}

#[test]
fn pull_merge_conflict_then_abort() {
    let (r, mut s) = opened_with(diverged(Some("src/bin/app.rs")));
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["in_progress"], true);
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    let aborted = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap();
    assert_eq!(aborted["status"]["operation"], json!(null));
    assert_eq!(
        aborted["status"]["entries"],
        json!([]),
        "as it was before the pull"
    );
    assert_eq!(aborted["can_undo"], false);
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
}

/// `diverged` with the conflict on `src/bin/app.rs` and `pull.rebase=true`.
fn rebase_conflict() -> (Registry, Session) {
    let mut repo = diverged(Some("src/bin/app.rs"));
    repo.set_pull_settings(PullSettings {
        rebase: Some("true".into()),
        ff: None,
    });
    opened_with(repo)
}

#[test]
fn pull_rebase_conflict_stays_in_progress_then_continue() {
    let (r, mut s) = rebase_conflict();
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "rebase");
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["src/bin/app.rs"]));
    assert_eq!(out["after"], oid(12).to_string());
    assert_eq!(out["backup"], "refs/project/backup/pull-1");
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["status"]["operation"], "Rebase");
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
    let op = r.run("repo.operation", &mut s, json!({})).unwrap();
    assert_eq!(op["continue"], "rebase.continue");
    assert_eq!(op["abort"], "rebase.abort");
    assert_eq!(op["onto"], oid(20).to_string());
    assert_eq!(op["rebase_branch"], "refs/heads/main");
    // Undo is refused while the rebase is in progress; continue too with the conflict.
    assert!(r.run("edit.undo", &mut s, json!({})).is_err());
    let err = r.run("rebase.continue", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("src/bin/app.rs"), "{err}");

    r.run(
        "index.stage",
        &mut s,
        json!({ "paths": ["src/bin/app.rs"] }),
    )
    .unwrap();
    let done = r.run("rebase.continue", &mut s, json!({})).unwrap();
    assert_eq!(done["in_progress"], false);
    assert_eq!(done["command"], "remote.pull");
    assert_eq!(done["before"], oid(12).to_string());
    assert_eq!(done["status"]["operation"], json!(null));
    assert_eq!(done["warnings"], json!([]));
    let after = target(&r, &mut s, "refs/heads/main").unwrap();
    assert_eq!(done["after"], after);
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    let last = &history["done"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["command"], "remote.pull");
    assert!(
        last["description"]["text"]
            .as_str()
            .unwrap()
            .contains("rebase onto origin/main"),
        "{last}"
    );
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(target(&r, &mut s, "refs/heads/main").unwrap(), after);
}

#[test]
fn pull_rebase_conflict_then_abort() {
    let (r, mut s) = rebase_conflict();
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["in_progress"], true);
    let err = r.run("rebase.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    let aborted = r
        .run("rebase.abort", &mut s, json!({ "confirm": true }))
        .unwrap();
    assert_eq!(aborted["backup"], "refs/project/backup/rebase-abort-1");
    assert_eq!(aborted["status"]["operation"], json!(null));
    assert_eq!(aborted["status"]["head"]["Branch"]["name"], "main");
    assert_eq!(aborted["status"]["entries"], json!([]));
    assert_eq!(aborted["can_undo"], false);
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
}

#[test]
fn pull_refusals() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    r.run("branch.create", &mut s, json!({ "name": "only" }))
        .unwrap();
    let err = r
        .run("remote.pull", &mut s, json!({ "branch": "only" }))
        .unwrap_err();
    assert!(err.to_string().contains("--set-upstream-to"), "{err}");
    let err = r
        .run("remote.pull", &mut s, json!({ "remote": "fork" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = r
        .run("remote.pull", &mut s, json!({ "branch": "missing" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.detach_head(oid(11));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(project_domain::Operation::Rebase));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("rebase"), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_pull_settings(PullSettings {
        rebase: Some("interactive".into()),
        ff: None,
    });
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("interactive"), "{err}");
    // Nothing fetched.
    let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
    assert!(!refs.to_string().contains(&oid(13).to_string()));

    let mut repo = MemoryBackend::sample();
    repo.fail_fetch("origin", Some(FetchFailure::Auth("no".into())));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::Auth { .. }), "{err}");

    let (r, mut s) = opened_with(MemoryBackend::sample());
    let token = CancelToken::new();
    token.cancel();
    let err = r
        .run_with(
            "remote.pull",
            &mut s,
            json!({ "branch": "feature/graph" }),
            JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Cancelled {
                command: "remote.pull"
            }
        ),
        "{err}"
    );
}

#[test]
fn push_of_the_sample() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["kind"], "fast_forward");
    assert_eq!(out["before"], oid(11).to_string());
    assert_eq!(out["after"], oid(12).to_string());
    assert_eq!(out["set_upstream"], false);
    assert_eq!(
        out["updated"],
        json!([{
            "name": "refs/remotes/origin/main",
            "before": oid(11).to_string(),
            "after": oid(12).to_string(),
        }])
    );
    assert_eq!(out["messages"][0], "To https://example.com/project.git");
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["kind"], "up_to_date");

    // First push of a new branch: the upstream is set.
    r.run("branch.create", &mut s, json!({ "name": "fresh" }))
        .unwrap();
    let out = r
        .run("remote.push", &mut s, json!({ "branch": "fresh" }))
        .unwrap();
    assert_eq!(out["kind"], "new");
    assert_eq!(out["set_upstream"], true);
    let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
    let fresh = refs
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "refs/heads/fresh")
        .unwrap();
    assert_eq!(fresh["upstream"]["name"], "refs/remotes/origin/fresh");
    // And now the pull finds it too.
    let out = r
        .run("remote.pull", &mut s, json!({ "branch": "fresh" }))
        .unwrap();
    assert_eq!(out["strategy"], "up_to_date");
}

#[test]
fn push_rejections_and_force() {
    // The remote has 20, which is not here: refused, nothing changes.
    let (r, mut s) = opened_with(diverged(None));
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::NonFastForward,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/main").unwrap(),
        oid(11).to_string()
    );
    // The force with the old local remote-tracking branch (11): the lease fails.
    let err = r
        .run("remote.push", &mut s, json!({ "force": true }))
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::StaleLease,
                ..
            }
        ),
        "{err}"
    );
    // After the fetch: the force would remove 20, the confirmation is needed.
    r.run("remote.fetch", &mut s, json!({})).unwrap();
    let err = r
        .run("remote.push", &mut s, json!({ "force": true }))
        .unwrap_err();
    let EngineError::ConfirmRequired {
        reason, expected, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert!(reason.contains("“From the remote”"), "{reason}");
    assert_eq!(expected.as_deref(), Some(oid(20).to_string().as_str()));
    assert!(
        reason.contains(&format!("\"expected\": \"{}\"", oid(20))),
        "{reason}"
    );
    // An `expected` that is not the local remote-tracking branch: asked again, nothing published.
    let err = r
        .run(
            "remote.push",
            &mut s,
            json!({ "force": true, "confirm": true, "expected": oid(11).to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    let out = r
        .run(
            "remote.push",
            &mut s,
            json!({ "force": true, "confirm": true, "expected": oid(20).to_string() }),
        )
        .unwrap();
    assert_eq!(out["kind"], "forced");
    assert_eq!(out["backup"], "refs/project/backup/push-1");
    assert_eq!(out["lost"][0]["id"], oid(20).to_string());
    assert_eq!(out["before"], oid(20).to_string());

    // `pre-push` hook, credentials, cancellation.
    let mut repo = MemoryBackend::sample();
    repo.fail_pre_push(Some("pre-push: no"));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(
            &err,
            EngineError::PushRejected { reason: PushRejection::Hook, message, .. } if message == "pre-push: no"
        ),
        "{err}"
    );
    let mut repo = MemoryBackend::sample();
    repo.fail_fetch("origin", Some(FetchFailure::Unreachable("down".into())));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(err, EngineError::RemoteUnreachable { .. }),
        "{err}"
    );
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let token = CancelToken::new();
    token.cancel();
    let err = r
        .run_with(
            "remote.push",
            &mut s,
            json!({}),
            JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled { .. }), "{err}");
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/main").unwrap(),
        oid(11).to_string()
    );
}

#[test]
fn push_refusals() {
    let mut repo = MemoryBackend::sample();
    repo.detach_head(oid(12));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_upstream("feature/graph", "origin/main");
    let (r, mut s) = opened_with(repo);
    let err = r
        .run("remote.push", &mut s, json!({ "branch": "feature/graph" }))
        .unwrap_err();
    assert!(err.to_string().contains("simple"), "{err}");
    for params in [
        json!({ "remote": "missing" }),
        json!({ "remote": "-x" }),
        json!({ "branch": "missing" }),
    ] {
        let err = r.run("remote.push", &mut s, params.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
    }
    for params in [
        json!({ "confirm": true }),
        json!({ "force": true, "confirm": true }),
        json!({ "force": true, "expected": "1234" }),
        json!({ "tags": true }),
        json!({ "branch": "" }),
        json!({ "remote": "a b" }),
    ] {
        let err = r.run("remote.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
}

#[test]
fn need_an_open_repo() {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(MemoryOpener::sample()));
    for id in ["remote.pull", "remote.push"] {
        let err = r.run(id, &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
    }
}

#[test]
fn push_remote_from_the_configuration() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "https://example.com/fork.git");
    repo.set_config("remote.pushDefault", &["fork"]);
    let (r, mut s) = opened_with(repo.clone());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["remote"], "fork");
    assert_eq!(out["remote_source"], "push_default");
    assert_eq!(out["kind"], "new");
    assert_eq!(out["set_upstream"], false);
    assert_eq!(out["updated"][0]["name"], "refs/remotes/fork/main");
    repo.set_config("branch.main.pushRemote", &["origin"]);
    let (r, mut s) = opened_with(repo.clone());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["remote_source"], "push_remote");
    assert_eq!(out["remote"], "origin");
    repo.set_config("remote.origin.mirror", &["true"]);
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("mirror"), "{err}");
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["remote_source"], "upstream");
}

#[test]
fn pull_never_overwrites_ignored_files() {
    // The remote adds `secret.env`, which is ignored here: the pull is refused.
    let mut repo = diverged(Some("secret.env"));
    repo.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("secret.env"),
        index: Change::Ignored,
        worktree: Change::Ignored,
        kind: project_domain::EntryKind::Ignored,
    }]);
    for strategy in ["merge", "rebase"] {
        let (r, mut s) = opened_with(repo.clone());
        let err = r
            .run("remote.pull", &mut s, json!({ "strategy": strategy }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(err.to_string().contains("secret.env"), "{err}");
        assert_eq!(
            target(&r, &mut s, "refs/heads/main").unwrap(),
            oid(12).to_string()
        );
    }
}

#[test]
fn rebase_never_passes_over_ignored_files() {
    // Commit 12 (to be reapplied) has `src/bin/app.rs`; here an ignored file is in its place,
    // as if a later commit had removed it: the rebase is refused, the merge is not.
    let mut repo = diverged(None);
    repo.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("src/bin/app.rs"),
        index: Change::Ignored,
        worktree: Change::Ignored,
        kind: project_domain::EntryKind::Ignored,
    }]);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("src/bin/app.rs"), "{err}");
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
}

#[test]
fn delete_branch_on_the_remote() {
    // In the sample `feature/graph` moved to 13 on the remote: here it is aligned with the fetch.
    let mut repo = MemoryBackend::sample();
    repo.set_branch_on_remote("origin", "feature/graph", oid(9));
    let (r, mut s) = opened_with(repo);
    let params = json!({ "remote": "origin", "branch": "feature/graph" });
    let err = r.run("remote.delete_branch", &mut s, params).unwrap_err();
    let EngineError::ConfirmRequired {
        reason, expected, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(expected.as_deref(), Some(oid(9).to_string().as_str()));
    assert!(reason.contains("`feature/graph`"), "{reason}");
    assert!(reason.contains("also in other branches"), "{reason}");
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/feature/graph").unwrap(),
        oid(9).to_string()
    );
    // An `expected` that is not the remote-tracking branch: asked again.
    let wrong = json!({ "remote": "origin", "branch": "feature/graph", "confirm": true, "expected": oid(11).to_string() });
    let err = r.run("remote.delete_branch", &mut s, wrong).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");

    let confirmed = json!({ "remote": "origin", "branch": "feature/graph", "confirm": true, "expected": oid(9).to_string() });
    let out = r
        .run("remote.delete_branch", &mut s, confirmed.clone())
        .unwrap();
    assert_eq!(out["summary"], "[deleted]");
    assert_eq!(out["backup"], "refs/project/backup/push-delete-1");
    assert_eq!(out["upstream_of"], json!(["feature/graph"]));
    assert_eq!(out["only_here"], 0);
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/feature/graph"),
        None
    );
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
    // Now it is not known here any more: refused before the remote.
    let err = r
        .run("remote.delete_branch", &mut s, confirmed)
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // Moved on the remote since the last fetch (the sample: 13): the lease rejects it.
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let err = r
        .run(
            "remote.delete_branch",
            &mut s,
            json!({ "remote": "origin", "branch": "feature/graph", "confirm": true, "expected": oid(9).to_string() }),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::StaleLease,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/feature/graph").unwrap(),
        oid(9).to_string()
    );
}

/// The typed results read back the JSON of the commands and give the same JSON again: the UI
/// deserializes into them.
#[test]
fn results_round_trip() {
    use crate::{
        PullApplied, RemoteDeleteBranchResult, RemoteDeleteTagResult, RemoteFetchResult,
        RemotePullResult, RemotePushResult, RemotePushTagResult,
    };
    use serde_json::Value;
    fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: Value) -> T {
        let typed: T = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        typed
    }
    fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
        let out = r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"));
        assert!(out.get("refs_error").is_none() && out.get("status_error").is_none());
        out
    }
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = run(&r, &mut s, "remote.fetch", json!({}));
    assert_eq!(out["branch"], Value::Null);
    same::<RemoteFetchResult>(out);
    same::<RemoteFetchResult>(run(
        &r,
        &mut s,
        "remote.fetch",
        json!({ "remote": "origin", "branch": "main" }),
    ));
    let pull: RemotePullResult = same(run(
        &r,
        &mut s,
        "remote.pull",
        json!({ "branch": "feature/graph" }),
    ));
    assert_eq!(pull.strategy, PullApplied::FastForward);
    let pull: RemotePullResult = same(run(&r, &mut s, "remote.pull", json!({})));
    assert_eq!(pull.strategy, PullApplied::UpToDate);
    assert_eq!(pull.backup, None);
    same::<RemotePushResult>(run(&r, &mut s, "remote.push", json!({})));
    same::<RemoteDeleteBranchResult>(run(
        &r,
        &mut s,
        "remote.delete_branch",
        json!({
            "remote": "origin",
            "branch": "feature/graph",
            "confirm": true,
            "expected": oid(13).to_string(),
        }),
    ));
    same::<RemotePushTagResult>(run(
        &r,
        &mut s,
        "remote.push_tag",
        json!({ "remote": "origin", "name": "v0.1.0" }),
    ));
    same::<RemoteDeleteTagResult>(run(
        &r,
        &mut s,
        "remote.delete_tag",
        json!({
            "remote": "origin",
            "name": "v0.1.0",
            "confirm": true,
            "expected": oid(1_000).to_string(),
        }),
    ));
    // A pull that stops on a rebase conflict.
    let (r, mut s) = rebase_conflict();
    let pull: RemotePullResult = same(run(&r, &mut s, "remote.pull", json!({})));
    assert_eq!(pull.strategy, PullApplied::Rebase);
    assert!(pull.in_progress && !pull.conflicts.is_empty());
}

/// Refs that cannot be re-read give `updated: null`, `refs: null` and `refs_error`, keeping the
/// warnings given; a status that cannot be re-read gives `null` and `status_error`.
#[test]
fn reread_error_fields() {
    use super::super::remote::RefsAfter;
    use super::super::reread;
    use crate::{
        Message, PushKind, PushRemoteSource, RemoteDeleteTagResult, RemotePushResult,
        RemotePushTagResult, TagPushKind,
    };
    use project_domain::{RefInfo, Status};
    use serde_json::Value;
    let error = EngineError::NoRepo.to_string();
    let after = RefsAfter::new(&[], Err(EngineError::NoRepo), vec![Message::plain("w")]);
    assert_eq!(after.updated, None);
    assert_eq!(after.refs, None);
    assert_eq!(after.warnings, vec![Message::plain("w")]);
    let (status, status_error) = reread::<Status>(Err(EngineError::NoRepo));
    let result = RemotePushResult {
        remote: "origin".into(),
        remote_source: PushRemoteSource::PushDefault,
        branch: "main".into(),
        remote_branch: "main".into(),
        kind: PushKind::Forced,
        summary: String::new(),
        before: None,
        after: oid(1),
        set_upstream: false,
        forced: true,
        lost: vec![],
        backup: None,
        updated: after.updated,
        messages: vec![],
        warnings: after.warnings,
        status,
        status_error,
        refs: after.refs,
        refs_error: after.refs_error,
    };
    let out = serde_json::to_value(&result).unwrap();
    for key in ["updated", "refs", "status", "before", "backup"] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(out["refs_error"], error.as_str());
    assert_eq!(out["status_error"], error.as_str());
    assert_eq!(out["remote_source"], "push_default");
    assert_eq!(out["kind"], "forced");
    let back: RemotePushResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);

    let (refs, refs_error) = reread::<Vec<RefInfo>>(Err(EngineError::NoRepo));
    let result = RemotePushTagResult {
        remote: "origin".into(),
        tag: "refs/tags/v1".into(),
        name: "v1".into(),
        target: oid(1),
        annotated: false,
        value: oid(1),
        kind: TagPushKind::UpToDate,
        summary: String::new(),
        messages: vec![],
        refs: refs.clone(),
        refs_error: refs_error.clone(),
    };
    let out = serde_json::to_value(&result).unwrap();
    assert_eq!(out["kind"], "up_to_date");
    assert_eq!(out["refs"], Value::Null);
    assert_eq!(out["refs_error"], error.as_str());
    let back: RemotePushTagResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);
    let result = RemoteDeleteTagResult {
        remote: "origin".into(),
        tag: "refs/tags/v1".into(),
        name: "v1".into(),
        deleted: oid(1),
        backup: "refs/project/backup/push-delete-1".into(),
        summary: "[deleted]".into(),
        messages: vec![],
        incomplete: true,
        warnings: vec![Message::plain("w")],
        refs,
        refs_error,
    };
    let out = serde_json::to_value(&result).unwrap();
    assert_eq!(out["refs_error"], error.as_str());
    let back: RemoteDeleteTagResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);
}
