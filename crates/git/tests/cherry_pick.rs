//! Integration tests of `commit.cherry_pick`, `commit.revert`, `cherry_pick.continue`,
//! `cherry_pick.abort`, `revert.continue`, `revert.abort` (S31) with the `CliBackend`: success with
//! undo and redo, preserved author, revert message, already applied (nothing stays in progress),
//! conflict that stays in progress and ends after `conflict.resolve`/`index.stage`, abort with
//! and without confirmation and its backup, merge commit with and without `mainline`, local
//! changes, untracked or ignored files in place of the incoming ones, detached HEAD, failing
//! signature, multi-commit sequence started from the terminal.

mod common;

use common::Repo;
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

fn backups(repo: &Repo, kind: &str) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        &format!("refs/project/backup/{kind}-*"),
    ]);
    out.lines().map(str::to_owned).collect()
}

fn clean(repo: &Repo) -> bool {
    repo.git(&["status", "--porcelain"]).is_empty()
}

fn has(repo: &Repo, file: &str) -> bool {
    repo.path().join(".git").join(file).exists()
}

fn parents(repo: &Repo, rev: &str) -> Vec<Oid> {
    repo.git(&["rev-list", "--parents", "-n", "1", rev])
        .split_whitespace()
        .skip(1)
        .map(|h| h.parse().unwrap())
        .collect()
}

/// A commit of `file` authored by Cleo (the test committer is Bob).
fn commit_by_cleo(repo: &Repo, file: &str, content: &str, message: &str) -> Oid {
    repo.write(file, content);
    repo.stage(&[file]);
    repo.git(&[
        "commit",
        "--quiet",
        "--author=Cleo <cleo@example.com>",
        "-m",
        message,
    ]);
    repo.rev("HEAD")
}

/// `main`: `base` (`a.txt` = "base") and then `c.txt`; `other` from `base`, a commit by Cleo
/// that adds `b.txt` (with a body in the message): it applies without conflicts.
fn picking_repo() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("other");
    repo.checkout("other");
    let theirs = commit_by_cleo(&repo, "b.txt", "b\n", "adds b\n\nwith a body");
    repo.checkout("main");
    let main = repo.commit_file("c.txt", "c\n", "adds c");
    (repo, main, theirs)
}

/// `main` and `other` change `a.txt` differently from `base`; the commit of `other` is by Cleo.
fn conflicting_repo() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("other");
    repo.checkout("other");
    let theirs = commit_by_cleo(&repo, "a.txt", "theirs\n", "a.txt on other");
    repo.checkout("main");
    let main = repo.commit_file("a.txt", "ours\n", "a.txt on main");
    (repo, main, theirs)
}

#[test]
fn cherry_pick_keeps_the_author_undo_and_redo() {
    let (repo, main, theirs) = picking_repo();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["commit"], theirs.to_string());
    assert_eq!(out["before"], main.to_string());
    let picked = repo.rev("main");
    assert_eq!(out["after"], picked.to_string());
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>|%cn|%B", "main"]),
        "Cleo <cleo@example.com>|Bob|adds b\n\nwith a body"
    );
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("b\n"));
    assert!(clean(&repo));
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), main);
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "main"]);
    assert!(
        reflog.starts_with("project: commit.cherry_pick"),
        "{reflog}"
    );
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Cherry-pick of other onto `main`"),
        "{}",
        out["next_undo"]["text"]
    );

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    assert!(read(&repo, "b.txt").is_none());
    assert!(clean(&repo));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), picked);
    assert!(clean(&repo));

    // Already applied: NotAllowed, nothing in progress, no `MERGE_MSG`, the backup removed.
    let before = backups(&repo, "cherry-pick").len();
    let err = r
        .run(
            "commit.cherry_pick",
            &mut s,
            json!({ "rev": theirs.to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("would change nothing"), "{err}");
    assert_eq!(repo.rev("main"), picked);
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert!(!has(&repo, "MERGE_MSG"));
    assert!(clean(&repo));
    assert_eq!(backups(&repo, "cherry-pick").len(), before);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], Value::Null);
}

#[test]
fn revert_message_and_already_reverted() {
    let (repo, main, _) = picking_repo();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD" }));
    assert_eq!(out["in_progress"], false);
    let reverted = repo.rev("main");
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an|%B", "main"]),
        format!("Ada|Revert \"adds c\"\n\nThis reverts commit {main}.")
    );
    assert!(read(&repo, "c.txt").is_none());
    assert!(clean(&repo));
    assert!(!has(&repo, "REVERT_HEAD"));
    assert_eq!(backups(&repo, "revert").len(), 1);
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "main"]);
    assert!(reflog.starts_with("project: commit.revert"), "{reflog}");

    // `revert.reference` does not change the message.
    repo.git(&["config", "revert.reference", "true"]);
    // Already reverted: nothing changes, nothing in progress.
    let err = r
        .run("commit.revert", &mut s, json!({ "rev": main.to_string() }))
        .unwrap_err();
    assert!(err.to_string().contains("would change nothing"), "{err}");
    assert_eq!(repo.rev("main"), reverted);
    assert!(!has(&repo, "REVERT_HEAD"));
    assert!(!has(&repo, "MERGE_MSG"));
    assert_eq!(backups(&repo, "revert").len(), 1);

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("c\n"));
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", "main"]),
        "Revert \"adds c\""
    );
}

#[test]
fn conflict_stays_in_progress_then_resolve_and_continue() {
    let (repo, main, theirs) = conflicting_repo();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], main.to_string());
    assert_eq!(out["can_undo"], false);
    assert!(out["message"].as_str().unwrap().contains("a.txt"));
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    assert!(!has(&repo, "sequencer"));
    assert_eq!(repo.rev("main"), main);
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert_eq!(repo.rev(&backup), main);

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["message"], "a.txt on other");
    assert_eq!(op["continue"], "cherry_pick.continue");
    assert_eq!(op["abort"], "cherry_pick.abort");
    assert_eq!(op["ready"], false);

    for (id, params) in [
        ("cherry_pick.continue", json!({})),
        ("merge.continue", json!({})),
        ("revert.abort", json!({ "confirm": true })),
        ("commit.revert", json!({ "rev": "other" })),
        ("edit.undo", json!({})),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }
    assert!(has(&repo, "CHERRY_PICK_HEAD"));

    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "merged\n" }),
    );
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);
    let out = run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(out["message"], "a.txt on other");
    let done = repo.rev("main");
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an|%cn|%B", "main"]),
        "Cleo|Bob|a.txt on other"
    );
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("merged\n"));
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert!(clean(&repo));
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Cherry-pick of other onto `main`")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("ours\n"));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), done);
}

#[test]
fn revert_conflict_index_stage_and_continue() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\n", "one");
    let x = repo.commit_file("a.txt", "two\n", "two");
    let last = repo.commit_file("a.txt", "three\n", "three");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD~1" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert!(has(&repo, "REVERT_HEAD"));
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Revert");
    assert_eq!(op["continue"], "revert.continue");
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("not a cherry-pick"), "{err}");

    repo.write("a.txt", "one and three\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let out = run(
        &r,
        &mut s,
        "revert.continue",
        json!({ "message": "Revert of two, by hand" }),
    );
    assert_eq!(out["message"], "Revert of two, by hand");
    assert_eq!(parents(&repo, "main"), [last]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an|%B", "main"]),
        "Ada|Revert of two, by hand"
    );
    assert!(!has(&repo, "REVERT_HEAD"));
    assert!(clean(&repo));
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!("Revert of {} onto `main`", &x.to_string()[..7]))
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), last);
    assert!(clean(&repo));
}

#[test]
fn abort_needs_confirm_and_saves_a_backup() {
    let (repo, main, _) = conflicting_repo();
    let (r, mut s) = open(&repo);
    let err = r
        .run("cherry_pick.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(
        err.to_string().contains("a cherry-pick is not in progress"),
        "{err}"
    );

    run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    let err = r.run("cherry_pick.abort", &mut s, json!({})).unwrap_err();
    match &err {
        EngineError::ConfirmRequired { reason, .. } => {
            assert!(reason.contains("`a.txt`"), "{reason}");
            assert!(reason.contains("cherry-pick-abort"), "{reason}");
        }
        other => panic!("{other}"),
    }
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    // A half-done resolution, which the abort would delete: it is in the backup.
    repo.write("a.txt", "half done\n");
    let out = run(&r, &mut s, "cherry_pick.abort", json!({ "confirm": true }));
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/cherry-pick-abort-"),
        "{backup}"
    );
    assert_eq!(out["files"], json!(["a.txt"]));
    assert_eq!(out["can_undo"], false);
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert_eq!(repo.rev("main"), main);
    assert!(clean(&repo));
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("ours\n"));
    assert_eq!(repo.git(&["show", &format!("{backup}:a.txt")]), "half done");
    assert_eq!(parents(&repo, &backup).len(), 2);
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "HEAD"]);
    assert!(!reflog.is_empty());

    // A stopped revert is aborted the same way.
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD" }));
    assert_eq!(out["in_progress"], false);
    run(&r, &mut s, "edit.undo", json!({}));
    repo.commit_file("a.txt", "again\n", "again");
    let out = run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": main.to_string() }),
    );
    assert_eq!(out["in_progress"], true);
    let out = run(&r, &mut s, "revert.abort", json!({ "confirm": true }));
    assert!(
        out["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/revert-abort-")
    );
    assert!(!has(&repo, "REVERT_HEAD"));
    assert!(clean(&repo));
}

#[test]
fn merge_commit_with_and_without_mainline() {
    let repo = Repo::new();
    let base = repo.commit_file("a.txt", "base\n", "base");
    repo.branch("feat");
    repo.checkout("feat");
    repo.commit_file("f.txt", "f\n", "adds f");
    repo.checkout("main");
    let m1 = repo.commit_file("m.txt", "m\n", "adds m");
    let merge = repo.merge("feat");
    // A branch from `base` where to apply the merge.
    repo.git(&["checkout", "--quiet", "-b", "dest", &base.to_string()]);
    let dest = repo.commit_file("d.txt", "d\n", "adds d");
    let (r, mut s) = open(&repo);
    let id = merge.to_string();
    for params in [
        json!({ "rev": id }),
        json!({ "rev": id, "mainline": 0 }),
        json!({ "rev": id, "mainline": 3 }),
        json!({ "rev": "HEAD", "mainline": 1 }),
    ] {
        let err = r
            .run("commit.cherry_pick", &mut s, params.clone())
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    let err = r
        .run("commit.revert", &mut s, json!({ "rev": id }))
        .unwrap_err();
    assert!(err.to_string().contains("mainline"), "{err}");
    assert!(backups(&repo, "cherry-pick").is_empty());

    // Relative to the first parent: the changes of `feat` (`f.txt`).
    let out = run(
        &r,
        &mut s,
        "commit.cherry_pick",
        json!({ "rev": id, "mainline": 1 }),
    );
    assert_eq!(out["mainline"], 1);
    assert_eq!(parents(&repo, "dest"), [dest]);
    assert_eq!(read(&repo, "f.txt").as_deref(), Some("f\n"));
    assert!(read(&repo, "m.txt").is_none());
    assert!(clean(&repo));

    // The revert of the merge on `main`, relative to the first parent: removes `f.txt`.
    repo.checkout("main");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": id, "mainline": 1 }),
    );
    assert!(read(&repo, "f.txt").is_none());
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "main"]),
        format!(
            "Revert \"Merge branch 'feat'\"\n\nThis reverts commit {merge}, reversing\nchanges \
             made to {m1}."
        )
    );
}

#[test]
fn local_changes_and_files_in_the_way_are_refused() {
    let (repo, main, _) = picking_repo();
    let (r, mut s) = open(&repo);
    // A change to a file the commit does not touch, unstaged and then staged.
    repo.write("a.txt", "changed\n");
    for id in ["commit.cherry_pick", "commit.revert"] {
        let rev = if id == "commit.revert" {
            "HEAD"
        } else {
            "other"
        };
        let err = r.run(id, &mut s, json!({ "rev": rev })).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains("`a.txt`"), "{id}: {err}");
        assert!(err.to_string().contains("stash.push"), "{id}: {err}");
    }
    repo.stage(&["a.txt"]);
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "other" }))
        .unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    repo.git(&["reset", "--quiet", "--hard"]);

    // An untracked file in place of the incoming file: nothing written, no backup.
    repo.write("b.txt", "mine\n");
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "other" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mine\n"));
    // Ignored: the same (git would overwrite it without saying).
    repo.write(".git/info/exclude", "b.txt\n");
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "other" }))
        .unwrap_err();
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mine\n"));
    // The revert that recreates a file: an untracked file in its place.
    std::fs::remove_file(repo.path().join("b.txt")).unwrap();
    repo.git(&["rm", "--quiet", "c.txt"]);
    let gone = repo.commit("removes c");
    repo.write("c.txt", "mine\n");
    let err = r
        .run("commit.revert", &mut s, json!({ "rev": gone.to_string() }))
        .unwrap_err();
    assert!(err.to_string().contains("`c.txt`"), "{err}");
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("mine\n"));
    assert_eq!(repo.rev("main"), gone);
    assert_ne!(gone, main);
    assert!(backups(&repo, "cherry-pick").is_empty());
    assert!(backups(&repo, "revert").is_empty());
    // Untracked files are fine.
    std::fs::remove_file(repo.path().join("c.txt")).unwrap();
    repo.write("other.txt", "x\n");
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(read(&repo, "other.txt").as_deref(), Some("x\n"));
}

#[test]
fn detached_head_is_refused() {
    let (repo, main, _) = picking_repo();
    repo.detach(main);
    let (r, mut s) = open(&repo);
    for id in ["commit.cherry_pick", "commit.revert"] {
        let err = r.run(id, &mut s, json!({ "rev": "other" })).unwrap_err();
        assert!(err.to_string().contains("HEAD is detached"), "{id}: {err}");
    }
    assert_eq!(repo.rev("HEAD"), main);
    assert!(clean(&repo));
}

#[test]
fn failing_signature_leaves_it_in_progress() {
    let (repo, main, _) = picking_repo();
    repo.git(&["config", "commit.gpgSign", "true"]);
    repo.git(&["config", "gpg.program", "false"]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!([]));
    assert!(out["message"].as_str().unwrap().contains("gpg"), "{out}");
    assert_eq!(repo.rev("main"), main);
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    // Failed after the backup: the backup stays.
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), main);
    // With the signature still broken, continue reports git's error and stays in progress.
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("gpg"), "{err}");
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    repo.git(&["config", "commit.gpgSign", "false"]);
    run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(repo.git(&["log", "-1", "--format=%an", "main"]), "Cleo");
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
}

#[test]
fn a_sequence_started_from_the_terminal_stays_there() {
    let (repo, main, _) = conflicting_repo();
    repo.checkout("other");
    let p2 = repo.commit_file("z.txt", "z\n", "z");
    repo.checkout("main");
    repo.git_fails(&["cherry-pick", "other~1", &p2.to_string()]);
    assert!(has(&repo, "sequencer"));
    let (r, mut s) = open(&repo);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["continue"], Value::Null);
    assert_eq!(op["abort"], Value::Null);
    for (id, params) in [
        ("cherry_pick.continue", json!({})),
        ("cherry_pick.abort", json!({ "confirm": true })),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains("terminal"), "{id}: {err}");
    }
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    assert!(backups(&repo, "cherry-pick-abort").is_empty());
    repo.git(&["cherry-pick", "--abort"]);
    assert_eq!(repo.rev("main"), main);
}

#[test]
fn rerere_auto_update_does_not_hide_the_conflict() {
    let (repo, main, _) = conflicting_repo();
    repo.git(&["config", "rerere.enabled", "true"]);
    repo.git(&["config", "rerere.autoUpdate", "true"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "merged\n" }),
    );
    run(&r, &mut s, "cherry_pick.continue", json!({}));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    // Again: rerere puts the resolution back in the file, but the conflict still has to be confirmed.
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("merged\n"));
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(parents(&repo, "main"), [main]);
}

#[test]
fn assume_unchanged_files_and_option_like_revs_are_refused() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("x");
    repo.checkout("x");
    repo.commit_file("a.txt", "x\n", "change a");
    repo.checkout("main");
    let main = repo.commit_file("c.txt", "c\n", "adds c");
    repo.git(&["update-index", "--assume-unchanged", "a.txt"]);
    repo.write("a.txt", "hidden\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "x" }))
        .unwrap_err();
    assert!(err.to_string().contains("assume-unchanged"), "{err}");
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("hidden\n"));
    assert_eq!(repo.rev("main"), main);
    assert!(backups(&repo, "cherry-pick").is_empty());

    for rev in ["--hard", "-x"] {
        let err = r
            .run("commit.cherry_pick", &mut s, json!({ "rev": rev }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{rev}: {err}");
    }
}
