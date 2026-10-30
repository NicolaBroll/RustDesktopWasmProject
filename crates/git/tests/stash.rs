//! Integration tests of stashes (S28: `stash.list`, `stash.push`, `stash.apply`,
//! `stash.pop`, `stash.drop` and their undo/redo) with the `CliBackend` on temporary repos:
//! push with options (untracked, `keep_index`, paths, message starting with `-`),
//! undo of the push that also restores the staged/unstaged split, refusals that change
//! nothing (local changes, untracked and ignored files in place of the stash's files: cases in
//! which `git stash apply` would write halfway), conflicts left with the stash kept,
//! `restore_index` that does not apply, drop with confirmation, backup and undo, stash bases in
//! the graph.

mod common;

use common::Repo;
use project_engine::{EngineError, GitStash, Registry, Session};
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

/// Status (with untracked and ignored files), file contents and refs: to see that nothing
/// changes, or that everything is back as it was.
fn snapshot(repo: &Repo) -> (String, Vec<(String, String)>, String) {
    let status = repo.git(&["status", "--porcelain", "-uall", "--ignored"]);
    let mut files = Vec::new();
    for name in ["a.txt", "b.txt", "new.txt", "dir/u.txt", "x.log", "c.txt"] {
        if let Ok(content) = std::fs::read_to_string(repo.path().join(name)) {
            files.push((name.to_owned(), content));
        }
    }
    let refs = repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    (status, files, refs)
}

/// A repo with two committed files and every kind of change: `a.txt` changed in the index and
/// then again outside (`MM`), `b.txt` changed outside the index, `new.txt` added to the index,
/// `dir/u.txt` untracked, `x.log` ignored.
fn dirty() -> Repo {
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignore the logs");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("two files");
    repo.write("a.txt", "a staged\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "a unstaged\n");
    repo.write("b.txt", "b unstaged\n");
    repo.write("new.txt", "new\n");
    repo.stage(&["new.txt"]);
    repo.write("dir/u.txt", "untracked\n");
    repo.write("x.log", "ignored\n");
    repo
}

#[test]
fn push_list_undo_and_redo() {
    let repo = dirty();
    let before = snapshot(&repo);
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    assert_eq!(
        run(&r, &mut s, "stash.list", json!({})),
        json!({ "stashes": [] })
    );

    let out = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "message": "-x --all test", "include_untracked": true }),
    );
    let stash = out["stash"].clone();
    assert_eq!(stash["message"], "On main: -x --all test");
    assert_eq!(stash["branch"], "main");
    assert_eq!(stash["base"], head.to_string());
    assert_eq!(stash["untracked"], true);
    assert_eq!(stash["id"], repo.rev("refs/stash").to_string());
    assert_eq!(out["stashes"], json!([stash]));
    // Only the ignored file remains; no `--all`.
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall", "--ignored"]),
        "!! x.log"
    );
    assert_eq!(
        repo.git(&["stash", "list"]),
        "stash@{0}: On main: -x --all test"
    );
    // The stash commit reads like a commit: the tracked files relative to the base.
    let show = run(&r, &mut s, "commit.show", json!({ "id": stash["id"] }));
    let files: Vec<&str> = show["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["a.txt", "b.txt", "new.txt"]);
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"], json!([stash]));

    // Undo: everything as before, staged and unstaged, with the stash commit in the backup.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "stash.push");
    assert_eq!(snapshot(&repo).0, before.0);
    assert_eq!(snapshot(&repo).1, before.1);
    let backup = undo["backups"][0].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/undo-"), "{backup}");
    assert_eq!(repo.rev(&backup).to_string(), stash["id"]);
    assert_eq!(repo.git(&["stash", "list"]), "");

    // Redo: stashed again, with the same parameters.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["redone"]["command"], "stash.push");
    assert_eq!(
        repo.git(&["stash", "list"]),
        "stash@{0}: On main: -x --all test"
    );
    assert_eq!(
        redo["redone"]["stash"]["id"],
        repo.rev("refs/stash").to_string()
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(snapshot(&repo).1, before.1);
}

#[test]
fn push_with_keep_index_and_paths() {
    let repo = dirty();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({ "keep_index": true }));
    // The index stays, the worktree matches the index.
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall"]),
        "M  a.txt\nA  new.txt\n?? dir/u.txt"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a staged\n"
    );

    let repo = dirty();
    let (r, mut s) = open(&repo);
    // A path starting with `-` and a real one: they go to git on stdin, literally.
    let err = r
        .run("stash.push", &mut s, json!({ "paths": ["-x"] }))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("nothing to stash")),
        "{err}"
    );
    let out = run(&r, &mut s, "stash.push", json!({ "paths": ["b.txt"] }));
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall"]),
        "MM a.txt\nA  new.txt\n?? dir/u.txt"
    );
    assert!(
        out["stash"]["message"]
            .as_str()
            .unwrap()
            .starts_with("WIP on main: "),
        "{out}"
    );
}

#[test]
fn push_refusals_change_nothing() {
    // Nothing to stash (only an ignored file and, without `include_untracked`, an untracked
    // one).
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignore");
    repo.write("x.log", "?\n");
    repo.write("u.txt", "?\n");
    let (r, mut s) = open(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.git(&["stash", "list"]), "");

    // An `intent-to-add` file: git would not know how to save it.
    repo.write("i.txt", "x\n");
    repo.git(&["add", "-N", "i.txt"]);
    let before = snapshot(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("intent-to-add"), "{err}");
    assert_eq!(snapshot(&repo), before);

    // Conflicting merge.
    let repo = common::repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let before = snapshot(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("a merge is in progress"), "{err}");
    assert_eq!(snapshot(&repo), before);

    // Branch without commits.
    let repo = Repo::new();
    repo.write("a.txt", "x\n");
    repo.stage(&["a.txt"]);
    let (r, mut s) = open(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("has no commits yet"), "{err}");
}

#[test]
fn apply_and_pop() {
    let repo = dirty();
    let before = snapshot(&repo);
    let (r, mut s) = open(&repo);
    let id = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    )["stash"]["id"]
        .clone();

    // Apply without index: everything unstaged except the new file, the stash stays.
    let out = run(&r, &mut s, "stash.apply", json!({ "expected": id }));
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["stashes"][0]["id"], id);
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall"]),
        " M a.txt\n M b.txt\nA  new.txt\n?? dir/u.txt"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a unstaged\n"
    );
    // Not in the journal: the next undo is the push.
    assert_eq!(
        run(&r, &mut s, "edit.history", json!({}))["done"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Again on top of the same changes: refused before git, nothing changes.
    let now = snapshot(&repo);
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`a.txt`") && m.contains("`dir/u.txt`")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), now);

    // Pop with the index from a clean worktree: everything as at the start, with the backup.
    repo.git(&["reset", "--hard", "--quiet"]);
    repo.git(&["clean", "-fdq"]);
    let out = run(
        &r,
        &mut s,
        "stash.pop",
        json!({ "expected": id, "restore_index": true }),
    );
    assert_eq!(out["dropped"], true);
    assert_eq!(out["stashes"], json!([]));
    assert_eq!(snapshot(&repo).0, before.0);
    assert_eq!(snapshot(&repo).1, before.1);
    let backup = out["backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/stash-"), "{backup}");
    assert_eq!(repo.rev(backup).to_string(), id);
}

#[test]
fn files_in_the_way_block_the_apply_before_git_writes() {
    // `git stash apply` with an untracked file of the stash already present would restore the
    // tracked files and then fail: here nothing changes.
    let repo = dirty();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    repo.write("dir/u.txt", "another\n");
    let before = snapshot(&repo);
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`dir/u.txt`")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), before);

    // A file in place of the folder of an untracked file of the stash.
    std::fs::remove_dir_all(repo.path().join("dir")).unwrap();
    repo.write("dir", "file\n");
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`dir`")),
        "{err}"
    );

    // An ignored file in place of a file the stash adds: git would overwrite it.
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignore");
    repo.write("new.log", "in stash\n");
    repo.git(&["add", "-f", "new.log"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.write("new.log", "ignored, do not lose\n");
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`new.log`")),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("new.log")).unwrap(),
        "ignored, do not lose\n"
    );
    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
}

#[test]
fn conflicts_are_left_and_pop_keeps_the_stash() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\ntwo\nthree\n", "first");
    repo.write("a.txt", "one\nTWO from the stash\nthree\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let (r, mut s) = open(&repo);
    let id = run(&r, &mut s, "stash.push", json!({}))["stash"]["id"].clone();
    repo.commit_file("a.txt", "one\nTWO from main\nthree\n", "change a");

    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["stashes"][0]["id"], id);
    assert!(
        out["warnings"][0]["text"]
            .as_str()
            .unwrap()
            .contains("stays in the list"),
        "{out}"
    );
    let a = std::fs::read_to_string(repo.path().join("a.txt")).unwrap();
    assert!(
        a.contains("<<<<<<<") && a.contains("TWO from the stash"),
        "{a}"
    );
    assert!(repo.git(&["status", "--porcelain"]).contains("UU a.txt"));
    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
    // With the conflict open, no stash.
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("conflicted"), "{err}");
}

#[test]
fn restore_index_that_does_not_apply_changes_nothing() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\nb\nc\n", "first");
    repo.write("a.txt", "a\nS\nc\n");
    repo.stage(&["a.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.commit_file("a.txt", "a\nT\nc\n", "change a");
    let before = snapshot(&repo);
    let err = r
        .run("stash.apply", &mut s, json!({ "restore_index": true }))
        .unwrap_err();
    assert!(err.to_string().contains("without `restore_index`"), "{err}");
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn drop_needs_confirm_and_undo_puts_it_back_on_top() {
    let repo = dirty();
    let (r, mut s) = open(&repo);
    let first = run(&r, &mut s, "stash.push", json!({ "paths": ["b.txt"] }))["stash"].clone();
    let second = run(&r, &mut s, "stash.push", json!({ "message": "the rest" }))["stash"].clone();
    let refs = repo.git(&["for-each-ref"]);

    let err = r
        .run("stash.drop", &mut s, json!({ "index": 1 }))
        .unwrap_err();
    let EngineError::ConfirmRequired { expected, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(expected.as_deref(), first["id"].as_str());
    let err2 = r
        .run("stash.drop", &mut s, json!({ "index": 1, "confirm": true }))
        .unwrap_err();
    assert!(matches!(err2, EngineError::InvalidParams { .. }), "{err2}");
    // With paths git stashes the whole index anyway: also `a.txt` and
    // `new.txt`, which were staged (and stay in the worktree).
    assert!(err.to_string().contains("3 tracked files"), "{err}");
    assert_eq!(repo.git(&["for-each-ref"]), refs);
    // The confirmation holds for the stash that was seen.
    let err = r
        .run(
            "stash.drop",
            &mut s,
            json!({ "index": 1, "expected": second["id"], "confirm": true }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("is now `stash@{0}`"), "{err}");

    let out = run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "index": 1, "expected": first["id"], "confirm": true }),
    );
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/stash-"), "{backup}");
    assert_eq!(repo.rev(&backup).to_string(), first["id"]);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(
        repo.git(&["stash", "list", "--format=%H"]),
        second["id"].as_str().unwrap()
    );

    // Undo: back on top.
    run(&r, &mut s, "edit.undo", json!({}));
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"][0]["id"], first["id"]);
    assert_eq!(list["stashes"][0]["message"], first["message"]);
    assert_eq!(list["stashes"][1]["id"], second["id"]);
    // Redo: dropped again, wherever it is. The drop backup was removed by hand: a new one is
    // made before dropping.
    repo.git(&["update-ref", "-d", &backup]);
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    let again = redo["backups"][0].as_str().unwrap().to_owned();
    assert!(again.starts_with("refs/project/backup/redo-"), "{redo}");
    assert_eq!(repo.rev(&again).to_string(), first["id"]);
    assert_eq!(
        repo.git(&["stash", "list", "--format=%H"]),
        second["id"].as_str().unwrap()
    );
    run(&r, &mut s, "edit.undo", json!({}));
    // And it still applies.
    let out = run(&r, &mut s, "stash.pop", json!({ "expected": first["id"] }));
    assert_eq!(out["dropped"], true);
    assert!(repo.git(&["status", "--porcelain"]).contains(" M b.txt"));
}

#[test]
fn undo_of_a_push_is_refused_once_head_moved() {
    let repo = dirty();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.commit_file("c.txt", "c\n", "from the terminal");
    let before = snapshot(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].starts_with("HEAD")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn log_reaches_the_stash_bases() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "first");
    let base = repo.commit_file("a.txt", "b\n", "second");
    repo.write("a.txt", "c\n");
    let (r, mut s) = open(&repo);
    let stash = run(&r, &mut s, "stash.push", json!({}))["stash"].clone();
    assert_eq!(stash["base"], base.to_string());
    // `main` goes back: the base stays only as the parent of the stash.
    repo.git(&["reset", "--hard", "--quiet", &first.to_string()]);
    let ids = |out: &Value| -> Vec<String> {
        out["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let all = run(&r, &mut s, "repo.log", json!({ "all": true }));
    assert_eq!(ids(&all), [first.to_string()]);
    let with = run(
        &r,
        &mut s,
        "repo.log",
        json!({ "all": true, "stashes": true }),
    );
    assert_eq!(ids(&with), [base.to_string(), first.to_string()]);
    // The backend says the same.
    let backend = repo.open();
    assert_eq!(backend.stashes().unwrap()[0].base, base);
}

#[test]
fn push_refuses_assume_unchanged_files() {
    // `git stash push` would not see the change to `b.txt` and then would delete it.
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("two files");
    repo.git(&["update-index", "--assume-unchanged", "b.txt"]);
    repo.write("a.txt", "a changed\n");
    repo.write("b.txt", "b changed, do not lose\n");
    let (r, mut s) = open(&repo);
    for params in [
        json!({}),
        json!({ "keep_index": true }),
        json!({ "include_untracked": true }),
        json!({ "paths": ["b.txt"] }),
    ] {
        let err = r.run("stash.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.contains("assume-unchanged") && m.contains("`b.txt`")),
            "{params}: {err}"
        );
    }
    assert_eq!(repo.git(&["stash", "list"]), "");
    // Outside the chosen paths it does not matter.
    run(&r, &mut s, "stash.push", json!({ "paths": ["a.txt"] }));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("b.txt")).unwrap(),
        "b changed, do not lose\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a\n"
    );
}

#[test]
fn apply_does_not_follow_directory_renames_onto_an_ignored_file() {
    // The stash adds `old/x.log`; then HEAD renames `old` to `new`, where there is an ignored
    // `x.log`: with directory rename detection git would overwrite it.
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignore");
    repo.commit_file("old/a.txt", "a\n", "old");
    repo.write("old/x.log", "from the stash\n");
    repo.git(&["add", "-f", "old/x.log"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.git(&["mv", "old", "new"]);
    repo.commit("rename the folder");
    repo.write("new/x.log", "ignored, do not lose\n");
    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(out["dropped"], true);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("new/x.log")).unwrap(),
        "ignored, do not lose\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("old/x.log")).unwrap(),
        "from the stash\n"
    );
}

/// `a.txt` changed only in the index, `b.txt` only outside it.
fn staged_and_unstaged() -> Repo {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("two files");
    repo.write("a.txt", "a staged\n");
    repo.stage(&["a.txt"]);
    repo.write("b.txt", "b unstaged\n");
    repo
}

#[test]
fn undo_of_keep_index_and_paths_pushes() {
    for params in [json!({ "keep_index": true }), json!({ "paths": ["b.txt"] })] {
        let repo = staged_and_unstaged();
        let before = snapshot(&repo);
        let (r, mut s) = open(&repo);
        run(&r, &mut s, "stash.push", params.clone());
        // `a.txt` stays staged (also with `paths`, but git puts it in the stash too).
        assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt", "{params}");
        let undo = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(undo["undone"]["command"], "stash.push", "{params}");
        assert_eq!(snapshot(&repo).0, before.0, "{params}");
        assert_eq!(snapshot(&repo).1, before.1, "{params}");
        // And the pop after such a push.
        run(&r, &mut s, "edit.redo", json!({}));
        let out = run(&r, &mut s, "stash.pop", json!({ "restore_index": true }));
        assert_eq!(out["dropped"], true, "{params}");
        assert_eq!(snapshot(&repo).0, before.0, "{params}");
    }
}

#[test]
fn untracked_to_put_back_blocks_even_when_the_status_shows_it() {
    // The stash has the untracked file `p` and a change to `t.txt`. Then `p` is committed,
    // removed from the index (`D.`), ignored and rewritten: `git stash apply` would merge
    // `t.txt` and then fail on `p`. Here nothing changes.
    let repo = Repo::new();
    repo.commit_file("t.txt", "t\n", "first");
    repo.write("p", "from the stash\n");
    repo.write("t.txt", "t changed\n");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    repo.commit_file("p", "committed\n", "p");
    repo.git(&["rm", "--cached", "--quiet", "p"]);
    repo.write(".git/info/exclude", "p\n");
    repo.write("p", "LOCAL\n");
    assert_eq!(repo.git(&["status", "--porcelain"]), "D  p");
    let before = snapshot(&repo);
    let p = || std::fs::read_to_string(repo.path().join("p")).unwrap();
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`p`")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), before);
    assert_eq!(p(), "LOCAL\n");
    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
}

#[test]
fn undo_of_a_paths_push_with_other_staged_files_explains_the_way_out() {
    // `a.txt` and `b.txt` staged, push of `b.txt`: git stashes the whole index, and
    // `pop --index` does not apply on top of an index that already has `a.txt`.
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("two files");
    repo.write("a.txt", "a staged\n");
    repo.write("b.txt", "b staged\n");
    repo.stage(&["a.txt", "b.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({ "paths": ["b.txt"] }));
    let before = snapshot(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("`stash.pop` without `restore_index`") && !text.contains("try again without"),
        "{text}"
    );
    assert_eq!(snapshot(&repo), before);
    assert_eq!(run(&r, &mut s, "edit.history", json!({}))["can_undo"], true);
    // The suggested way out works.
    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["dropped"], true);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("b.txt")).unwrap(),
        "b staged\n"
    );
}
