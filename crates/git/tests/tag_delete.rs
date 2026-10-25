//! Integration tests of `tag.delete` with the `CliBackend` on real repos: always confirmed, the value
//! (tag object or commit) saved in a backup before deleting, undo that puts back the same tag
//! object and redo, conflicts with tags changed from outside, names that look like options, a backup
//! that cannot be made.

mod common;

use common::Repo;
use project_engine::{EngineError, GitBackend, Registry, Session};
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

fn all_refs(repo: &Repo) -> String {
    repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"])
}

fn value(repo: &Repo, name: &str) -> Option<String> {
    let out = repo.git_output(&[
        "rev-parse",
        "--quiet",
        "--verify",
        "--end-of-options",
        &format!("refs/tags/{name}"),
    ]);
    out.status
        .success()
        .then(|| String::from_utf8(out.stdout).unwrap().trim().to_owned())
}

#[test]
fn annotated_tag_with_confirmation_backup_undo_and_redo() {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    let object = repo.annotated_tag("v1", "HEAD", "Version 1\n\nnotes");
    repo.commit_file("a.txt", "two\n", "second");
    let (r, mut s) = open(&repo);

    let before = all_refs(&repo);
    let err = r
        .run("tag.delete", &mut s, json!({ "name": "v1" }))
        .unwrap_err();
    let EngineError::ConfirmRequired { reason, .. } = &err else {
        panic!("{err}");
    };
    assert!(reason.contains("annotated"), "{reason}");
    assert!(reason.contains(&object.to_string()), "{reason}");
    assert_eq!(all_refs(&repo), before, "nothing changes without confirm");

    let out = run(
        &r,
        &mut s,
        "tag.delete",
        json!({ "name": "v1", "confirm": true }),
    );
    assert_eq!(out["tag"], "refs/tags/v1");
    assert_eq!(out["annotated"], true);
    assert_eq!(out["target"], c1.to_string());
    assert_eq!(out["tag_object"], object.to_string());
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/tag-delete-"),
        "{backup}"
    );
    assert_eq!(value(&repo, "v1"), None);
    // The backup keeps the tag object itself: message and author.
    assert_eq!(repo.rev(&backup), object);
    assert_eq!(repo.git(&["cat-file", "-t", &backup]).trim(), "tag");
    assert!(repo.git(&["cat-file", "tag", &backup]).contains("notes"));

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(value(&repo, "v1"), Some(object.to_string()));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(value(&repo, "v1"), None);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(value(&repo, "v1"), Some(object.to_string()));
    assert_eq!(repo.git(&["cat-file", "-t", "refs/tags/v1"]).trim(), "tag");
}

#[test]
fn lightweight_tags_with_names_that_look_like_options() {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    for name in ["-x", "--force", "-d"] {
        repo.git(&["update-ref", &format!("refs/tags/{name}"), &c1.to_string()]);
    }
    repo.tag("keep", "HEAD");
    let (r, mut s) = open(&repo);
    for name in ["-x", "--force", "-d"] {
        let out = run(
            &r,
            &mut s,
            "tag.delete",
            json!({ "name": name, "confirm": true }),
        );
        assert_eq!(out["annotated"], false, "{name}");
        assert_eq!(out["tag_object"], Value::Null);
        assert_eq!(value(&repo, name), None, "{name}");
        let backup = out["backup"].as_str().unwrap();
        assert_eq!(repo.rev(backup), c1);
    }
    assert_eq!(value(&repo, "keep"), Some(c1.to_string()));
    for name in ["-d", "--force", "-x"] {
        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(value(&repo, name), Some(c1.to_string()), "{name}");
    }
}

#[test]
fn undo_and_redo_refuse_a_tag_changed_from_outside() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\n", "first");
    let object = repo.annotated_tag("v1", "HEAD", "Version 1");
    let c2 = repo.commit_file("a.txt", "two\n", "second");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "tag.delete",
        json!({ "name": "v1", "confirm": true }),
    );
    // Someone creates `v1` again elsewhere: the undo does not overwrite it.
    repo.tag("v1", "HEAD");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(value(&repo, "v1"), Some(c2.to_string()));

    // Put back by hand as it was: undo works; then moved again: the redo does not delete it.
    repo.git(&["update-ref", "refs/tags/v1", &object.to_string()]);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    repo.git(&["update-ref", "-d", "refs/tags/v1"]);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(value(&repo, "v1"), Some(object.to_string()));
    repo.git(&["tag", "-f", "v1", "HEAD"]);
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(value(&repo, "v1"), Some(c2.to_string()));
}

#[test]
fn the_backend_deletes_only_the_expected_value() {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    let c2 = repo.commit_file("a.txt", "two\n", "second");
    repo.tag("v1", &c2.to_string());
    let before = all_refs(&repo);
    // Moved between the read and the write: refused, no backup left behind.
    let err = repo.open().delete_tag("v1", &c1).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = repo.open().delete_tag("gone", &c1).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(all_refs(&repo), before);
}

#[test]
fn nothing_is_deleted_if_the_backup_fails() {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    repo.tag("v1", "HEAD");
    // A ref `refs/project/backup` blocks every `refs/project/backup/...` (file/directory conflict).
    repo.git(&["update-ref", "refs/project/backup", &c1.to_string()]);
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "tag.delete",
            &mut s,
            json!({ "name": "v1", "confirm": true }),
        )
        .unwrap_err();
    assert!(!matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert_eq!(value(&repo, "v1"), Some(c1.to_string()));
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_undo"], false);
}

#[test]
fn refusals_before_reaching_git() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\n", "first");
    repo.tag("v1", "HEAD");
    let (r, mut s) = open(&repo);
    let before = all_refs(&repo);
    let err = r
        .run(
            "tag.delete",
            &mut s,
            json!({ "name": "nothing", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    for name in ["", "refs/tags/v1", "a b", "v1..x"] {
        let err = r
            .run(
                "tag.delete",
                &mut s,
                json!({ "name": name, "confirm": true }),
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{name}: {err}"
        );
    }
    assert_eq!(all_refs(&repo), before);
}
