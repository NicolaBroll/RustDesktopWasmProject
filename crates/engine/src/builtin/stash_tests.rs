//! Tests of `stash.list`, `stash.push`, `stash.apply`, `stash.pop`, `stash.drop` and their
//! undo/redo on the `MemoryBackend` (sample repo: `README.md` `MM`, `docs/guide.md` `A.`,
//! `src/lib.rs` `.M`, `note.txt` untracked).

use crate::{EngineError, GitBackend, MemoryBackend, MemoryOpener, Registry, Session, StashPush};
use project_domain::{
    Change, ChangedFile, Commit, EntryKind, Operation, RepoPath, Signature, StatusEntry,
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

fn opened() -> (Registry, Session) {
    opened_with(MemoryBackend::sample())
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// `(path, X, Y)` of each state entry.
fn codes(status: &Value) -> Vec<(String, String, String)> {
    status["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["path"].as_str().unwrap().to_owned(),
                e["index"].as_str().unwrap().to_owned(),
                e["worktree"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn code(path: &str, x: &str, y: &str) -> (String, String, String) {
    (path.to_owned(), x.to_owned(), y.to_owned())
}

fn stash_count(r: &Registry, s: &mut Session) -> usize {
    run(r, s, "stash.list", json!({}))["stashes"]
        .as_array()
        .unwrap()
        .len()
}

#[test]
fn push_list_undo_and_redo() {
    let (r, mut s) = opened();
    let before = run(&r, &mut s, "repo.status", json!({}));
    let diff_before = run(&r, &mut s, "diff.worktree", json!({}));
    assert_eq!(
        run(&r, &mut s, "stash.list", json!({})),
        json!({ "stashes": [] })
    );

    let out = run(&r, &mut s, "stash.push", json!({}));
    let stash = &out["stash"];
    let head = before["head"]["Branch"]["target"].clone();
    assert_eq!(stash["index"], 0);
    assert_eq!(stash["name"], "stash@{0}");
    assert_eq!(stash["base"], head);
    assert_eq!(stash["branch"], "main");
    assert_eq!(stash["untracked"], false);
    assert!(
        stash["message"]
            .as_str()
            .unwrap()
            .starts_with("WIP on main: "),
        "{stash}"
    );
    // Only the untracked files remain (without `include_untracked`).
    assert_eq!(
        codes(&out["status"]),
        [code("note.txt", "Untracked", "Untracked")]
    );
    assert_eq!(out["stashes"], json!([stash]));
    assert_eq!(out["can_undo"], true);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Stash changes (stash ")
    );
    // `refs/stash` is among the refs, and the stash commit reads as a commit.
    let refs = run(&r, &mut s, "repo.refs", json!({}));
    let stash_ref = refs
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "refs/stash")
        .unwrap();
    assert_eq!(stash_ref["kind"], "Stash");
    assert_eq!(stash_ref["target"], stash["id"]);
    let show = run(&r, &mut s, "commit.show", json!({ "id": stash["id"] }));
    let files: Vec<&str> = show["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["README.md", "docs/guide.md", "src/lib.rs"]);
    assert_eq!(show["compared_to"], head);

    // A second push has nothing to set aside.
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("nothing to stash")),
        "{err}"
    );

    // Undo: the stash goes back into the files (also the split between staged and not) and disappears
    // from the list, with the commit in a backup.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "stash.push");
    assert_eq!(undo["status"], before);
    assert_eq!(undo["backups"], json!(["refs/project/backup/undo-1"]));
    assert_eq!(run(&r, &mut s, "diff.worktree", json!({})), diff_before);
    assert_eq!(stash_count(&r, &mut s), 0);
    assert_eq!(
        undo["undone"]["stash"]["push"],
        json!({ "message": null, "include_untracked": false, "keep_index": false, "paths": null })
    );

    // Redo: set aside again.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(codes(&redo["status"]), codes(&out["status"]));
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"].as_array().unwrap().len(), 1);
    assert_eq!(redo["redone"]["stash"]["id"], list["stashes"][0]["id"]);
    // And undo again.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["status"], before);
}

#[test]
fn push_options() {
    // With the untracked files and a message.
    let (r, mut s) = opened();
    let out = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "message": "-x half done", "include_untracked": true }),
    );
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(out["stash"]["message"], "On main: -x half done");
    assert_eq!(out["stash"]["untracked"], true);
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        codes(&undo["status"]),
        [
            code("README.md", "Modified", "Modified"),
            code("docs/guide.md", "Added", "Unmodified"),
            code("src/lib.rs", "Unmodified", "Modified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );

    // `keep_index`: what is staged stays, and the worktree is as the index.
    let (r, mut s) = opened();
    let out = run(&r, &mut s, "stash.push", json!({ "keep_index": true }));
    assert_eq!(
        codes(&out["status"]),
        [
            code("README.md", "Modified", "Unmodified"),
            code("docs/guide.md", "Added", "Unmodified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );

    // Only some paths.
    let (r, mut s) = opened();
    let out = run(&r, &mut s, "stash.push", json!({ "paths": ["src"] }));
    assert_eq!(
        codes(&out["status"]),
        [
            code("README.md", "Modified", "Modified"),
            code("docs/guide.md", "Added", "Unmodified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );
    let show = run(
        &r,
        &mut s,
        "commit.show",
        json!({ "id": out["stash"]["id"] }),
    );
    assert_eq!(show["files"].as_array().unwrap().len(), 1);
    // An untracked file requested without `include_untracked`.
    let err = r
        .run("stash.push", &mut s, json!({ "paths": ["note.txt"] }))
        .unwrap_err();
    assert!(err.to_string().contains("include_untracked"), "{err}");
}

#[test]
fn push_refusals() {
    let (r, mut s) = opened();
    for params in [
        json!({ "message": "due\nrighe" }),
        json!({ "paths": [] }),
        json!({ "paths": ["../fuori"] }),
        json!({ "all": true }),
        json!({ "message": 3 }),
        json!({ "keep_index": "yes" }),
    ] {
        let err = r.run("stash.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::InvalidParams {
                    id: "stash.push",
                    ..
                }
            ),
            "{params}: {err}"
        );
    }
    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(Operation::Merge));
    let (r, mut s) = opened_with(repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(stash_count(&r, &mut s), 0);

    let (r, mut s) = opened_with(MemoryBackend::new());
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("has no commits yet"), "{err}");
}

#[test]
fn apply_keeps_the_stash_and_pop_drops_it() {
    let (r, mut s) = opened();
    let before = run(&r, &mut s, "repo.status", json!({}));
    let pushed = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    let id = pushed["stash"]["id"].clone();

    // Apply without index: everything unstaged, except the new file (like git).
    let out = run(&r, &mut s, "stash.apply", json!({ "expected": id }));
    assert_eq!(out["stash"]["id"], id);
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["stashes"].as_array().unwrap().len(), 1);
    assert_eq!(
        codes(&out["status"]),
        [
            code("README.md", "Unmodified", "Modified"),
            code("docs/guide.md", "Added", "Unmodified"),
            code("src/lib.rs", "Unmodified", "Modified"),
            code("note.txt", "Untracked", "Untracked"),
        ]
    );
    // Apply and pop do not enter the journal: the next undo is still the push.
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"].as_array().unwrap().len(), 1);
    assert_eq!(history["done"][0]["command"], "stash.push");
    // Again over the same changes: refused, nothing changes.
    let status = run(&r, &mut s, "repo.status", json!({}));
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    let EngineError::NotAllowed(message) = &err else {
        panic!("{err}");
    };
    assert!(message.contains("`README.md`"), "{message}");
    assert!(message.contains("`note.txt`"), "{message}");
    assert_eq!(run(&r, &mut s, "repo.status", json!({})), status);
    assert_eq!(stash_count(&r, &mut s), 1);

    // Pop with the index from a clean repo: everything as it was, the stash leaves with a backup.
    let (r, mut s) = opened();
    run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    let out = run(&r, &mut s, "stash.pop", json!({ "restore_index": true }));
    assert_eq!(out["status"], before);
    assert_eq!(out["dropped"], true);
    assert_eq!(out["backup"], "refs/project/backup/stash-1");
    assert_eq!(out["stashes"], json!([]));
    assert_eq!(out["warnings"], json!([]));
}

#[test]
fn apply_of_a_moved_stash_or_a_missing_one() {
    let (r, mut s) = opened();
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("there are no stashes"), "{err}");
    let first = run(&r, &mut s, "stash.push", json!({ "paths": ["src"] }));
    let second = run(&r, &mut s, "stash.push", json!({ "paths": ["README.md"] }));
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"][0]["id"], second["stash"]["id"]);
    assert_eq!(list["stashes"][1]["id"], first["stash"]["id"]);
    assert_eq!(list["stashes"][1]["name"], "stash@{1}");
    let err = r
        .run("stash.apply", &mut s, json!({ "index": 2 }))
        .unwrap_err();
    assert!(err.to_string().contains("there are 2 stashes"), "{err}");
    // The stash seen at position 0 is now at position 1.
    let err = r
        .run(
            "stash.pop",
            &mut s,
            json!({ "index": 0, "expected": first["stash"]["id"] }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("is now `stash@{1}`"), "{err}");
    for params in [
        json!({ "expected": "abc" }),
        json!({ "index": -1 }),
        json!({ "id": "x" }),
        json!({ "confirm": true }),
    ] {
        let err = r.run("stash.apply", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    // The pop of the oldest leaves the most recent on top.
    let out = run(
        &r,
        &mut s,
        "stash.pop",
        json!({ "index": 1, "expected": first["stash"]["id"] }),
    );
    assert_eq!(out["dropped"], true);
    assert_eq!(out["stashes"][0]["id"], second["stash"]["id"]);
}

/// The sample repo with the changes set aside, then a commit that changes `README.md`
/// on `main`: the stash conflicts on that file.
fn moved_head() -> (MemoryBackend, String) {
    let mut repo = MemoryBackend::sample();
    let old = match &repo.status().unwrap().entries[0].kind {
        EntryKind::Ordinary(info) => info.oid_head,
        other => panic!("{other:?}"),
    };
    let stash = repo.stash_push(&StashPush::default()).unwrap();
    let head = repo.head().unwrap().target().unwrap();
    let new = MemoryBackend::fake_oid(4242);
    repo.set_blob(new, "README cambiato su main\n");
    let id = MemoryBackend::fake_oid(4243);
    let who = Signature {
        name: "Ada".to_owned(),
        email: "ada@example.com".to_owned(),
        time: 1_800_000_000,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id,
        parents: vec![head],
        author: who.clone(),
        committer: who,
        summary: "README".to_owned(),
        body: String::new(),
    });
    repo.set_changes(
        id,
        vec![ChangedFile {
            path: RepoPath::from("README.md"),
            change: Change::Modified,
            original: None,
            score: None,
            old_mode: 0o100644,
            new_mode: 0o100644,
            old_oid: old,
            new_oid: new,
        }],
    );
    repo.set_branch("main", id);
    (repo, stash.id.to_string())
}

#[test]
fn conflicts_are_left_and_pop_keeps_the_stash() {
    let (repo, id) = moved_head();
    let (r, mut s) = opened_with(repo);
    let out = run(&r, &mut s, "stash.pop", json!({ "expected": id }));
    assert_eq!(out["conflicts"], json!(["README.md"]));
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
    let readme = &out["status"]["entries"][0];
    assert_eq!(readme["path"], "README.md");
    assert_eq!(readme["index"], "Unmerged");
    // The other files are applied.
    assert!(
        codes(&out["status"]).contains(&code("src/lib.rs", "Unmodified", "Modified")),
        "{}",
        out["status"]
    );
    // With a conflict open no stash: neither apply nor push.
    for id in ["stash.apply", "stash.push"] {
        let err = r.run(id, &mut s, json!({})).unwrap_err();
        assert!(err.to_string().contains("conflict"), "{id}: {err}");
    }
    // Resolved by staging the file, the stash is dropped by hand.
    run(&r, &mut s, "index.stage", json!({ "paths": ["README.md"] }));
    let out = run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "confirm": true, "expected": id }),
    );
    assert_eq!(out["stashes"], json!([]));
}

#[test]
fn untracked_in_the_way_blocks_the_apply() {
    let mut repo = MemoryBackend::sample();
    repo.stash_push(&StashPush {
        include_untracked: true,
        ..StashPush::default()
    })
    .unwrap();
    // `note.txt` is back in the worktree.
    repo.set_worktree_file("note.txt", "altro\n");
    repo.set_entries(vec![StatusEntry {
        path: RepoPath::from("note.txt"),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    }]);
    let (r, mut s) = opened_with(repo);
    let before = run(&r, &mut s, "repo.status", json!({}));
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`note.txt`")),
        "{err}"
    );
    assert_eq!(run(&r, &mut s, "repo.status", json!({})), before);
}

#[test]
fn drop_needs_confirm_and_undo_puts_it_back_on_top() {
    let (r, mut s) = opened();
    let first = run(&r, &mut s, "stash.push", json!({ "paths": ["src"] }))["stash"].clone();
    let second = run(&r, &mut s, "stash.push", json!({ "message": "readme" }))["stash"].clone();

    let err = r
        .run("stash.drop", &mut s, json!({ "index": 1 }))
        .unwrap_err();
    let EngineError::ConfirmRequired {
        command,
        reason,
        expected,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*command, "stash.drop");
    // The id to repeat in `expected`, also in the text.
    assert_eq!(expected.as_deref(), first["id"].as_str());
    assert!(reason.contains(first["id"].as_str().unwrap()), "{reason}");
    // `confirm` without `expected` is not enough.
    let err = r
        .run("stash.drop", &mut s, json!({ "index": 1, "confirm": true }))
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::InvalidParams {
                id: "stash.drop",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(stash_count(&r, &mut s), 2);
    assert!(reason.contains("`stash@{1}`"), "{reason}");
    assert!(reason.contains("1 tracked file"), "{reason}");
    assert_eq!(stash_count(&r, &mut s), 2);
    let err = r
        .run(
            "stash.drop",
            &mut s,
            json!({ "index": 1, "expected": second["id"], "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    let out = run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "index": 1, "expected": first["id"], "confirm": true }),
    );
    assert_eq!(out["stash"]["id"], first["id"]);
    assert_eq!(out["backup"], "refs/project/backup/stash-1");
    assert_eq!(out["stashes"], json!([second]));
    assert_eq!(
        out["next_undo"]["text"],
        format!(
            "Drop stash@{{1}} ({} “{}”)",
            &first["id"].as_str().unwrap()[..7],
            first["message"].as_str().unwrap()
        )
    );
    let entry = &run(&r, &mut s, "edit.history", json!({}))["done"][2];
    assert_eq!(entry["command"], "stash.drop");
    assert_eq!(entry["backup"], "refs/project/backup/stash-1");
    assert_eq!(entry["stash"]["id"], first["id"]);

    // Undo: it comes back, but on top (`git stash store`).
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "stash.drop");
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"][0]["id"], first["id"]);
    assert_eq!(list["stashes"][0]["message"], first["message"]);
    assert_eq!(list["stashes"][1]["id"], second["id"]);
    // And it applies again with its files.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["redone"]["command"], "stash.drop");
    assert_eq!(stash_count(&r, &mut s), 1);
    run(&r, &mut s, "edit.undo", json!({}));
    let out = run(&r, &mut s, "stash.pop", json!({ "expected": first["id"] }));
    assert!(
        codes(&out["status"]).contains(&code("src/lib.rs", "Unmodified", "Modified")),
        "{}",
        out["status"]
    );
}

#[test]
fn undo_conflicts() {
    // The stash is gone (a pop, which does not enter the journal): the undo of the push is refused.
    let (r, mut s) = opened();
    run(&r, &mut s, "stash.push", json!({}));
    run(&r, &mut s, "stash.pop", json!({}));
    let status = run(&r, &mut s, "repo.status", json!({}));
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].contains("is no longer in the list")),
        "{err}"
    );
    assert_eq!(run(&r, &mut s, "repo.status", json!({})), status);
    assert_eq!(run(&r, &mut s, "edit.history", json!({}))["can_undo"], true);

    // HEAD moved from outside after the push (a commit made without going through the journal).
    let (r, mut s) = opened();
    run(&r, &mut s, "stash.push", json!({}));
    let repo = s.repo_mut().unwrap();
    repo.stage(&[RepoPath::from("note.txt")]).unwrap();
    repo.commit(&crate::CommitRequest::new("from outside"))
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].starts_with("HEAD")),
        "{err}"
    );
    assert_eq!(stash_count(&r, &mut s), 1);

    // Undo of a drop with the stash back in the list (put back by hand).
    let (r, mut s) = opened();
    let id = run(&r, &mut s, "stash.push", json!({}))["stash"]["id"].clone();
    run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "confirm": true, "expected": id }),
    );
    let oid: project_domain::Oid = id.as_str().unwrap().parse().unwrap();
    s.repo_mut()
        .unwrap()
        .stash_store(&oid, "On main: a mano")
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].contains("already in the list")),
        "{err}"
    );
}

#[test]
fn log_with_the_stash_bases() {
    let mut repo = MemoryBackend::sample();
    let stash = repo.stash_push(&StashPush::default()).unwrap();
    // `main` goes back: the stash's base is no longer on any branch.
    let parent = repo.show_commit("HEAD").unwrap().commit.parents[0];
    repo.set_branch("main", parent);
    let (r, mut s) = opened_with(repo);
    let ids = |out: &Value| -> Vec<String> {
        out["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let base = stash.base.to_string();
    let all = run(&r, &mut s, "repo.log", json!({ "all": true }));
    assert!(!ids(&all).contains(&base));
    let with = run(
        &r,
        &mut s,
        "repo.log",
        json!({ "all": true, "stashes": true }),
    );
    assert!(ids(&with).contains(&base));
    assert!(
        !ids(&with).contains(&stash.id.to_string()),
        "the stash is not there"
    );
    let err = r
        .run("repo.log", &mut s, json!({ "stashes": true }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
}

#[test]
fn undo_of_a_keep_index_push_and_pop_after_it() {
    // `docs/guide.md` (`A.`) stays staged with `keep_index`: it already has the stash's version
    // and blocks neither the undo nor the pop.
    let (r, mut s) = opened();
    let before = run(&r, &mut s, "repo.status", json!({}));
    let out = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "keep_index": true, "paths": ["docs", "src"] }),
    );
    assert!(
        codes(&out["status"]).contains(&code("docs/guide.md", "Added", "Unmodified")),
        "{}",
        out["status"]
    );
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["status"], before);
    run(&r, &mut s, "edit.redo", json!({}));
    let out = run(&r, &mut s, "stash.pop", json!({ "restore_index": true }));
    assert_eq!(out["dropped"], true);
    assert_eq!(out["status"], before);
    // An `MM` file left with `keep_index` (worktree different from the stash's) blocks.
    let (r, mut s) = opened();
    run(&r, &mut s, "stash.push", json!({ "keep_index": true }));
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`README.md`") && !m.contains("guide")),
        "{err}"
    );
}

#[test]
fn a_failed_drop_after_the_apply_is_not_an_error_of_the_pop() {
    let mut repo = MemoryBackend::sample();
    repo.stash_push(&StashPush::default()).unwrap();
    repo.fail_stash_drop(Some("drop failed"));
    let (r, mut s) = opened_with(repo);
    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["conflicts"], json!([]));
    assert!(
        out["warnings"][0]["text"]
            .as_str()
            .unwrap()
            .contains("applied but not removed from the list"),
        "{out}"
    );
    assert_eq!(out["stashes"].as_array().unwrap().len(), 1);
    assert_eq!(out["status"]["entries"].as_array().unwrap().len(), 4);

    // Undo of a push: error saying the apply happened, the entry stays.
    let mut repo = MemoryBackend::sample();
    repo.fail_stash_drop(Some("drop failed"));
    let (r, mut s) = opened_with(repo);
    run(&r, &mut s, "stash.push", json!({}));
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        err.to_string().contains("applied stash")
            && err.to_string().contains("do not repeat the undo"),
        "{err}"
    );
    assert_eq!(run(&r, &mut s, "edit.history", json!({}))["can_undo"], true);
}

#[test]
fn stash_commands_need_an_open_repo() {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(MemoryOpener::sample()));
    for id in [
        "stash.list",
        "stash.push",
        "stash.apply",
        "stash.pop",
        "stash.drop",
    ] {
        let err = r.run(id, &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
    }
}
