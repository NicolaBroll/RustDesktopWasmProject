//! Integration tests of the squash merge (`merge.start` with `squash`, S58) with the
//! `CliBackend`: diverged branch, branch ahead (no fast-forward: one commit), default and
//! custom message, up to date, local changes and files in the way refused, conflicts → in
//! progress → continue / abort with backup, HEAD moved from the terminal while in progress, a
//! hook that rejects the commit, `merge.ff=only` / `mergeOptions` / `merge.autoStash` in the
//! configuration, a `post-merge` hook that moves HEAD, undo and redo.

mod common;

use common::{Repo, repo_with_divergent_branches};
use project_domain::Oid;
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

fn read(repo: &Repo, file: &str) -> Option<String> {
    std::fs::read_to_string(repo.path().join(file)).ok()
}

fn clean(repo: &Repo) -> bool {
    repo.git(&["status", "--porcelain"]).is_empty()
}

fn parents(repo: &Repo, rev: &str) -> Vec<Oid> {
    repo.git(&["rev-list", "--parents", "-n", "1", rev])
        .split_whitespace()
        .skip(1)
        .map(|h| h.parse().unwrap())
        .collect()
}

fn message(repo: &Repo) -> String {
    repo.git(&["log", "-1", "--format=%B", "HEAD"])
}

fn record(repo: &Repo) -> bool {
    repo.path().join(".git/project-squash.json").exists()
}

fn squash_msg(repo: &Repo) -> bool {
    repo.path().join(".git/SQUASH_MSG").exists()
}

/// `main` on `c1` (`a.txt`), `ahead` two commits later (`b.txt`, `c.txt`).
fn ahead_branch() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first");
    repo.branch("ahead");
    repo.checkout("ahead");
    repo.commit_file("b.txt", "b\n", "second");
    let c3 = repo.commit_file("c.txt", "c\n", "third");
    repo.checkout("main");
    (repo, c1, c3)
}

#[cfg(unix)]
fn hook(repo: &Repo, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = repo.path().join(".git/hooks").join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn squash_of_a_diverged_branch_undo_and_redo() {
    let (repo, _, c3) = ahead_branch();
    let m1 = repo.commit_file("d.txt", "d\n", "on main");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "squash": true }),
    );
    assert_eq!(out["strategy"], "squash");
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["before"], m1.to_string());
    assert_eq!(parents(&repo, "HEAD"), [m1]);
    assert_eq!(
        message(&repo),
        "Squashed branch 'ahead'\n\n* second\n* third"
    );
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("c\n"));
    assert!(clean(&repo));
    assert!(!record(&repo) && !squash_msg(&repo));
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), m1);
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "main"]);
    assert!(reflog.starts_with("commit"), "{reflog}");
    // The branch is not recorded as merged.
    repo.git_fails(&["merge-base", "--is-ancestor", &c3.to_string(), "main"]);
    let squash = repo.rev("main");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), m1);
    assert!(read(&repo, "c.txt").is_none());
    assert!(clean(&repo));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), squash);
    assert!(clean(&repo));
}

#[test]
fn squash_of_a_branch_ahead_custom_message_and_up_to_date() {
    let (repo, c1, c3) = ahead_branch();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "squash": true, "message": "Everything\n\nin one" }),
    );
    assert_eq!(out["strategy"], "squash");
    // No fast-forward: one new commit on top of `c1`.
    assert_ne!(repo.rev("main"), c3);
    assert_eq!(parents(&repo, "HEAD"), [c1]);
    assert_eq!(message(&repo), "Everything\n\nin one");
    assert!(clean(&repo));

    // Contained already: nothing written.
    let before = repo.rev("main");
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": c1.to_string(), "squash": true }),
    );
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(repo.rev("main"), before);
    // `squash` with `no_ff` or `ff_only`: invalid.
    for extra in ["no_ff", "ff_only"] {
        let err = r
            .run(
                "merge.start",
                &mut s,
                json!({ "rev": "ahead", "squash": true, extra: true }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }
}

#[test]
fn squash_with_nothing_new_writes_nothing() {
    // `ahead` has a commit whose change `main` already has (same content).
    let (repo, _, _) = ahead_branch();
    repo.write("b.txt", "b\n");
    repo.write("c.txt", "c\n");
    repo.stage(&["b.txt", "c.txt"]);
    let m1 = repo.commit("same content");
    let backups = repo.git(&["for-each-ref", "refs/project/backup/"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "squash": true }),
    );
    assert_eq!(out["after"], m1.to_string());
    assert_eq!(out["can_undo"], false);
    assert!(!out["warnings"].as_array().unwrap().is_empty(), "{out}");
    assert_eq!(repo.rev("main"), m1);
    assert!(clean(&repo));
    assert!(!record(&repo) && !squash_msg(&repo));
    assert_eq!(repo.git(&["for-each-ref", "refs/project/backup/"]), backups);
}

#[test]
fn squash_refuses_local_changes_and_files_in_the_way() {
    let (repo, c1, _) = ahead_branch();
    let (r, mut s) = open(&repo);
    // A change to a file the squash does not touch: refused all the same.
    repo.write("a.txt", "changed\n");
    let err = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "ahead", "squash": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    repo.git(&["checkout", "--", "a.txt"]);
    // An untracked file where an incoming one goes.
    repo.write("b.txt", "mine\n");
    let err = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "ahead", "squash": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mine\n"));
    assert_eq!(repo.rev("main"), c1);
    assert!(!record(&repo) && !squash_msg(&repo));
}

#[test]
fn conflict_stays_in_progress_then_continue_and_undo() {
    let (repo, theirs) = repo_with_divergent_branches();
    let ours = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "other", "squash": true }),
    );
    assert_eq!(out["strategy"], "squash");
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], ours.to_string());
    assert_eq!(out["status"]["operation"], "Squash");
    assert_eq!(out["can_undo"], false);
    assert!(record(&repo) && squash_msg(&repo));
    assert!(!repo.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), ours);

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Squash");
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["incoming"][0]["label"], "other");
    assert_eq!(op["message"], "Squashed branch 'other'\n\n* a.txt on other");
    assert_eq!(op["ready"], false);
    assert_eq!(op["continue"], "merge.continue");
    assert_eq!(op["abort"], "merge.abort");

    // During the squash the other commands refuse, as during a merge.
    for (id, params) in [
        ("edit.undo", json!({})),
        ("merge.start", json!({ "rev": "other" })),
        ("branch.checkout", json!({ "name": "other" })),
        ("stash.push", json!({})),
        ("merge.continue", json!({})),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }

    repo.write("a.txt", "resolved\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);
    let out = run(&r, &mut s, "merge.continue", json!({}));
    assert_eq!(
        out["message"],
        "Squashed branch 'other'\n\n* a.txt on other"
    );
    assert_eq!(parents(&repo, "HEAD"), [ours]);
    assert_eq!(
        message(&repo),
        "Squashed branch 'other'\n\n* a.txt on other"
    );
    assert_eq!(out["status"]["operation"], Value::Null);
    assert!(clean(&repo));
    assert!(!record(&repo) && !squash_msg(&repo));
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .contains("squash into one commit"),
        "{out}"
    );
    let squash = repo.rev("HEAD");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), ours);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("ours\n"));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), squash);
}

#[test]
fn abort_needs_confirm_and_keeps_a_backup() {
    let (repo, _) = repo_with_divergent_branches();
    let ours = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "other", "squash": true }),
    );
    repo.write("a.txt", "half resolved\n");
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(record(&repo));
    let out = run(&r, &mut s, "merge.abort", json!({ "confirm": true }));
    let backup = out["backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/merge-abort-"));
    assert_eq!(
        repo.git(&["show", &format!("{backup}:a.txt")]),
        "half resolved"
    );
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(repo.rev("main"), ours);
    assert!(clean(&repo));
    assert!(!record(&repo) && !squash_msg(&repo));
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], Value::Null);
}

#[test]
fn head_moved_from_the_terminal_ends_the_squash() {
    let (repo, _) = repo_with_divergent_branches();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "other", "squash": true }),
    );
    // Concluded from the terminal: the record is stale, ignored and removed.
    repo.write("a.txt", "resolved\n");
    repo.stage(&["a.txt"]);
    repo.git(&["commit", "-q", "-m", "by hand"]);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], Value::Null);
    assert!(!record(&repo));
    let err = r.run("merge.continue", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // Aborted from the terminal (`reset --merge` removes `SQUASH_MSG`).
    run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "other", "squash": true }),
    );
    assert!(record(&repo));
    repo.git(&["reset", "-q", "--merge"]);
    let status = run(&r, &mut s, "repo.status", json!({}));
    assert_eq!(status["operation"], Value::Null);
    assert!(!record(&repo));
}

#[cfg(unix)]
#[test]
fn hook_that_rejects_the_commit_leaves_the_squash_in_progress() {
    let (repo, c1, _) = ahead_branch();
    hook(&repo, "pre-commit", "echo 'no, thanks' >&2; exit 1");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "squash": true }),
    );
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!([]));
    assert!(
        out["message"].as_str().unwrap().contains("no, thanks"),
        "{out}"
    );
    assert_eq!(out["status"]["operation"], "Squash");
    assert_eq!(repo.rev("main"), c1);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);
    std::fs::remove_file(repo.path().join(".git/hooks/pre-commit")).unwrap();
    let out = run(&r, &mut s, "merge.continue", json!({ "message": "Retry" }));
    assert_eq!(parents(&repo, "HEAD"), [c1]);
    assert_eq!(message(&repo), "Retry");
    assert_eq!(out["can_undo"], true);
    assert!(clean(&repo));
    assert!(!record(&repo));
}

#[test]
fn configuration_does_not_change_the_squash() {
    let (repo, c1, _) = ahead_branch();
    let m1 = repo.commit_file("d.txt", "d\n", "on main");
    repo.git(&["config", "merge.ff", "only"]);
    repo.git(&[
        "config",
        "branch.main.mergeOptions",
        "--commit --no-ff --no-squash",
    ]);
    repo.git(&["config", "merge.autoStash", "true"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "squash": true }),
    );
    assert_eq!(out["strategy"], "squash");
    assert_eq!(out["in_progress"], false);
    assert_eq!(parents(&repo, "HEAD"), [m1]);
    assert_ne!(repo.rev("HEAD"), c1);
    assert!(clean(&repo));
    assert_eq!(out["can_undo"], true);
}

/// A `post-merge` hook (git runs it after `--squash` too) that moves HEAD: an error that names
/// the backup, no commit made by the app.
#[cfg(unix)]
#[test]
fn head_moved_by_a_hook_during_the_squash_is_an_error() {
    let (repo, c1, _) = ahead_branch();
    hook(
        &repo,
        "post-merge",
        "git commit --quiet --allow-empty -m other",
    );
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "ahead", "squash": true }),
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("refs/project/backup/merge-"), "{text}");
    assert_eq!(parents(&repo, "main"), [c1]);
    assert_eq!(repo.git(&["log", "-1", "--format=%s", "main"]), "other");
    let backup = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/merge-*",
    ]);
    assert_eq!(repo.rev(backup.trim()), c1);
}
