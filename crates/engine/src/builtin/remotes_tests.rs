//! Tests of `remote.list`, `remote.add`, `remote.remove` on the `MemoryBackend`.

use crate::test_util::oid;
use crate::test_util::same;
use crate::{EngineError, GitRemotes, MemoryBackend, MemoryOpener, Registry, Session};
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

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn remote_refs(s: &Session, remote: &str) -> Vec<(String, Oid)> {
    s.repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.name.starts_with(&format!("refs/remotes/{remote}/")))
        .map(|r| (r.name, r.target))
        .collect()
}

fn upstream_of(s: &Session, branch: &str) -> Option<String> {
    s.repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == format!("refs/heads/{branch}"))
        .and_then(|r| r.upstream)
        .map(|u| u.name)
}

#[test]
fn list_masks_credentials_and_marks_the_defaults() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "https://nicola:secret@example.com/fork.git");
    repo.add_remote("gh", "https://ghp_token@github.com/x/y.git");
    repo.set_config("remote.fork.pushurl", &["ssh://git@example.com/fork.git"]);
    repo.set_config("remote.pushDefault", &["fork"]);
    let (r, mut s) = opened_with(repo);
    let out = run(&r, &mut s, "remote.list", json!({}));
    let text = out.to_string();
    assert!(
        !text.contains("secret") && !text.contains("ghp_token"),
        "{text}"
    );
    let names: Vec<&str> = out["remotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["fork", "gh", "origin"]);
    assert_eq!(
        out["remotes"][0],
        json!({
            "name": "fork",
            "url": "https://nicola:***@example.com/fork.git",
            "push_urls": ["ssh://git@example.com/fork.git"],
            "fetch": ["+refs/heads/*:refs/remotes/fork/*"],
            "mirror": false,
            "branches": 0,
            "default": false,
            "push_default": true,
            "credentials": true,
        })
    );
    assert_eq!(out["remotes"][1]["url"], "https://***@github.com/x/y.git");
    let origin = &out["remotes"][2];
    assert_eq!(origin["url"], "https://example.com/project.git");
    assert_eq!(origin["credentials"], false);
    assert_eq!(origin["push_urls"], json!([]));
    assert_eq!(origin["branches"], 2);
    assert_eq!(origin["default"], true);
    assert_eq!(out["default"], "origin");
    assert_eq!(out["push_default"], "fork");

    let err = r
        .run("remote.list", &mut s, json!({ "name": "x" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
}

#[test]
fn add_with_fetch_then_undo_and_redo() {
    // The origin "server" stays aside when the remote is removed: added with the same URL,
    // the fetch downloads its branches.
    let mut repo = MemoryBackend::sample();
    repo.add_remote("copy", "https://u:pw@example.com/copy.git");
    repo.set_branch_on_remote("copy", "main", oid(12));
    let snapshot = repo.remote_snapshot("copy").unwrap();
    repo.remove_remote_config(&snapshot);
    let (r, mut s) = opened_with(repo);

    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "fresh", "url": "https://u:pw@example.com/copy.git", "fetch": true }),
    );
    assert_eq!(out["remote"], "fresh");
    assert_eq!(out["url"], "https://u:***@example.com/copy.git");
    assert_eq!(out["credentials"], true);
    assert_eq!(out["fetched"], true);
    assert_eq!(out["fetch_error"], Value::Null);
    assert!(
        out["warnings"][0]["text"]
            .as_str()
            .unwrap()
            .contains("in clear text")
    );
    assert_eq!(
        out["fetch"]["updated"],
        json!([{ "name": "refs/remotes/fresh/main", "before": null, "after": oid(12).to_string() }])
    );
    assert!(!out.to_string().contains("pw@"), "{out}");
    assert!(
        out["remotes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "fresh" && r["branches"] == 1)
    );
    assert_eq!(
        out["next_undo"]["text"],
        "Add remote `fresh` (with fetch: 1 remote branch)"
    );
    // The journal does not show the password.
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert!(!history.to_string().contains("pw@"), "{history}");

    run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        !s.repo()
            .unwrap()
            .remotes()
            .unwrap()
            .contains(&"fresh".to_owned())
    );
    assert!(remote_refs(&s, "fresh").is_empty());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        remote_refs(&s, "fresh"),
        [("refs/remotes/fresh/main".to_owned(), oid(12))]
    );
    assert_eq!(
        s.repo().unwrap().config_values("remote.fresh.url").unwrap(),
        ["https://u:pw@example.com/copy.git"]
    );
    let snapshot = s.repo().unwrap().remote_snapshot("fresh").unwrap();
    assert_eq!(snapshot.url(), Some("https://u:pw@example.com/copy.git"));
}

#[test]
fn add_without_fetch_and_fetch_errors_do_not_undo_the_add() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "fork", "url": "/srv/fork.git" }),
    );
    assert_eq!(out["fetched"], false);
    assert_eq!(out["fetch"], Value::Null);
    assert_eq!(out["credentials"], false);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["next_undo"]["text"], "Add remote `fork`");

    let mut repo = MemoryBackend::sample();
    repo.add_remote("broken", "/srv/broken.git");
    repo.fail_fetch("broken", Some(crate::FetchFailure::Auth("HTTP 401".into())));
    let snapshot = repo.remote_snapshot("broken").unwrap();
    repo.remove_remote_config(&snapshot);
    let (r, mut s) = opened_with(repo);
    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "broken", "url": "/srv/broken.git", "fetch": true }),
    );
    assert_eq!(out["fetched"], false);
    assert_eq!(out["fetch_error"]["kind"], "auth");
    assert!(
        s.repo()
            .unwrap()
            .remotes()
            .unwrap()
            .contains(&"broken".to_owned())
    );
    assert_eq!(out["can_undo"], true);
}

#[test]
fn add_refuses_existing_names_and_bad_names_or_urls() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let err = r
        .run(
            "remote.add",
            &mut s,
            json!({ "name": "origin", "url": "/srv/x.git" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(
        s.repo().unwrap().remote_snapshot("origin").unwrap().url(),
        Some("https://example.com/project.git")
    );
    for name in ["-x", "--upload-pack=x", "", "a b", "a..b"] {
        let err = r
            .run(
                "remote.add",
                &mut s,
                json!({ "name": name, "url": "/srv/x" }),
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{name}: {err}"
        );
    }
    for url in [
        "-x",
        "--upload-pack=touch x",
        "ext::sh -c touch% /tmp/x",
        "fd::3",
        "",
    ] {
        let err = r
            .run("remote.add", &mut s, json!({ "name": "x", "url": url }))
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{url}: {err}"
        );
    }
    let err = r
        .run(
            "remote.add",
            &mut s,
            json!({ "name": "x", "url": "/a", "prune": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert_eq!(s.repo().unwrap().remotes().unwrap(), ["origin"]);
    assert_eq!(
        run(&r, &mut s, "edit.history", json!({}))["can_undo"],
        false
    );

    // Remote-tracking branches left over from a remote removed by hand: undo would delete them.
    let mut repo = MemoryBackend::sample();
    repo.set_remote_branch("old/main", oid(11));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run("remote.add", &mut s, json!({ "name": "old", "url": "/x" }))
        .unwrap_err();
    assert!(err.to_string().contains("refs/remotes/old/"), "{err}");
}

#[test]
fn remove_needs_confirm_and_undo_restores_everything() {
    let mut repo = MemoryBackend::sample();
    repo.set_config("branch.feature/graph.pushRemote", &["origin"]);
    let (r, mut s) = opened_with(repo);
    let before = s.repo().unwrap().remote_snapshot("origin").unwrap();
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "origin" }))
        .unwrap_err();
    let EngineError::ConfirmRequired { reason, .. } = &err else {
        panic!("{err}");
    };
    assert!(reason.contains("2 remote-tracking branches"), "{reason}");
    assert!(
        reason.contains("`feature/graph`, `main`") || reason.contains("`main`"),
        "{reason}"
    );
    assert!(reason.contains("pushRemote"), "{reason}");
    assert_eq!(s.repo().unwrap().remote_snapshot("origin").unwrap(), before);

    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert_eq!(out["remote"], "origin");
    assert_eq!(out["upstreams_removed"], json!(["feature/graph", "main"]));
    assert_eq!(out["push_remotes_removed"], json!(["feature/graph"]));
    assert_eq!(out["push_default_removed"], false);
    assert_eq!(out["removed"]["refs"].as_array().unwrap().len(), 2);
    assert_eq!(out["remotes"], json!([]));
    // origin/main (11) and origin/feature/graph (9) are reachable from main: no backup.
    assert_eq!(out["backups"], json!([]));
    assert!(remote_refs(&s, "origin").is_empty());
    assert_eq!(upstream_of(&s, "main"), None);
    assert!(s.repo().unwrap().remotes().unwrap().is_empty());
    assert!(
        s.repo()
            .unwrap()
            .config_values("branch.feature/graph.pushRemote")
            .unwrap()
            .is_empty()
    );

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["warnings"], json!([]));
    assert_eq!(s.repo().unwrap().remote_snapshot("origin").unwrap(), before);
    assert_eq!(
        upstream_of(&s, "main").as_deref(),
        Some("refs/remotes/origin/main")
    );
    // The "server" is back: the fetch works as before.
    let fetch = run(&r, &mut s, "remote.fetch", json!({}));
    assert_eq!(
        fetch["updated"][0]["name"],
        "refs/remotes/origin/feature/graph"
    );
    // After the fetch the remote has changed: the redo is refused and nothing changes.
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(remote_refs(&s, "origin").len(), 2);
}

#[test]
fn remove_a_remote_with_commits_only_there_saves_a_backup() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    run(&r, &mut s, "remote.fetch", json!({}));
    // oid(13) is only on origin/feature/graph.
    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert_eq!(out["backups"], json!(["refs/project/backup/remote-1"]));
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        remote_refs(&s, "origin")
            .contains(&("refs/remotes/origin/feature/graph".to_owned(), oid(13)))
    );
    // Redo: removed again, without a new backup.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["backups"], json!([]));
    assert!(remote_refs(&s, "origin").is_empty());
}

#[test]
fn remove_push_default_and_undo_conflicts() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "/srv/fork.git");
    repo.set_remote_branch("fork/main", oid(12));
    repo.set_config("remote.pushDefault", &["fork"]);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "fork" }))
        .unwrap_err();
    assert!(err.to_string().contains("remote.pushDefault"), "{err}");
    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "fork", "confirm": true }),
    );
    assert_eq!(out["push_default_removed"], true);
    let config = |s: &Session| {
        s.repo()
            .unwrap()
            .config_values("remote.pushDefault")
            .unwrap()
    };
    assert!(config(&s).is_empty());
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(config(&s), ["fork"]);
    run(&r, &mut s, "edit.redo", json!({}));

    // Recreated outside the journal: UndoConflict, nothing changes.
    let (r, mut s) = opened_with(MemoryBackend::sample());
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    s.repo_mut()
        .unwrap()
        .create_remote("origin", "/srv/fresh.git")
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert!(remote_refs(&s, "origin").is_empty());
    assert_eq!(
        s.repo().unwrap().remote_snapshot("origin").unwrap().url(),
        Some("/srv/fresh.git")
    );
}

#[test]
fn undo_of_add_is_refused_once_a_branch_uses_the_remote() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "fork", "url": "/srv/fork.git" }),
    );
    // The first push sets `main`'s upstream to `fork/main` (the push is not in the journal).
    run(&r, &mut s, "remote.push", json!({ "remote": "fork" }));
    assert_eq!(
        upstream_of(&s, "main").as_deref(),
        Some("refs/remotes/fork/main")
    );
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let EngineError::UndoConflict { refs, .. } = &err else {
        panic!("{err}");
    };
    assert!(
        refs.iter().any(|r| r.contains("refs/remotes/fork/main")),
        "{refs:?}"
    );
    assert!(refs.iter().any(|r| r.contains("upstream")), "{refs:?}");
    assert!(
        s.repo()
            .unwrap()
            .remotes()
            .unwrap()
            .contains(&"fork".to_owned())
    );
}

#[test]
fn remove_refuses_unknown_remotes_and_operations() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "missing", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`origin`"), "{err}");
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "a..b" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(project_domain::Operation::Merge));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(remote_refs(&s, "origin").len(), 2);
}

#[test]
fn ahead_and_behind_follow_fetch_pull_and_push() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let track = |out: &Value, name: &str| {
        let r = out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone();
        (
            r["upstream"]["ahead"].clone(),
            r["upstream"]["behind"].clone(),
        )
    };
    let refs = run(&r, &mut s, "repo.refs", json!({}));
    let refs = json!({ "refs": refs });
    assert_eq!(track(&refs, "refs/heads/main"), (json!(1), json!(0)));
    assert_eq!(
        track(&refs, "refs/heads/feature/graph"),
        (json!(0), json!(0))
    );
    let out = run(&r, &mut s, "remote.fetch", json!({}));
    assert_eq!(
        track(&out, "refs/heads/feature/graph"),
        (json!(0), json!(1))
    );
    let out = run(&r, &mut s, "remote.push", json!({}));
    assert_eq!(track(&out, "refs/heads/main"), (json!(0), json!(0)));
    let out = run(
        &r,
        &mut s,
        "remote.pull",
        json!({ "branch": "feature/graph" }),
    );
    assert_eq!(
        track(&out, "refs/heads/feature/graph"),
        (json!(0), json!(0))
    );
}

#[test]
fn remove_refuses_remotes_defined_elsewhere_and_promisors() {
    let mut repo = MemoryBackend::sample();
    repo.set_remote_elsewhere(
        "origin",
        &["`remote.origin.fetch` (global, file:~/.gitconfig)"],
    );
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("~/.gitconfig"), "{err}");
    assert_eq!(remote_refs(&s, "origin").len(), 2);

    for (key, value) in [
        ("remote.origin.promisor", "true"),
        ("extensions.partialClone", "origin"),
    ] {
        let mut repo = MemoryBackend::sample();
        repo.set_config(key, &[value]);
        let (r, mut s) = opened_with(repo);
        let err = r
            .run("remote.remove", &mut s, json!({ "name": "origin" }))
            .unwrap_err();
        assert!(err.to_string().contains("promisor"), "{key}: {err}");
        assert_eq!(remote_refs(&s, "origin").len(), 2);
    }
}

#[test]
fn cancelled_fetch_of_remote_add_names_the_command() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let token = crate::CancelToken::new();
    token.cancel();
    let out = r
        .run_with(
            "remote.add",
            &mut s,
            json!({ "name": "fork", "url": "/srv/fork.git", "fetch": true }),
            crate::JobContext::new().with_cancel(token),
        )
        .unwrap();
    assert_eq!(out["fetch_error"]["kind"], "cancelled");
    assert!(
        out["fetch_error"]["message"]
            .as_str()
            .unwrap()
            .contains("remote.add")
    );
}

#[test]
fn masked_values_and_valueless_keys() {
    let mut repo = MemoryBackend::sample();
    repo.set_config("remote.origin.proxy", &["http://u:secret@proxy:3128"]);
    let (r, mut s) = opened_with(repo);
    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert!(!out.to_string().contains("secret"), "{out}");
    assert!(out.to_string().contains("http://u:***@proxy:3128"), "{out}");

    let mut a = crate::RemoteSnapshot {
        name: "o".into(),
        config: vec![crate::ConfigEntry {
            key: "remote.o.skipFetchAll".into(),
            value: None,
        }],
        ..crate::RemoteSnapshot::default()
    };
    let mut b = a.clone();
    b.config[0].value = Some("true".into());
    assert!(a.differences(&b).is_empty());
    a.config[0].value = Some("false".into());
    assert!(!a.differences(&b).is_empty());
}

#[test]
fn a_remote_still_listed_after_removal_is_put_back() {
    let mut repo = MemoryBackend::sample();
    repo.keep_remote_listed("origin");
    let (r, mut s) = opened_with(repo);
    let before = s.repo().unwrap().remote_snapshot("origin").unwrap();
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("put back as it was"), "{err}");
    assert_eq!(s.repo().unwrap().remote_snapshot("origin").unwrap(), before);
    // Everything put back: nothing in the journal.
    assert_eq!(
        run(&r, &mut s, "edit.history", json!({}))["can_undo"],
        false
    );
}

#[test]
fn a_failed_put_back_writes_the_config_and_keeps_the_journal_entry() {
    let mut repo = MemoryBackend::sample();
    repo.keep_remote_listed("origin");
    // The removal succeeds, the restoration of the remote-tracking branches does not.
    repo.fail_restore_refs_after(1);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("only in part") && text.contains("journal"),
        "{text}"
    );
    // The configuration was rewritten anyway; the remote-tracking branches were not.
    let now = s.repo().unwrap().remote_snapshot("origin").unwrap();
    assert_eq!(now.url(), Some("https://example.com/project.git"));
    assert!(now.refs.is_empty());
    assert_eq!(
        upstream_of(&s, "main").as_deref(),
        Some("refs/remotes/origin/main")
    );
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"][0]["command"], "remote.remove");
}

/// The typed results read back the JSON of the commands and give the same JSON again: the UI
/// deserializes into them.
#[test]
fn results_round_trip() {
    use crate::{
        FetchErrorKind, RemoteAddResult, RemoteListResult, RemoteRemoveResult, RemovedRemote,
    };
    // `remote.list` with credentials, push URLs and `remote.pushDefault`.
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "https://nicola:secret@example.com/fork.git");
    repo.set_config("remote.fork.pushurl", &["ssh://git@example.com/fork.git"]);
    repo.set_config("remote.pushDefault", &["fork"]);
    repo.set_config("branch.feature/graph.pushRemote", &["origin"]);
    // A remote whose fetch fails, ready to be added again.
    repo.add_remote("broken", "/srv/broken.git");
    repo.fail_fetch("broken", Some(crate::FetchFailure::Auth("HTTP 401".into())));
    let snapshot = repo.remote_snapshot("broken").unwrap();
    repo.remove_remote_config(&snapshot);
    // A remote with a branch, ready to be added again and fetched.
    repo.add_remote("copy", "https://u:pw@example.com/copy.git");
    repo.set_branch_on_remote("copy", "main", oid(12));
    let snapshot = repo.remote_snapshot("copy").unwrap();
    repo.remove_remote_config(&snapshot);
    let (r, mut s) = opened_with(repo);

    let list: RemoteListResult = same(run(&r, &mut s, "remote.list", json!({})));
    assert_eq!(list.push_default.as_deref(), Some("fork"));
    assert_eq!(list.default.as_deref(), Some("origin"));
    let fork = &list.remotes[0];
    assert!(fork.credentials && fork.push_default && !fork.default);
    assert_eq!(fork.push_urls, ["ssh://git@example.com/fork.git"]);

    // Add with a fetch that succeeds.
    let add: RemoteAddResult = same(run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "fresh", "url": "https://u:pw@example.com/copy.git", "fetch": true }),
    ));
    assert!(add.fetched && add.credentials && add.fetch_error.is_none());
    let fetch = add.fetch.unwrap();
    assert_eq!(fetch.updated.len(), 1);
    assert_eq!(fetch.updated[0].after, Some(oid(12)));
    assert_eq!(add.warnings.len(), 1);
    assert!(add.remotes.is_some() && add.remotes_error.is_none());
    assert!(add.refs.is_some() && add.refs_error.is_none());
    assert!(add.journal.can_undo);

    // Add with a fetch that fails: the remote stays.
    let add: RemoteAddResult = same(run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "broken", "url": "/srv/broken.git", "fetch": true }),
    ));
    assert!(!add.fetched && add.fetch.is_none());
    assert_eq!(add.fetch_error.unwrap().kind, FetchErrorKind::Auth);

    // Add without fetch.
    let add: RemoteAddResult = same(run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "plain", "url": "/srv/plain.git" }),
    ));
    assert!(!add.fetched && add.fetch.is_none() && add.fetch_error.is_none());

    // Remove with upstreams, a pushRemote and a backup.
    run(&r, &mut s, "remote.fetch", json!({ "remote": "origin" }));
    let removed: RemoteRemoveResult = same(run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    ));
    assert_eq!(removed.remote, "origin");
    assert!(!removed.upstreams_removed.is_empty());
    assert_eq!(removed.push_remotes_removed, ["feature/graph"]);
    assert!(!removed.push_default_removed);
    assert_eq!(removed.backups, ["refs/project/backup/remote-1"]);
    let RemovedRemote { name, refs, .. } = &removed.removed;
    assert_eq!(name, "origin");
    assert!(!refs.is_empty());

    // Remove `remote.pushDefault` and a URL with credentials: masked in `removed`.
    let removed: RemoteRemoveResult = same(run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "fork", "confirm": true }),
    ));
    assert!(removed.push_default_removed && removed.removed.push_default);
    assert!(!serde_json::to_string(&removed).unwrap().contains("secret"));
}

/// A re-read that fails gives `null` and `<key>_error`.
#[test]
fn reread_error_fields() {
    use super::super::{JournalState, reread};
    use crate::{
        FetchErrorKind, Message, RemoteAddFetchError, RemoteAddResult, RemoteListEntry,
        RemoteRemoveResult, RemovedRemote,
    };
    use project_domain::RefInfo;
    let error = EngineError::NoRepo.to_string();
    let (remotes, remotes_error) = reread::<Vec<RemoteListEntry>>(Err(EngineError::NoRepo));
    let (refs, refs_error) = reread::<Vec<RefInfo>>(Err(EngineError::NoRepo));
    let add = RemoteAddResult {
        remote: "x".into(),
        url: "/srv/x.git".into(),
        credentials: false,
        fetched: false,
        fetch: None,
        fetch_error: Some(RemoteAddFetchError {
            kind: FetchErrorKind::Unreachable,
            message: "down".into(),
        }),
        warnings: vec![Message::plain("w")],
        remotes: remotes.clone(),
        remotes_error: remotes_error.clone(),
        refs: refs.clone(),
        refs_error: refs_error.clone(),
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&add).unwrap();
    for key in ["remotes", "refs", "fetch", "next_undo"] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(
        out["fetch_error"],
        json!({ "kind": "unreachable", "message": "down" })
    );
    assert_eq!(out["remotes_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    let back: RemoteAddResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, add);

    let remove = RemoteRemoveResult {
        remote: "x".into(),
        removed: RemovedRemote {
            name: "x".into(),
            config: vec![],
            refs: vec![],
            head: None,
            upstreams: vec![],
            push_default: false,
        },
        upstreams_removed: vec![],
        push_remotes_removed: vec![],
        push_default_removed: false,
        backups: vec![],
        warnings: vec![],
        remotes,
        remotes_error,
        refs,
        refs_error,
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&remove).unwrap();
    for key in ["remotes", "refs"] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(out["remotes_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    let back: RemoteRemoveResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, remove);

    // The other kinds of fetch error.
    for (kind, text) in [
        (FetchErrorKind::Auth, "auth"),
        (FetchErrorKind::Cancelled, "cancelled"),
        (FetchErrorKind::Other, "other"),
    ] {
        assert_eq!(serde_json::to_value(kind).unwrap(), text);
    }
}

/// `removed` is the same JSON as the `RemoteSnapshot` it comes from (credentials masked).
#[test]
fn removed_remote_is_the_snapshot_json() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "https://nicola:secret@example.com/fork.git");
    repo.set_branch_on_remote("fork", "main", oid(12));
    repo.set_config("branch.main.pushRemote", &["fork"]);
    repo.set_config("remote.pushDefault", &["fork"]);
    let snapshot = repo.remote_snapshot("fork").unwrap();
    let view = super::RemovedRemote::of(&snapshot);
    let out = serde_json::to_value(&view).unwrap();
    assert_eq!(out, serde_json::to_value(&snapshot).unwrap());
    assert!(!out.to_string().contains("secret"), "{out}");
}
