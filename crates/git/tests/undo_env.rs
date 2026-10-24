//! `edit.undo` / `edit.redo` with a hostile inherited environment (e.g. the app started from a
//! git hook or from a terminal with git variables set): the refs that move are those of the
//! open repo, and the configuration injected by the environment does not apply. A separate
//! file because it changes the process environment, which the tests of a file share.

mod common;

use common::Repo;
use project_engine::{Registry, Session};
use serde_json::json;
use std::sync::Arc;

#[test]
fn inherited_environment_does_not_move_other_refs() {
    let repo = Repo::new();
    let before = repo.commit_file("a.txt", "a\n", "first");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let other = Repo::new();
    let other_head = other.commit_file("x.txt", "x\n", "other repo");
    let other_refs = other.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);

    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    r.run("commit.create", &mut s, json!({ "message": "b" }))
        .unwrap();
    let commit = repo.rev("HEAD");

    let other_git = other.path().join(".git");
    let vars: [(&str, &std::ffi::OsStr); 5] = [
        ("GIT_DIR", other_git.as_os_str()),
        ("GIT_COMMON_DIR", other_git.as_os_str()),
        // Configuration injected by the environment: hooks elsewhere, "bare" repo.
        (
            "GIT_CONFIG_PARAMETERS",
            "'core.hooksPath=/nonexistent' 'core.logAllRefUpdates=false'".as_ref(),
        ),
        ("GIT_CONFIG_COUNT", "1".as_ref()),
        ("GIT_CONFIG_KEY_0", "core.bare".as_ref()),
    ];
    // SAFETY: it is the only test of this executable, no other thread reads the environment.
    unsafe {
        for (key, value) in &vars {
            std::env::set_var(key, value);
        }
        std::env::set_var("GIT_CONFIG_VALUE_0", "true");
    }
    let undo = r.run("edit.undo", &mut s, json!({}));
    let redo = r.run("edit.redo", &mut s, json!({}));
    unsafe {
        for (key, _) in &vars {
            std::env::remove_var(key);
        }
        std::env::remove_var("GIT_CONFIG_VALUE_0");
    }
    let undo = undo.unwrap();
    redo.unwrap();
    // (The test's git commands inherit the environment: HEAD after the undo is read from the result.)
    assert_eq!(
        undo["status"]["head"]["Branch"]["target"],
        before.to_string()
    );
    assert_eq!(repo.rev("refs/heads/main"), commit);
    let backup = undo["backups"][0].as_str().unwrap();
    assert_eq!(repo.rev(backup), commit);
    // The other repo was not touched.
    assert_eq!(other.rev("HEAD"), other_head);
    assert_eq!(
        other.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        other_refs
    );
}
