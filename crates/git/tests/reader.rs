//! Integration tests of `Session::reader` on real repos: the reads the UI runs beside a write in
//! progress (architecture §6, "Reads beside a write").

mod common;

use common::{Repo, repo_with_changes};
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn opened(repo: &Repo) -> (Registry, Session) {
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    registry
        .run(
            "repo.open",
            &mut session,
            json!({ "path": repo.path_str() }),
        )
        .unwrap();
    (registry, session)
}

/// Every command with `CommandSpec::reader`, with parameters for [`repo_with_changes`]; the
/// boolean says whether it answers there (`conflict.show`: no file is in conflict).
fn reads() -> Vec<(&'static str, Value, bool)> {
    vec![
        (
            "branch.reset_preview",
            json!({ "target": "HEAD", "mode": "hard" }),
            true,
        ),
        ("commit.show", json!({ "id": "HEAD" }), true),
        ("config.identity", json!({}), true),
        ("conflict.show", json!({ "path": "a.txt" }), false),
        ("diff.commit", json!({ "id": "HEAD" }), true),
        ("diff.index", json!({}), true),
        (
            "diff.range",
            json!({ "from": "HEAD~1", "to": "HEAD" }),
            true,
        ),
        ("diff.worktree", json!({}), true),
        ("file.blame", json!({ "path": "a.txt" }), true),
        ("file.log", json!({ "path": "a.txt" }), true),
        ("rebase.plan", json!({ "base": "HEAD~1" }), true),
        ("remote.list", json!({}), true),
        ("repo.lfs_paths", json!({ "paths": ["a.txt"] }), true),
        ("repo.lfs_status", json!({}), true),
        ("repo.log", json!({}), true),
        ("repo.operation", json!({}), true),
        ("repo.refs", json!({}), true),
        ("repo.search", json!({ "query": "first" }), true),
        ("repo.status", json!({}), true),
        ("stash.list", json!({}), true),
        ("submodule.list", json!({}), true),
        ("worktree.list", json!({}), true),
    ]
}

#[test]
fn the_reads_need_no_lock_and_leave_the_index_as_it_is() {
    let repo = repo_with_changes();
    let (registry, session) = opened(&repo);
    let mut reader = session.reader().unwrap();
    let ids: Vec<_> = registry.list().filter(|c| c.reader).map(|c| c.id).collect();
    assert_eq!(ids, reads().iter().map(|r| r.0).collect::<Vec<_>>());

    // A tracked file rewritten as it was, with a date in the past (otherwise it would be
    // "racily clean"): its `stat` data in the index are stale, and a git command free to do so
    // would take the lock and rewrite the index.
    repo.write("renamed.txt", "content long enough to be recognized\n");
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(repo.path().join("renamed.txt"))
        .unwrap()
        .set_modified(past)
        .unwrap();
    let index = repo.path().join(".git/index");
    let before = std::fs::read(&index).unwrap();
    let lock = repo.path().join(".git/index.lock");

    // Free to take the lock, then with the write in progress on the session holding it.
    for locked in [false, true] {
        if locked {
            std::fs::write(&lock, "").unwrap();
        }
        for (id, params, answers) in reads() {
            let result = registry.run(id, &mut reader, params);
            match &result {
                Ok(_) => assert!(answers, "{id}"),
                Err(EngineError::NotAllowed(_)) => assert!(!answers, "{id}: {result:?}"),
                Err(e) => panic!("{id} (locked: {locked}): {e}"),
            }
            assert_eq!(lock.exists(), locked, "{id}");
            assert_eq!(
                std::fs::read(&index).unwrap(),
                before,
                "{id} wrote the index"
            );
        }
    }
    std::fs::remove_file(&lock).unwrap();

    // The index was there to be rewritten: a plain `git status` does.
    repo.git(&["status"]);
    assert_ne!(std::fs::read(&index).unwrap(), before);
}

#[test]
fn a_reader_writes_nothing_and_sees_what_the_session_writes() {
    let repo = repo_with_changes();
    let (registry, mut session) = opened(&repo);
    let mut reader = session.reader().unwrap();
    let status = registry.run("repo.status", &mut reader, json!({})).unwrap();
    let head = repo.rev("HEAD");

    for (id, params) in [
        ("index.stage", json!({ "paths": ["a.txt"] })),
        ("commit.create", json!({ "message": "from the reader" })),
        ("stash.push", json!({})),
        ("branch.create", json!({ "name": "beside" })),
        ("repo.open", json!({ "path": repo.path_str() })),
        ("edit.history", json!({})),
    ] {
        let err = registry.run(id, &mut reader, params).unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reader-session"),
            "{id}: {err}"
        );
    }
    assert_eq!(repo.rev("HEAD"), head);
    assert_eq!(repo.git(&["branch", "--list", "beside"]), "");
    assert_eq!(
        registry.run("repo.status", &mut reader, json!({})).unwrap(),
        status
    );

    // The same repository on disk: no new reader is needed after a write.
    let out = registry
        .run("commit.create", &mut session, json!({ "message": "third" }))
        .unwrap();
    let log = registry
        .run("repo.log", &mut reader, json!({ "limit": 1 }))
        .unwrap();
    assert_eq!(log["commits"][0]["id"], out["commit"]["id"]);
}
