//! Integration tests of `tag.create` with the `CliBackend` on real repos: lightweight and annotated
//! tags (also on a remote branch), undo and redo, a tag never overwritten, names that look like
//! options.

mod common;

use common::Repo;
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn tags(repo: &Repo) -> String {
    repo.git(&[
        "for-each-ref",
        "--format=%(refname) %(objecttype) %(objectname)",
        "refs/tags/",
    ])
}

#[test]
fn lightweight_and_annotated_tags_with_undo_and_redo() {
    let remote = Repo::bare();
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    repo.add_remote("origin", &remote);
    repo.push("origin", "HEAD", "main");
    repo.git(&["fetch", "--quiet", "origin"]);
    let c2 = repo.commit_file("a.txt", "two\n", "second");
    let (r, mut s) = open(&repo);

    let out = run(&r, &mut s, "tag.create", json!({ "name": "v2" }));
    assert_eq!(out["tag"], "refs/tags/v2");
    assert_eq!(out["annotated"], false);
    assert_eq!(
        repo.git(&["cat-file", "-t", "refs/tags/v2"]).trim(),
        "commit"
    );
    assert_eq!(repo.rev("v2"), c2);

    let out = run(
        &r,
        &mut s,
        "tag.create",
        json!({ "name": "v1", "target": "origin/main", "message": "#1 first release" }),
    );
    assert_eq!(out["annotated"], true);
    assert_eq!(repo.git(&["cat-file", "-t", "refs/tags/v1"]).trim(), "tag");
    assert_eq!(repo.rev("v1^{commit}"), c1);
    // `#1` is not taken for a comment.
    assert_eq!(
        repo.git(&["tag", "-l", "--format=%(contents:subject)", "v1"])
            .trim(),
        "#1 first release"
    );
    let both = tags(&repo);

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert!(!tags(&repo).contains("refs/tags/v1"));
    // The tag object (message, author) is kept even though its commit stays reachable.
    let backups = repo.git(&[
        "for-each-ref",
        "--format=%(objecttype)",
        "refs/project/backup/",
    ]);
    assert_eq!(backups.trim(), "tag", "{undo}");
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(tags(&repo), both, "the same tag object is back");
    run(&r, &mut s, "edit.undo", json!({}));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(tags(&repo), "");
}

#[test]
fn an_existing_tag_is_never_overwritten() {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    repo.tag("v1", "HEAD");
    repo.commit_file("a.txt", "two\n", "second");
    let (r, mut s) = open(&repo);
    for message in [Value::Null, json!("again")] {
        let err = r
            .run(
                "tag.create",
                &mut s,
                json!({ "name": "v1", "message": message }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }
    assert_eq!(repo.rev("v1"), c1);
    let err = r
        .run("tag.create", &mut s, json!({ "name": "--force" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert_eq!(tags(&repo), format!("refs/tags/v1 commit {c1}"));
}
