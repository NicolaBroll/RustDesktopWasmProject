//! Tests of `remote.set_url` and `remote.rename` (S59) on the `MemoryBackend`.

use crate::{
    EngineError, GitRead, GitRemotes, HeadState, Journal, JournalEntry, MemoryBackend,
    MemoryOpener, Registry, Session,
};
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

fn config(s: &Session, key: &str) -> Vec<String> {
    s.repo().unwrap().config_values(key).unwrap()
}

/// The names of the refs under `refs/remotes/<remote>/`.
fn remote_refs(s: &Session, remote: &str) -> Vec<String> {
    s.repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.name.starts_with(&format!("refs/remotes/{remote}/")))
        .map(|r| r.name)
        .collect()
}

fn upstream_of(s: &Session, branch: &str) -> Option<String> {
    s.repo()
        .unwrap()
        .upstream(branch)
        .unwrap()
        .map(|u| u.remote)
}

fn can_undo(s: &Session) -> bool {
    s.journal().is_ok_and(|j| j.can_undo())
}

#[test]
fn set_url_keeps_refs_and_upstreams_and_undo_redo() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let refs = remote_refs(&s, "origin");
    let out = run(
        &r,
        &mut s,
        "remote.set_url",
        json!({ "name": "origin", "url": "https://u:secret@example.com/new.git" }),
    );
    assert_eq!(out["changed"], true);
    assert_eq!(out["url"], "https://u:***@example.com/new.git");
    assert_eq!(out["push_url"], Value::Null);
    assert_eq!(out["credentials"], true);
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1);
    assert!(!out.to_string().contains("secret"), "{out}");
    assert_eq!(
        config(&s, "remote.origin.url"),
        ["https://u:secret@example.com/new.git"]
    );
    assert_eq!(remote_refs(&s, "origin"), refs);
    assert_eq!(upstream_of(&s, "main").as_deref(), Some("origin"));

    let history = run(&r, &mut s, "edit.history", json!({}));
    assert!(!history.to_string().contains("secret"), "{history}");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        config(&s, "remote.origin.url"),
        ["https://example.com/project.git"]
    );
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        config(&s, "remote.origin.url"),
        ["https://u:secret@example.com/new.git"]
    );

    // Changed from outside after the command: the undo is refused, nothing changes.
    s.repo_mut()
        .unwrap()
        .set_remote_url("origin", false, Some("/srv/other.git"))
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let EngineError::UndoConflict { refs, .. } = &err else {
        panic!("{err}");
    };
    assert!(refs[0].contains("remote.origin.url"), "{refs:?}");
    assert!(!err.to_string().contains("secret"), "{err}");
    assert_eq!(config(&s, "remote.origin.url"), ["/srv/other.git"]);
}

#[test]
fn set_push_url_only_and_undo_removes_it() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = run(
        &r,
        &mut s,
        "remote.set_url",
        json!({ "name": "origin", "url": "ssh://git@example.com/project.git", "push": true }),
    );
    assert_eq!(out["changed"], true);
    assert_eq!(out["url"], "https://example.com/project.git");
    assert_eq!(out["push_url"], "ssh://git@example.com/project.git");
    assert_eq!(out["credentials"], false);
    assert_eq!(
        config(&s, "remote.origin.url"),
        ["https://example.com/project.git"]
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(config(&s, "remote.origin.pushurl").is_empty());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        config(&s, "remote.origin.pushurl"),
        ["ssh://git@example.com/project.git"]
    );
}

#[test]
fn same_url_changes_nothing() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = run(
        &r,
        &mut s,
        "remote.set_url",
        json!({ "name": "origin", "url": "https://example.com/project.git" }),
    );
    assert_eq!(out["changed"], false);
    assert_eq!(out["can_undo"], false);
    assert!(!can_undo(&s));
}

#[test]
fn set_url_refusals_change_nothing() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("glob", "/srv/glob.git");
    repo.set_remote_elsewhere("glob", &["`remote.glob.url` (global, ~/.gitconfig)"]);
    repo.set_config("remote.origin.pushurl", &["/a", "/b"]);
    let (r, mut s) = opened_with(repo);
    let cases = [
        (json!({ "name": "missing", "url": "/x" }), "not configured"),
        (json!({ "name": "glob", "url": "/x" }), "outside"),
        (
            json!({ "name": "origin", "url": "/x", "push": true }),
            "values",
        ),
    ];
    for (params, why) in cases {
        let err = r.run("remote.set_url", &mut s, params.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{why}: {err}");
    }
    for params in [
        json!({ "name": "origin", "url": "--upload-pack=touch /tmp/x" }),
        json!({ "name": "origin", "url": "ext::sh -c x" }),
        json!({ "name": "origin", "url": "/a\nb" }),
        json!({ "name": "origin", "url": "/x", "fetch": true }),
        json!({ "name": "-x", "url": "/x" }),
    ] {
        let err = r.run("remote.set_url", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::InvalidParams { .. } | EngineError::NotAllowed(_)
            ),
            "{params}: {err}"
        );
    }
    assert_eq!(
        config(&s, "remote.origin.url"),
        ["https://example.com/project.git"]
    );
    assert!(!can_undo(&s));

    // git fails: error with git's text, nothing in the journal.
    let mut repo = MemoryBackend::sample();
    repo.fail_config_writes(Some("error: could not lock config file"));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.set_url",
            &mut s,
            json!({ "name": "origin", "url": "/x" }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("could not lock"), "{err}");
    assert!(!can_undo(&s));
}

#[test]
fn rename_moves_refs_upstreams_push_remote_and_push_default() {
    let mut repo = MemoryBackend::sample();
    repo.set_config("branch.main.pushRemote", &["origin"]);
    repo.set_config("remote.pushDefault", &["origin"]);
    let (r, mut s) = opened_with(repo);
    let tags = s.repo().unwrap().refs().unwrap();
    let tags: Vec<_> = tags
        .into_iter()
        .filter(|r| r.name.starts_with("refs/tags/"))
        .collect();
    let out = run(
        &r,
        &mut s,
        "remote.rename",
        json!({ "name": "origin", "new_name": "upstream" }),
    );
    assert_eq!(out["from"], "origin");
    assert_eq!(out["to"], "upstream");
    assert_eq!(
        out["moved_refs"],
        json!([
            "refs/remotes/upstream/feature/graph",
            "refs/remotes/upstream/main"
        ])
    );
    assert_eq!(out["upstreams"], json!(["feature/graph", "main"]));
    assert_eq!(out["warnings"], json!([]));
    assert!(remote_refs(&s, "origin").is_empty());
    assert_eq!(upstream_of(&s, "main").as_deref(), Some("upstream"));
    assert_eq!(config(&s, "branch.main.pushRemote"), ["upstream"]);
    assert_eq!(config(&s, "remote.pushDefault"), ["upstream"]);
    let after: Vec<_> = s
        .repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.name.starts_with("refs/tags/"))
        .collect();
    assert_eq!(after, tags);

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(remote_refs(&s, "origin").len(), 2);
    assert!(remote_refs(&s, "upstream").is_empty());
    assert_eq!(upstream_of(&s, "main").as_deref(), Some("origin"));
    assert_eq!(config(&s, "remote.pushDefault"), ["origin"]);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(remote_refs(&s, "upstream").len(), 2);

    // The old name recreated in the meantime: the undo is refused, nothing changes.
    s.repo_mut()
        .unwrap()
        .create_remote("origin", "/srv/fresh.git")
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(remote_refs(&s, "upstream").len(), 2);
}

#[test]
fn rename_refusals_change_nothing() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "/srv/fork.git");
    repo.add_remote("fork/a", "/srv/fork-a.git");
    repo.add_remote("glob", "/srv/glob.git");
    repo.set_remote_elsewhere("glob", &["`remote.glob.fetch` (global, ~/.gitconfig)"]);
    repo.set_remote_branch("old/main", MemoryBackend::fake_oid(1));
    let (r, mut s) = opened_with(repo);
    for (params, invalid) in [
        (json!({ "name": "origin", "new_name": "fork" }), false),
        (json!({ "name": "origin", "new_name": "origin" }), false),
        (json!({ "name": "missing", "new_name": "x" }), false),
        (json!({ "name": "fork", "new_name": "spoon" }), false),
        (json!({ "name": "origin", "new_name": "fork/b" }), false),
        (json!({ "name": "origin", "new_name": "origin/x" }), false),
        (json!({ "name": "glob", "new_name": "x" }), false),
        (json!({ "name": "origin", "new_name": "old" }), false),
        (json!({ "name": "origin", "new_name": "a..b" }), true),
        (json!({ "name": "origin", "new_name": "-x" }), true),
        (json!({ "name": "origin", "new_name": "" }), true),
        (json!({ "name": "origin", "to": "x" }), true),
    ] {
        let err = r.run("remote.rename", &mut s, params.clone()).unwrap_err();
        if invalid {
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{params}: {err}"
            );
        } else {
            assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
        }
    }
    assert_eq!(remote_refs(&s, "origin").len(), 2);
    assert!(!can_undo(&s));

    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(project_domain::Operation::Rebase));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.rename",
            &mut s,
            json!({ "name": "origin", "new_name": "up" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(remote_refs(&s, "origin").len(), 2);
}

#[test]
fn rename_failing_midway_is_in_the_journal() {
    // Before writing: nothing changes, nothing in the journal.
    let mut repo = MemoryBackend::sample();
    repo.fail_remote_rename(Some(false));
    let (r, mut s) = opened_with(repo);
    let params = json!({ "name": "origin", "new_name": "up" });
    let err = r.run("remote.rename", &mut s, params.clone()).unwrap_err();
    assert!(err.to_string().contains("could not lock"), "{err}");
    assert!(!can_undo(&s));
    assert_eq!(remote_refs(&s, "origin").len(), 2);

    // After moving the configuration: the error says so and undo renames it back.
    let mut repo = MemoryBackend::sample();
    repo.fail_remote_rename(Some(true));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.rename", &mut s, params).unwrap_err();
    assert!(err.to_string().contains("edit.undo"), "{err}");
    assert!(can_undo(&s));
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        s.repo()
            .unwrap()
            .remotes()
            .unwrap()
            .contains(&"origin".to_owned())
    );
    assert_eq!(remote_refs(&s, "origin").len(), 2);
    assert_eq!(upstream_of(&s, "main").as_deref(), Some("origin"));
}

#[test]
fn undo_of_rename_is_refused_during_an_operation() {
    let mut repo = MemoryBackend::sample();
    let before = repo.remote_snapshot("origin").unwrap();
    repo.rename_remote("origin", "up").unwrap();
    let after = repo.remote_snapshot("up").unwrap();
    let head = HeadState::from_head(&repo.head().unwrap());
    let mut journal = Journal::default();
    journal.record(JournalEntry::remote_rename(&before, &after, head));
    repo.set_operation(Some(project_domain::Operation::Merge));
    let err = journal.undo(&mut repo).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.remote_snapshot("up").unwrap().refs.len(), 2);
    repo.set_operation(None);
    journal.undo(&mut repo).unwrap();
    assert_eq!(repo.remote_snapshot("origin").unwrap().refs.len(), 2);
}
