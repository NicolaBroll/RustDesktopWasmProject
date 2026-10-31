//! Integration tests of `remote.set_url` and `remote.rename` (S59) with the `CliBackend` on
//! temporary repos and a local remote (`git init --bare`): fetch and push URL, same URL, refused
//! URLs and names, rename with upstreams, `pushRemote`, `remote.pushDefault` and tags, operation
//! in progress, `git remote rename` stopped by a locked ref, undo/redo and undo conflicts.

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

/// The repo configuration and all the refs (symbolic ones too), to check that nothing changes.
fn state(repo: &Repo) -> (String, String) {
    let config = std::fs::read_to_string(repo.path().join(".git/config")).unwrap();
    let refs = repo.git(&[
        "for-each-ref",
        "--format=%(refname) %(objectname) %(symref)",
    ]);
    (config, refs)
}

fn can_undo(s: &Session) -> bool {
    s.journal().is_ok_and(|j| j.can_undo())
}

/// A bare remote with `main` and `feature`, and the repo under test that has it as `origin`, with
/// symbolic `origin/HEAD`, `main`/`feature` tracking it and a tag `v1`.
fn setup() -> (Repo, Repo) {
    let remote = Repo::bare();
    let work = Repo::new();
    work.commit_file("a.txt", "one\n", "first");
    work.add_remote("origin", &remote);
    work.push("origin", "HEAD", "main");
    work.branch("feature");
    work.push("origin", "feature", "feature");
    work.set_upstream("main", "origin");
    work.set_upstream("feature", "origin");
    work.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    work.tag("v1", "HEAD");
    (remote, work)
}

#[test]
fn set_fetch_url_then_undo_and_redo() {
    let (remote, work) = setup();
    let (r, mut s) = open(&work);
    let refs = state(&work).1;
    let out = run(
        &r,
        &mut s,
        "remote.set_url",
        json!({ "name": "origin", "url": "https://u:secret@example.com/r.git" }),
    );
    assert_eq!(out["changed"], true);
    assert_eq!(out["url"], "https://u:***@example.com/r.git");
    assert_eq!(out["push_url"], Value::Null);
    assert!(!out.to_string().contains("secret"), "{out}");
    assert_eq!(
        work.git(&["config", "remote.origin.url"]),
        "https://u:secret@example.com/r.git"
    );
    // Remote-tracking branches and upstreams stay.
    assert_eq!(state(&work).1, refs);
    assert_eq!(work.git(&["config", "branch.main.remote"]), "origin");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert!(!history.to_string().contains("secret"), "{history}");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        work.git(&["config", "remote.origin.url"]),
        remote.path_str()
    );
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        work.git(&["config", "remote.origin.url"]),
        "https://u:secret@example.com/r.git"
    );
}

#[test]
fn set_push_url_only_then_undo_removes_it() {
    let (remote, work) = setup();
    let (r, mut s) = open(&work);
    let out = run(
        &r,
        &mut s,
        "remote.set_url",
        json!({ "name": "origin", "url": "git@example.com:a/b.git", "push": true }),
    );
    assert_eq!(out["changed"], true);
    assert_eq!(out["url"], remote.path_str());
    assert_eq!(out["push_url"], "git@example.com:a/b.git");
    assert_eq!(
        work.git(&["config", "remote.origin.pushurl"]),
        "git@example.com:a/b.git"
    );
    assert_eq!(
        work.git(&["config", "remote.origin.url"]),
        remote.path_str()
    );
    run(&r, &mut s, "edit.undo", json!({}));
    work.git_fails(&["config", "remote.origin.pushurl"]);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        work.git(&["config", "remote.origin.pushurl"]),
        "git@example.com:a/b.git"
    );
}

#[test]
fn same_url_writes_nothing() {
    let (remote, work) = setup();
    let (r, mut s) = open(&work);
    let before = state(&work);
    let out = run(
        &r,
        &mut s,
        "remote.set_url",
        json!({ "name": "origin", "url": remote.path_str() }),
    );
    assert_eq!(out["changed"], false);
    assert_eq!(state(&work), before);
    assert!(!can_undo(&s));
}

#[test]
fn url_changed_from_the_terminal_blocks_the_undo() {
    let (_remote, work) = setup();
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "remote.set_url",
        json!({ "name": "origin", "url": "/srv/new.git" }),
    );
    work.git(&["remote", "set-url", "origin", "/srv/terminal.git"]);
    let before = state(&work);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert!(err.to_string().contains("/srv/terminal.git"), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn set_url_refusals_never_reach_git() {
    let (_remote, work) = setup();
    work.git(&["config", "--add", "remote.origin.pushurl", "/a"]);
    work.git(&["config", "--add", "remote.origin.pushurl", "/b"]);
    let (r, mut s) = open(&work);
    let before = state(&work);
    for (params, invalid) in [
        (json!({ "name": "missing", "url": "/x" }), false),
        (
            json!({ "name": "origin", "url": "/x", "push": true }),
            false,
        ),
        (
            json!({ "name": "origin", "url": "--upload-pack=touch /tmp/pwned" }),
            true,
        ),
        (json!({ "name": "origin", "url": "-oProxyCommand=x" }), true),
        (json!({ "name": "origin", "url": "ext::sh -c x" }), true),
        (
            json!({ "name": "origin", "url": "ssh://-oProxyCommand=x/r" }),
            true,
        ),
        (json!({ "name": "origin", "url": "" }), true),
    ] {
        let err = r.run("remote.set_url", &mut s, params.clone()).unwrap_err();
        if invalid {
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{params}: {err}"
            );
        } else {
            assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
        }
    }
    assert!(err_text_lists_known(&r, &mut s));
    assert_eq!(state(&work), before);
    assert!(!can_undo(&s));
}

fn err_text_lists_known(r: &Registry, s: &mut Session) -> bool {
    let err = r
        .run(
            "remote.set_url",
            s,
            json!({ "name": "missing", "url": "/x" }),
        )
        .unwrap_err();
    err.to_string().contains("`origin`")
}

#[test]
fn local_paths_and_file_urls_are_accepted() {
    let (_remote, work) = setup();
    let (r, mut s) = open(&work);
    for url in ["../other.git", "file:///srv/r.git", "/srv/with space/r.git"] {
        let out = run(
            &r,
            &mut s,
            "remote.set_url",
            json!({ "name": "origin", "url": url }),
        );
        assert_eq!(out["url"], url);
        assert_eq!(work.git(&["config", "remote.origin.url"]), url);
    }
}

#[test]
fn set_url_refuses_a_remote_defined_in_an_included_file() {
    let (_remote, work) = setup();
    let included = work.path().join("remotes.inc");
    std::fs::write(&included, "[remote \"origin\"]\n\tpushurl = /srv/inc.git\n").unwrap();
    work.git(&["config", "include.path", included.to_str().unwrap()]);
    let (r, mut s) = open(&work);
    let before = state(&work);
    let err = r
        .run(
            "remote.set_url",
            &mut s,
            json!({ "name": "origin", "url": "/x" }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");
    let err = r
        .run(
            "remote.rename",
            &mut s,
            json!({ "name": "origin", "new_name": "up" }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("outside"), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn set_url_with_a_locked_config_reports_git_and_records_nothing() {
    let (_remote, work) = setup();
    let (r, mut s) = open(&work);
    let lock = work.path().join(".git/config.lock");
    std::fs::write(&lock, "").unwrap();
    let err = r
        .run(
            "remote.set_url",
            &mut s,
            json!({ "name": "origin", "url": "https://u:secret@example.com/r.git" }),
        )
        .unwrap_err();
    std::fs::remove_file(&lock).unwrap();
    assert!(err.to_string().contains("lock"), "{err}");
    assert!(!err.to_string().contains("secret"), "{err}");
    assert!(!can_undo(&s));
}

#[test]
fn rename_with_upstreams_push_remote_and_tags_then_undo_and_redo() {
    let (_remote, work) = setup();
    work.git(&["config", "branch.main.pushRemote", "origin"]);
    work.git(&["config", "remote.pushDefault", "origin"]);
    let (r, mut s) = open(&work);
    let tags = work.git(&["for-each-ref", "refs/tags/"]);
    let main = work.rev("refs/remotes/origin/main");
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
            "refs/remotes/upstream/feature",
            "refs/remotes/upstream/main"
        ])
    );
    assert_eq!(out["upstreams"], json!(["feature", "main"]));
    assert_eq!(out["warnings"], json!([]), "{out}");
    assert_eq!(work.git(&["remote"]), "upstream");
    assert_eq!(work.rev("refs/remotes/upstream/main"), main);
    assert_eq!(
        work.git(&["symbolic-ref", "refs/remotes/upstream/HEAD"]),
        "refs/remotes/upstream/main"
    );
    assert_eq!(work.git(&["config", "branch.main.remote"]), "upstream");
    assert_eq!(work.git(&["config", "branch.feature.remote"]), "upstream");
    assert_eq!(work.git(&["config", "branch.main.pushRemote"]), "upstream");
    assert_eq!(work.git(&["config", "remote.pushDefault"]), "upstream");
    assert_eq!(
        work.git(&["config", "remote.upstream.fetch"]),
        "+refs/heads/*:refs/remotes/upstream/*"
    );
    assert_eq!(work.git(&["for-each-ref", "refs/tags/"]), tags);
    assert_eq!(work.git(&["for-each-ref", "refs/remotes/origin/"]), "");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(work.git(&["remote"]), "origin");
    assert_eq!(work.rev("refs/remotes/origin/main"), main);
    assert_eq!(work.git(&["config", "branch.main.remote"]), "origin");
    assert_eq!(work.git(&["config", "remote.pushDefault"]), "origin");
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(work.git(&["remote"]), "upstream");

    // The old name recreated from the terminal: the undo is refused, nothing changes.
    work.git(&["remote", "add", "origin", "/srv/fresh.git"]);
    let before = state(&work);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn rename_refusals_change_nothing() {
    let (remote, work) = setup();
    work.add_remote("fork", &remote);
    // Recent git refuses `git remote add` of a nested name: configured by hand.
    work.git(&["config", "remote.fork/a.url", remote.path_str()]);
    work.git(&["update-ref", "refs/remotes/old/main", "HEAD"]);
    // Settings left over from remotes removed by hand: renaming to those names would take them over.
    work.git(&["config", "branch.main.pushRemote", "gone"]);
    work.git(&["config", "remote.pushDefault", "lost"]);
    let (r, mut s) = open(&work);
    let before = state(&work);
    for (params, invalid) in [
        (json!({ "name": "origin", "new_name": "fork" }), false),
        (json!({ "name": "missing", "new_name": "x" }), false),
        (json!({ "name": "fork", "new_name": "spoon" }), false),
        (json!({ "name": "origin", "new_name": "fork/b" }), false),
        (json!({ "name": "origin", "new_name": "old" }), false),
        (json!({ "name": "origin", "new_name": "gone" }), false),
        (json!({ "name": "origin", "new_name": "lost" }), false),
        (json!({ "name": "origin", "new_name": "a..b" }), true),
        (json!({ "name": "origin", "new_name": "a b" }), true),
        (json!({ "name": "origin", "new_name": "-x" }), true),
        (
            json!({ "name": "origin", "new_name": "--upload-pack=x" }),
            true,
        ),
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
    assert_eq!(state(&work), before);
    assert!(!can_undo(&s));
}

#[test]
fn rename_during_an_operation_is_refused() {
    let (_remote, work) = setup();
    work.diverge("other", "a.txt", "ours\n", "theirs\n");
    work.conflict("other");
    let (r, mut s) = open(&work);
    let before = state(&work);
    let err = r
        .run(
            "remote.rename",
            &mut s,
            json!({ "name": "origin", "new_name": "up" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("merge"), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn rename_stopped_by_a_locked_ref_is_in_the_journal_and_undo_renames_back() {
    let (_remote, work) = setup();
    let lock = work.path().join(".git/refs/remotes/origin/main.lock");
    std::fs::write(&lock, "").unwrap();
    let (r, mut s) = open(&work);
    let err = r
        .run(
            "remote.rename",
            &mut s,
            json!({ "name": "origin", "new_name": "up" }),
        )
        .unwrap_err();
    std::fs::remove_file(&lock).unwrap();
    // git moved the configuration (and maybe more) before stopping: the journal must have it.
    if work.git(&["remote"]) == "origin" {
        // An older git that checks the refs first: nothing changed, nothing in the journal.
        assert!(!can_undo(&s), "{err}");
        return;
    }
    assert!(err.to_string().contains("edit.undo"), "{err}");
    assert!(can_undo(&s));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(work.git(&["remote"]), "origin");
    assert_eq!(
        work.git(&["config", "remote.origin.fetch"]),
        "+refs/heads/*:refs/remotes/origin/*"
    );
    assert_eq!(work.git(&["config", "branch.main.remote"]), "origin");
    assert!(
        !work
            .git(&["for-each-ref", "refs/remotes/origin/"])
            .is_empty()
    );
}
