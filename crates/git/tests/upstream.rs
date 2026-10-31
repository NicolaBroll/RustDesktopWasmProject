//! Integration tests of the upstream of local branches (S51) with the `CliBackend` on temporary
//! repos and a local bare remote: `branch.create` with `track` (also from a remote with `/` in
//! the name), `branch.set_upstream` (set, replace, remove, a branch named `-x`), undo/redo of
//! both, configuration changed from outside before the undo and a locked `.git/config`.

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

/// The repo configuration and all the refs, to check that nothing changes.
fn state(repo: &Repo) -> (String, String) {
    let config = std::fs::read_to_string(repo.path().join(".git/config")).unwrap();
    let refs = repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    (config, refs)
}

/// The local `branch.<name>.*` entries, `key value` per line (empty if none).
fn branch_config(repo: &Repo, name: &str) -> String {
    let out = repo.git_output(&[
        "config",
        "--local",
        "--get-regexp",
        &format!("^branch\\.{}\\.", name.replace('.', "\\.")),
    ]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The upstream of `branch` as git resolves it (`<branch>@{upstream}`), `None` without one.
fn git_upstream(repo: &Repo, branch: &str) -> Option<String> {
    let out = repo.git_output(&[
        "rev-parse",
        "--symbolic-full-name",
        &format!("{branch}@{{upstream}}"),
    ]);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A bare remote with `main`, `feature` and `topic`; the repo under test has it as `origin`,
/// everything fetched, local `main` and `feature` (no upstream), no local `topic`.
struct Setup {
    _remote: Repo,
    work: Repo,
}

fn setup() -> Setup {
    let remote = Repo::bare();
    let work = Repo::new();
    work.commit_file("a.txt", "one\n", "first");
    work.add_remote("origin", &remote);
    work.push("origin", "HEAD", "main");
    work.branch("feature");
    work.push("origin", "feature", "feature");
    work.commit_file("b.txt", "two\n", "second");
    work.push("origin", "HEAD", "topic");
    work.git(&["reset", "--quiet", "--hard", "HEAD~1"]);
    Setup {
        _remote: remote,
        work,
    }
}

#[test]
fn track_from_origin_then_undo_and_redo() {
    let Setup { _remote, work } = setup();
    let (r, mut s) = open(&work);
    let topic = work.rev("refs/remotes/origin/topic");
    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "topic", "start": "origin/topic", "track": true }),
    );
    assert_eq!(out["upstream"], "origin/topic");
    assert_eq!(out["target"], topic.to_string());
    assert_eq!(work.rev("refs/heads/topic"), topic);
    assert_eq!(
        branch_config(&work, "topic"),
        "branch.topic.remote origin\nbranch.topic.merge refs/heads/topic\n"
    );
    assert_eq!(
        git_upstream(&work, "topic").as_deref(),
        Some("refs/remotes/origin/topic")
    );
    // The command left HEAD and the worktree alone.
    assert_eq!(
        work.git(&["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/main"
    );

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["warnings"], json!([]), "{undo}");
    work.git_fails(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]);
    assert_eq!(branch_config(&work, "topic"), "");

    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(work.rev("refs/heads/topic"), topic);
    assert_eq!(
        git_upstream(&work, "topic").as_deref(),
        Some("refs/remotes/origin/topic")
    );
}

#[test]
fn track_from_a_remote_with_a_slash_in_the_name() {
    let Setup { _remote, work } = setup();
    work.add_remote("team/a", &_remote);
    work.git(&["fetch", "--quiet", "team/a"]);
    let (r, mut s) = open(&work);
    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "t", "start": "team/a/topic", "track": true }),
    );
    assert_eq!(out["upstream"], "team/a/topic");
    assert_eq!(
        branch_config(&work, "t"),
        "branch.t.remote team/a\nbranch.t.merge refs/heads/topic\n"
    );
    assert_eq!(
        git_upstream(&work, "t").as_deref(),
        Some("refs/remotes/team/a/topic")
    );

    // With a remote `team` too, `team/a/topic` could be `a/topic` of `team`: refused.
    // `git remote add` refuses it ("superset"), the configuration does not.
    work.git(&["config", "remote.team.url", _remote.path_str()]);
    work.git(&[
        "config",
        "remote.team.fetch",
        "+refs/heads/*:refs/remotes/team/*",
    ]);
    let before = state(&work);
    let err = r
        .run(
            "branch.create",
            &mut s,
            json!({ "name": "u", "start": "team/a/topic", "track": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err:?}");
    assert_eq!(state(&work), before);
}

#[test]
fn track_with_a_local_start_or_a_dash_name_is_refused() {
    let Setup { _remote, work } = setup();
    let (r, mut s) = open(&work);
    let before = state(&work);
    for params in [
        json!({ "name": "x", "start": "main", "track": true }),
        json!({ "name": "x", "start": "HEAD", "track": true }),
        json!({ "name": "x", "track": true }),
        json!({ "name": "-x", "start": "origin/topic", "track": true }),
        json!({ "name": "--force", "start": "origin/topic", "track": true }),
    ] {
        let err = r.run("branch.create", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err:?}"
        );
    }
    // A local branch `origin/topic` shadows the remote one for git: ambiguous.
    work.git(&["branch", "origin/topic", "main"]);
    let before_shadow = state(&work);
    let err = r
        .run(
            "branch.create",
            &mut s,
            json!({ "name": "x", "start": "origin/topic", "track": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err:?}");
    assert_eq!(state(&work), before_shadow);
    work.git(&["branch", "-D", "origin/topic"]);
    assert_eq!(state(&work).1, before.1);
}

#[test]
fn set_replace_and_unset_upstream_with_undo_and_redo() {
    let Setup { _remote, work } = setup();
    work.git(&["config", "branch.feature.description", "keep me"]);
    let (r, mut s) = open(&work);

    let out = run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "feature", "upstream": "origin/feature" }),
    );
    assert_eq!(out["previous"], Value::Null);
    assert_eq!(out["changed"], true);
    assert_eq!(
        git_upstream(&work, "feature").as_deref(),
        Some("refs/remotes/origin/feature")
    );

    let out = run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "feature", "upstream": "origin/topic" }),
    );
    assert_eq!(out["previous"], "origin/feature");
    assert_eq!(
        branch_config(&work, "feature"),
        "branch.feature.description keep me\nbranch.feature.remote origin\nbranch.feature.merge refs/heads/topic\n"
    );

    let out = run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "feature", "upstream": null }),
    );
    assert_eq!(out["previous"], "origin/topic");
    assert_eq!(git_upstream(&work, "feature"), None);
    assert_eq!(
        branch_config(&work, "feature"),
        "branch.feature.description keep me\n",
        "only remote and merge are removed"
    );

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        git_upstream(&work, "feature").as_deref(),
        Some("refs/remotes/origin/topic")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        git_upstream(&work, "feature").as_deref(),
        Some("refs/remotes/origin/feature")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(git_upstream(&work, "feature"), None);
    assert_eq!(
        branch_config(&work, "feature"),
        "branch.feature.description keep me\n"
    );
    run(&r, &mut s, "edit.redo", json!({}));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        git_upstream(&work, "feature").as_deref(),
        Some("refs/remotes/origin/topic")
    );
}

#[test]
fn set_upstream_of_a_branch_named_dash_x() {
    let Setup { _remote, work } = setup();
    let main = work.rev("main");
    work.git(&["update-ref", "refs/heads/-x", &main.to_string()]);
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "-x", "upstream": "origin/main" }),
    );
    assert_eq!(
        branch_config(&work, "-x"),
        "branch.-x.remote origin\nbranch.-x.merge refs/heads/main\n"
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(branch_config(&work, "-x"), "");
}

#[test]
fn undo_refuses_a_configuration_changed_from_outside() {
    let Setup { _remote, work } = setup();
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "branch.set_upstream",
        json!({ "name": "feature", "upstream": "origin/feature" }),
    );
    work.git(&["config", "branch.feature.merge", "refs/heads/main"]);
    let before = state(&work);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err:?}");
    assert!(err.to_string().contains("origin/main"), "{err}");
    assert_eq!(state(&work), before);
}

#[test]
fn undo_of_a_tracking_create_leaves_a_changed_upstream() {
    let Setup { _remote, work } = setup();
    let (r, mut s) = open(&work);
    run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "topic", "start": "origin/topic", "track": true }),
    );
    work.git(&["config", "branch.topic.merge", "refs/heads/main"]);
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    work.git_fails(&["rev-parse", "--verify", "--quiet", "refs/heads/topic"]);
    assert!(
        undo["warnings"].to_string().contains("left as it is"),
        "{undo}"
    );
    assert_eq!(
        branch_config(&work, "topic"),
        "branch.topic.remote origin\nbranch.topic.merge refs/heads/main\n"
    );
}

#[test]
fn a_locked_config_leaves_nothing_half_done() {
    let Setup { _remote, work } = setup();
    let (r, mut s) = open(&work);
    let lock = work.path().join(".git/config.lock");
    std::fs::write(&lock, "").unwrap();
    let before = state(&work);
    let err = r
        .run(
            "branch.create",
            &mut s,
            json!({ "name": "topic", "start": "origin/topic", "track": true }),
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("deleted again"), "{text}");
    assert_eq!(state(&work), before, "the new branch was removed");
    let err = r
        .run(
            "branch.set_upstream",
            &mut s,
            json!({ "name": "feature", "upstream": "origin/feature" }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("lock"), "{err}");
    assert_eq!(state(&work), before);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"], json!([]));
    std::fs::remove_file(&lock).unwrap();
}
