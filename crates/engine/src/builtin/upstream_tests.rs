//! Tests of the upstream of local branches (S51) on the `MemoryBackend`: `branch.create` with
//! `track`, `branch.set_upstream` and their undo/redo. Sample repo: `main` follows `origin/main`,
//! `feature/graph` follows `origin/feature/graph`.

use crate::{ConfigEntry, EngineError, GitRead, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::Oid;
use serde_json::{Value, json};
use std::sync::Arc;

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn opened() -> (Registry, Session) {
    opened_with(MemoryBackend::sample())
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn branch(s: &Session, name: &str) -> Option<Oid> {
    s.repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == format!("refs/heads/{name}"))
        .map(|r| r.target)
}

/// The upstream of `name` as `refs` reports it (the remote-tracking branch, full name).
fn upstream(s: &Session, name: &str) -> Option<String> {
    s.repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == format!("refs/heads/{name}"))
        .and_then(|r| r.upstream.map(|u| u.name))
}

fn entries(name: &str, remote: &str, merge: &str) -> Vec<ConfigEntry> {
    vec![
        ConfigEntry {
            key: format!("branch.{name}.remote"),
            value: Some(remote.to_owned()),
        },
        ConfigEntry {
            key: format!("branch.{name}.merge"),
            value: Some(format!("refs/heads/{merge}")),
        },
    ]
}

fn invalid(err: EngineError) -> String {
    match err {
        EngineError::InvalidParams { reason, .. } => reason.to_string(),
        other => panic!("expected InvalidParams, got {other:?}"),
    }
}

/// A sample repo with a second remote named `team/a`, which has `feature`.
fn with_slash_remote() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("team/a", "https://example.com/team.git");
    let tip = repo.head().unwrap().target().unwrap();
    repo.set_remote_branch("team/a/feature", tip);
    repo
}

#[test]
fn create_tracking_a_remote_branch_then_undo_and_redo() {
    let (r, mut s) = opened();
    let remote_tip = s
        .repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == "refs/remotes/origin/feature/graph")
        .unwrap()
        .target;
    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "graph", "start": "origin/feature/graph", "track": true }),
    );
    assert_eq!(out["branch"], "refs/heads/graph");
    assert_eq!(out["upstream"], "origin/feature/graph");
    assert_eq!(out["target"], remote_tip.to_string());
    assert_eq!(
        out["next_undo"]["text"].as_str().unwrap(),
        format!(
            "Create branch `graph` at {} tracking `origin/feature/graph`",
            remote_tip.short()
        )
    );
    assert_eq!(
        upstream(&s, "graph").as_deref(),
        Some("refs/remotes/origin/feature/graph")
    );
    assert_eq!(
        s.repo().unwrap().local_upstream("graph").unwrap(),
        entries("graph", "origin", "feature/graph")
    );

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["warnings"], json!([]), "{undo}");
    assert_eq!(branch(&s, "graph"), None);
    assert!(
        s.repo()
            .unwrap()
            .local_upstream("graph")
            .unwrap()
            .is_empty()
    );

    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(branch(&s, "graph"), Some(remote_tip));
    assert_eq!(
        upstream(&s, "graph").as_deref(),
        Some("refs/remotes/origin/feature/graph")
    );
}

#[test]
fn create_without_track_sets_no_upstream() {
    let (r, mut s) = opened();
    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "plain", "start": "origin/main" }),
    );
    assert_eq!(out["upstream"], Value::Null);
    assert_eq!(upstream(&s, "plain"), None);
}

#[test]
fn track_needs_a_remote_tracking_start() {
    let (r, mut s) = opened();
    for (params, text) in [
        (json!({ "name": "x", "track": true }), "needs `start`"),
        (
            json!({ "name": "x", "start": "main", "track": true }),
            "not a remote branch",
        ),
        (
            json!({ "name": "x", "start": "HEAD", "track": true }),
            "not a remote branch",
        ),
        (
            json!({ "name": "x", "start": "origin/nope", "track": true }),
            "not a remote branch",
        ),
        (
            json!({ "name": "-x", "start": "origin/main", "track": true }),
            "",
        ),
    ] {
        let err = r.run("branch.create", &mut s, params.clone()).unwrap_err();
        let reason = invalid(err);
        assert!(reason.contains(text), "{params}: {reason}");
    }
    assert_eq!(branch(&s, "x"), None);
}

#[test]
fn track_refuses_a_start_shadowed_by_a_local_branch() {
    let mut repo = MemoryBackend::sample();
    let tip = repo.head().unwrap().target().unwrap();
    repo.set_branch("origin/main", tip);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "branch.create",
            &mut s,
            json!({ "name": "x", "start": "origin/main", "track": true }),
        )
        .unwrap_err();
    assert!(invalid(err).contains("refs/heads/origin/main"));
    // In full it is not ambiguous.
    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "x", "start": "refs/remotes/origin/main", "track": true }),
    );
    assert_eq!(out["upstream"], "origin/main");
}

#[test]
fn track_from_a_remote_with_a_slash() {
    let (r, mut s) = opened_with(with_slash_remote());
    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "t", "start": "team/a/feature", "track": true }),
    );
    assert_eq!(out["upstream"], "team/a/feature");
    assert_eq!(
        s.repo().unwrap().local_upstream("t").unwrap(),
        entries("t", "team/a", "feature")
    );
}

#[test]
fn a_ref_of_two_remotes_is_ambiguous() {
    let mut repo = with_slash_remote();
    repo.add_remote("team", "https://example.com/other.git");
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "branch.create",
            &mut s,
            json!({ "name": "f", "start": "team/a/feature", "track": true }),
        )
        .unwrap_err();
    let reason = invalid(err);
    assert!(
        reason.contains("`team`") && reason.contains("`team/a`"),
        "{reason}"
    );
    assert_eq!(branch(&s, "f"), None);
}

#[test]
fn a_failed_config_write_removes_the_new_branch() {
    let mut repo = MemoryBackend::sample();
    repo.fail_config_writes(Some("could not lock config file .git/config"));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "branch.create",
            &mut s,
            json!({ "name": "graph", "start": "origin/feature/graph", "track": true }),
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("deleted again"), "{text}");
    assert!(text.contains("could not lock"), "{text}");
    assert_eq!(branch(&s, "graph"), None);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"], json!([]), "nothing in the journal");
}

#[test]
fn set_and_unset_upstream_then_undo_and_redo() {
    let (r, mut s) = opened();
    run(&r, &mut s, "branch.create", json!({ "name": "topic" }));

    let out = run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "topic", "upstream": "origin/main" }),
    );
    assert_eq!(out["branch"], "refs/heads/topic");
    assert_eq!(out["upstream"], "origin/main");
    assert_eq!(out["previous"], Value::Null);
    assert_eq!(out["changed"], true);
    assert_eq!(
        out["next_undo"]["text"],
        "Set the upstream of `topic` to `origin/main`"
    );
    assert!(
        out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "refs/heads/topic"
                && r["upstream"]["name"] == "refs/remotes/origin/main")
    );

    // Same value: nothing written, nothing in the journal.
    let again = run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "topic", "upstream": "refs/remotes/origin/main" }),
    );
    assert_eq!(again["changed"], false);
    assert_eq!(again["previous"], "origin/main");

    let out = run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "topic", "upstream": null }),
    );
    assert_eq!(out["upstream"], Value::Null);
    assert_eq!(out["previous"], "origin/main");
    assert_eq!(
        out["next_undo"]["text"],
        "Remove the upstream `origin/main` of `topic`"
    );
    assert_eq!(upstream(&s, "topic"), None);

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        upstream(&s, "topic").as_deref(),
        Some("refs/remotes/origin/main")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(upstream(&s, "topic"), None);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        upstream(&s, "topic").as_deref(),
        Some("refs/remotes/origin/main")
    );
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(upstream(&s, "topic"), None);
}

#[test]
fn set_upstream_replaces_the_previous_one() {
    let (r, mut s) = opened();
    let out = run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "main", "upstream": "origin/feature/graph" }),
    );
    assert_eq!(out["previous"], "origin/main");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        upstream(&s, "main").as_deref(),
        Some("refs/remotes/origin/main")
    );
}

#[test]
fn set_upstream_refusals() {
    let (r, mut s) = opened();
    // Missing `upstream`: an error, never "remove".
    let err = r
        .run("branch.set_upstream", &mut s, json!({ "name": "main" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err:?}");
    for upstream in ["", "origin/nope", "main", "origin/HEAD"] {
        let err = r
            .run(
                "branch.set_upstream",
                &mut s,
                json!({ "name": "main", "upstream": upstream }),
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{upstream}: {err:?}"
        );
    }
    let err = r
        .run(
            "branch.set_upstream",
            &mut s,
            json!({ "name": "nope", "upstream": "origin/main" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err:?}");
    let err = r
        .run(
            "branch.set_upstream",
            &mut s,
            json!({ "name": "main", "upstream": "origin/main", "extra": 1 }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err:?}");
    // Nothing changed.
    assert_eq!(
        upstream(&s, "main").as_deref(),
        Some("refs/remotes/origin/main")
    );
}

#[test]
fn undo_refuses_an_upstream_changed_from_outside() {
    let (r, mut s) = opened();
    run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "main", "upstream": null }),
    );
    // Someone sets another upstream from a terminal.
    s.repo_mut()
        .unwrap()
        .write_upstream("main", &[], &entries("main", "origin", "feature/graph"))
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err:?}");
    assert!(err.to_string().contains("origin/feature/graph"), "{err}");
    assert_eq!(
        upstream(&s, "main").as_deref(),
        Some("refs/remotes/origin/feature/graph")
    );
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"].as_array().unwrap().len(), 1, "entry stays");
}

#[test]
fn undo_of_a_tracking_create_leaves_a_changed_upstream_with_a_warning() {
    let (r, mut s) = opened();
    run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "graph", "start": "origin/feature/graph", "track": true }),
    );
    s.repo_mut()
        .unwrap()
        .write_upstream(
            "graph",
            &entries("graph", "origin", "feature/graph"),
            &entries("graph", "origin", "main"),
        )
        .unwrap();
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(branch(&s, "graph"), None, "the branch is deleted anyway");
    let warnings = undo["warnings"].to_string();
    assert!(warnings.contains("left as it is"), "{warnings}");
}
