//! Tests of `commit.cherry_pick`, `commit.revert`, `*.continue`, `*.abort` and `repo.operation`
//! with cherry-pick and revert on the `MemoryBackend` (with real git: `crates/git/tests/cherry_pick.rs`).
//!
//! The repo: `c1` (root: `a.txt`, `b.txt`); `main` at `c2` (`a.txt` = "ours"); `other` at
//! `c3` from `c1` (`a.txt` = "theirs": conflict with `main`); `feat` at `c4` from `c1`, by another
//! author (adds `n.txt`); `later` at `c5` from `c2` (`a.txt` = "later": the revert of `c2` there is
//! in conflict); `merged` at `c6`, merge commit of `c1` and `c4` (adds `n.txt`). HEAD on
//! `main`, clean worktree.

use crate::test_util::oid;
use crate::test_util::same;
use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::{Change, ChangedFile, Commit, EntryKind, Oid, RepoPath, Signature};
use serde_json::{Value, json};
use std::sync::Arc;

fn sig(name: &str, n: i64) -> Signature {
    Signature {
        name: name.into(),
        email: format!("{}@example.com", name.to_lowercase()),
        time: 1_700_000_000 + n * 60,
        offset_min: 0,
    }
}

fn blob(repo: &mut MemoryBackend, content: &str) -> Oid {
    let hash = content.bytes().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    });
    let id = oid(hash);
    repo.set_blob(id, content);
    id
}

/// Commit `n` with parents `parents` and the files `files` (path, content before if there was any,
/// content after) relative to the first parent.
fn commit(
    repo: &mut MemoryBackend,
    n: u64,
    parents: &[u64],
    author: &str,
    summary: &str,
    files: &[(&str, Option<&str>, &str)],
) {
    let changes = files
        .iter()
        .map(|(path, old, new)| {
            let old_oid = old.map(|c| blob(repo, c));
            let new_oid = blob(repo, new);
            ChangedFile {
                path: RepoPath::from(*path),
                change: if old.is_some() {
                    Change::Modified
                } else {
                    Change::Added
                },
                original: None,
                score: None,
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid: old_oid.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
                new_oid,
            }
        })
        .collect();
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parents.iter().map(|p| oid(*p)).collect(),
        author: sig(author, n as i64),
        committer: sig(author, n as i64),
        summary: summary.into(),
        body: String::new(),
    });
    repo.set_changes(oid(n), changes);
}

fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    let base = [("a.txt", None, "base\n"), ("b.txt", None, "b\n")];
    commit(&mut repo, 1, &[], "Ada", "base", &base);
    let ours = [("a.txt", Some("base\n"), "ours\n")];
    commit(&mut repo, 2, &[1], "Ada", "ours", &ours);
    let theirs = [("a.txt", Some("base\n"), "theirs\n")];
    commit(&mut repo, 3, &[1], "Ada", "theirs", &theirs);
    let added = [("n.txt", None, "new\n")];
    commit(&mut repo, 4, &[1], "Bea", "new", &added);
    let later = [("a.txt", Some("ours\n"), "later\n")];
    commit(&mut repo, 5, &[2], "Ada", "later", &later);
    commit(&mut repo, 6, &[1, 4], "Ada", "Merge branch 'feat'", &added);
    repo.set_branch("main", oid(2));
    repo.set_branch("other", oid(3));
    repo.set_branch("feat", oid(4));
    repo.set_branch("later", oid(5));
    repo.set_branch("merged", oid(6));
    repo.set_head_branch("main");
    repo
}

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn main_at(r: &Registry, s: &mut Session) -> String {
    run(r, s, "repo.status", json!({}))["head"]["Branch"]["target"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn status(r: &Registry, s: &mut Session) -> Value {
    run(r, s, "repo.status", json!({}))
}

#[test]
fn pick_commands_write() {
    let r = Registry::builtin();
    for id in [
        "commit.cherry_pick",
        "commit.revert",
        "cherry_pick.continue",
        "cherry_pick.abort",
        "revert.continue",
        "revert.abort",
    ] {
        assert!(r.get(id).unwrap().writes, "{id}");
    }
}

#[test]
fn cherry_pick_keeps_the_author_and_undo_redo() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "feat" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["commit"], oid(4).to_string());
    assert_eq!(out["before"], oid(2).to_string());
    assert_eq!(out["backup"], "refs/project/backup/cherry-pick-1");
    let after = out["after"].as_str().unwrap().to_owned();
    let show = run(&r, &mut s, "commit.show", json!({ "id": after }));
    assert_eq!(show["commit"]["summary"], "new");
    assert_eq!(show["commit"]["author"]["name"], "Bea");
    assert_eq!(show["commit"]["committer"]["name"], "Ada");
    assert_eq!(show["commit"]["parents"], json!([oid(2).to_string()]));
    assert_eq!(show["files"][0]["path"], "n.txt");
    assert_eq!(status(&r, &mut s)["entries"], json!([]));
    assert_eq!(
        out["next_undo"]["text"],
        format!(
            "Cherry-pick of feat onto `main` (from {} to {})",
            &oid(2).to_string()[..7],
            &after[..7]
        )
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(status(&r, &mut s)["entries"], json!([]));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(main_at(&r, &mut s), after);

    // Again: already applied, nothing changes and nothing stays in progress.
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "feat" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("would change nothing"), "{err}");
    assert_eq!(main_at(&r, &mut s), after);
    assert_eq!(status(&r, &mut s)["operation"], Value::Null);
}

#[test]
fn revert_writes_the_message_like_git() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "main" }));
    assert_eq!(out["backup"], "refs/project/backup/revert-1");
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["after"] }));
    assert_eq!(show["commit"]["summary"], "Revert \"ours\"");
    assert_eq!(
        show["commit"]["body"],
        format!("This reverts commit {}.", oid(2))
    );
    assert_eq!(show["commit"]["author"]["name"], "Ada");
    assert_eq!(show["files"][0]["path"], "a.txt");
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Revert of main onto `main`")
    );
    // The revert of the revert reapplies it; a second revert of the same commit is empty.
    let err = r
        .run(
            "commit.revert",
            &mut s,
            json!({ "rev": oid(2).to_string() }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("would change nothing"), "{err}");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
}

#[test]
fn conflict_stays_in_progress_then_continue_keeps_the_author() {
    let mut base = repo();
    // `other` by another author.
    let theirs = [("a.txt", Some("base\n"), "theirs\n")];
    commit(&mut base, 3, &[1], "Bea", "theirs", &theirs);
    let (r, mut s) = opened_with(base);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], oid(2).to_string());
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["status"]["operation"], "CherryPick");

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["incoming"][0]["id"], oid(3).to_string());
    assert_eq!(op["message"], "theirs");
    assert_eq!(op["continue"], "cherry_pick.continue");
    assert_eq!(op["abort"], "cherry_pick.abort");

    // During the operation: undo, commands of another kind and continue with conflicts are refused.
    for (id, params, text) in [
        ("edit.undo", json!({}), ""),
        ("commit.revert", json!({ "rev": "feat" }), "cherry-pick"),
        ("merge.continue", json!({}), "not a merge"),
        ("revert.continue", json!({}), "not a revert"),
        ("revert.abort", json!({ "confirm": true }), "not a revert"),
        ("cherry_pick.continue", json!({}), "`a.txt`"),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains(text), "{id}: {err}");
    }

    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    // `commit.create` does not conclude a cherry-pick: only `cherry_pick.continue`.
    let err = r
        .run("commit.create", &mut s, json!({ "message": "x" }))
        .unwrap_err();
    assert!(err.to_string().contains("cherry-pick"), "{err}");
    let out = run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(out["message"], "theirs");
    assert_eq!(out["commit"]["author"]["name"], "Bea");
    assert_eq!(out["commit"]["parents"], json!([oid(2).to_string()]));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Cherry-pick of other onto `main`")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(status(&r, &mut s)["entries"], json!([]));
}

#[test]
fn revert_conflict_and_abort_with_backup() {
    let mut base = repo();
    base.set_branch("main", oid(5));
    let (r, mut s) = opened_with(base);
    let err = r
        .run("revert.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(
        err.to_string().contains("a revert is not in progress"),
        "{err}"
    );

    let out = run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": oid(2).to_string() }),
    );
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Revert");
    assert_eq!(op["continue"], "revert.continue");
    assert_eq!(op["abort"], "revert.abort");
    assert_eq!(
        op["message"],
        format!("Revert \"ours\"\n\nThis reverts commit {}.", oid(2))
    );

    let err = r.run("revert.abort", &mut s, json!({})).unwrap_err();
    match err {
        EngineError::ConfirmRequired { reason, .. } => {
            assert!(reason.contains("`a.txt`"), "{reason}");
            assert!(reason.contains("revert-abort"), "{reason}");
        }
        other => panic!("{other}"),
    }
    assert_eq!(status(&r, &mut s)["operation"], "Revert");
    let out = run(&r, &mut s, "revert.abort", json!({ "confirm": true }));
    assert_eq!(out["backup"], "refs/project/backup/revert-abort-1");
    assert_eq!(out["files"], json!(["a.txt"]));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(out["can_undo"], false);
    assert_eq!(main_at(&r, &mut s), oid(5).to_string());
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["backup"] }));
    assert_eq!(show["commit"]["parents"].as_array().unwrap().len(), 2);

    // A stopped cherry-pick is aborted the same way.
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    let out = run(&r, &mut s, "cherry_pick.abort", json!({ "confirm": true }));
    assert_eq!(out["backup"], "refs/project/backup/cherry-pick-abort-1");
    assert_eq!(out["status"]["operation"], Value::Null);
}

#[test]
fn merge_commit_needs_mainline() {
    let (r, mut s) = opened_with(repo());
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "merged" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert!(err.to_string().contains("mainline"), "{err}");
    for params in [
        json!({ "rev": "merged", "mainline": 0 }),
        json!({ "rev": "merged", "mainline": 3 }),
        json!({ "rev": "feat", "mainline": 1 }),
    ] {
        let err = r
            .run("commit.cherry_pick", &mut s, params.clone())
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    let out = run(
        &r,
        &mut s,
        "commit.cherry_pick",
        json!({ "rev": "merged", "mainline": 1 }),
    );
    assert_eq!(out["mainline"], 1);
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["after"] }));
    assert_eq!(show["commit"]["parents"], json!([oid(2).to_string()]));
    assert_eq!(show["files"][0]["path"], "n.txt");
    // The revert of the merge relative to the first parent: git's message.
    let out = run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": "merged", "mainline": 1 }),
    );
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["after"] }));
    assert_eq!(
        show["commit"]["body"],
        format!(
            "This reverts commit {}, reversing\nchanges made to {}.",
            oid(6),
            oid(1)
        )
    );
}

#[test]
fn refusals() {
    let (r, mut s) = opened_with(repo());
    for params in [
        json!({ "rev": "" }),
        json!({ "rev": "feat", "no_commit": true }),
    ] {
        for id in ["commit.cherry_pick", "commit.revert"] {
            let err = r.run(id, &mut s, params.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{id} {params}: {err}"
            );
        }
    }
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "missing" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(
        err.to_string().contains("a cherry-pick is not in progress"),
        "{err}"
    );

    // Detached HEAD.
    let mut detached = repo();
    detached.detach_head(oid(2));
    let (r, mut s) = opened_with(detached);
    for id in ["commit.cherry_pick", "commit.revert"] {
        let err = r.run(id, &mut s, json!({ "rev": "feat" })).unwrap_err();
        assert!(err.to_string().contains("HEAD is detached"), "{id}: {err}");
    }

    // Any change to tracked files, even to files the commit does not touch.
    let mut dirty = repo();
    let (mode, b) = (0o100644, blob(&mut dirty, "b\n"));
    dirty.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("b.txt"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: EntryKind::Ordinary(project_domain::TrackedInfo {
            submodule: None,
            mode_head: mode,
            mode_index: mode,
            mode_worktree: mode,
            oid_head: b,
            oid_index: b,
        }),
    }]);
    dirty.set_worktree_file("b.txt", "changed\n");
    let (r, mut s) = opened_with(dirty);
    for id in ["commit.cherry_pick", "commit.revert"] {
        let err = r.run(id, &mut s, json!({ "rev": "feat" })).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains("`b.txt`"), "{id}: {err}");
        assert!(err.to_string().contains("stash.push"), "{id}: {err}");
    }
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
}

#[test]
fn continue_resolved_as_head_points_to_abort() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    let digest = shown["digest"].clone();
    // Resolved keeping HEAD's version: nothing to commit.
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": digest, "content": "ours\n" }),
    );
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("cherry_pick.abort"), "{err}");
    assert_eq!(status(&r, &mut s)["operation"], "CherryPick");
}

/// The typed results read back the JSON of the commands and give the same JSON again: the UI
/// deserializes into them. `*.continue` and `*.abort` have the shape of `merge.continue` and
/// `merge.abort`.
#[test]
fn results_round_trip() {
    use crate::{MergeAbortResult, MergeContinueResult, PickStartResult};
    // Concluded right away.
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "feat" }));
    assert!(out.get("refs_error").is_none() && out.get("status_error").is_none());
    let done: PickStartResult = same(out);
    assert_eq!(done.rev, "feat");
    assert_eq!(done.commit, oid(4));
    assert_eq!(done.summary, "new");
    assert_eq!(done.mainline, None);
    assert_eq!(done.branch, "main");
    assert_eq!(done.before, oid(2));
    assert!(!done.in_progress && done.journal.can_undo);
    assert_eq!(done.backup, "refs/project/backup/cherry-pick-1");
    let merged: PickStartResult = same(run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": "merged", "mainline": 1 }),
    ));
    assert_eq!(merged.mainline, Some(1));

    // Cherry-pick stopped, then continued.
    let (r, mut s) = opened_with(repo());
    let stopped: PickStartResult = same(run(
        &r,
        &mut s,
        "commit.cherry_pick",
        json!({ "rev": "other" }),
    ));
    assert!(stopped.in_progress && !stopped.journal.can_undo);
    assert_eq!(stopped.conflicts, vec![RepoPath::from("a.txt")]);
    assert_eq!(stopped.after, oid(2));
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let out = run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert!(out.get("refs_error").is_none() && out.get("status_error").is_none());
    let done: MergeContinueResult = same(out);
    assert_eq!(done.message, "theirs");
    assert_eq!(done.previous, Some(oid(2)));
    assert_eq!(done.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(done.backup, None);
    assert!(done.also_on.is_empty() && done.journal.can_undo);

    // Revert stopped, continued with a message; another one stopped and aborted.
    let mut base = repo();
    base.set_branch("main", oid(5));
    let (r, mut s) = opened_with(base);
    run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": oid(2).to_string() }),
    );
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let done: MergeContinueResult = same(run(
        &r,
        &mut s,
        "revert.continue",
        json!({ "message": "Undo ours" }),
    ));
    assert_eq!(done.message, "Undo ours");
    assert_eq!(done.commit.parents, vec![oid(5)]);
    run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "other" }));
    let aborted: MergeAbortResult = same(run(
        &r,
        &mut s,
        "cherry_pick.abort",
        json!({ "confirm": true }),
    ));
    assert_eq!(aborted.backup, "refs/project/backup/cherry-pick-abort-1");
    assert_eq!(aborted.files, vec![RepoPath::from("a.txt")]);
    assert!(aborted.status.is_some_and(|s| s.operation.is_none()));
}

/// A re-read that fails gives `null` and `<key>_error`.
#[test]
fn reread_error_fields() {
    use super::super::{JournalState, reread};
    use crate::{Message, PickStartResult};
    use project_domain::{RefInfo, Status};
    let error = EngineError::NoRepo.to_string();
    let (status, status_error) = reread::<Status>(Err(EngineError::NoRepo));
    let (refs, refs_error) = reread::<Vec<RefInfo>>(Err(EngineError::NoRepo));
    let result = PickStartResult {
        rev: "feat".into(),
        commit: oid(4),
        summary: "new".into(),
        mainline: Some(2),
        branch: "main".into(),
        before: oid(2),
        after: oid(9),
        in_progress: false,
        conflicts: vec![],
        message: String::new(),
        backup: "refs/project/backup/cherry-pick-1".into(),
        warnings: vec![Message::plain("w")],
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&result).unwrap();
    for key in ["status", "refs", "next_undo", "next_redo"] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(out["mainline"], 2);
    assert_eq!(out["status_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    let back: PickStartResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);
}
