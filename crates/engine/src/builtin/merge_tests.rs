//! Tests of `merge.start`, `merge.continue`, `merge.abort` and `repo.operation` on the
//! `MemoryBackend` (with real git: `crates/git/tests/merge.rs`).
//!
//! The repo: `c1` (root: `a.txt`, `b.txt`); `main` at `c2` (`a.txt` = "ours"); `other` at
//! `c3` from `c1` (`a.txt` = "theirs": conflict with `main`); `feat` at `c4` from `c1` (adds
//! `n.txt`: merges without conflicts); `ahead` at `c5` from `c2` (adds `c.txt`:
//! fast-forward). HEAD on `main`, clean worktree.

use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::{Change, ChangedFile, Commit, Oid, RepoPath, Signature};
use serde_json::{Value, json};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn sig(n: i64) -> Signature {
    Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000 + n * 60,
        offset_min: 0,
    }
}

/// A blob with `content` (id from the content).
fn blob(repo: &mut MemoryBackend, content: &str) -> Oid {
    let hash = content.bytes().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    });
    let id = oid(hash);
    repo.set_blob(id, content);
    id
}

/// Commit `n` with parent `parent` and the files `files` (path, content before if
/// there was any, content after).
fn commit(
    repo: &mut MemoryBackend,
    n: u64,
    parent: Option<u64>,
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
        parents: parent.map(oid).into_iter().collect(),
        author: sig(n as i64),
        committer: sig(n as i64),
        summary: summary.into(),
        body: String::new(),
    });
    repo.set_changes(oid(n), changes);
}

fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(
        &mut repo,
        1,
        None,
        "base",
        &[("a.txt", None, "base\n"), ("b.txt", None, "b\n")],
    );
    commit(
        &mut repo,
        2,
        Some(1),
        "ours",
        &[("a.txt", Some("base\n"), "ours\n")],
    );
    commit(
        &mut repo,
        3,
        Some(1),
        "theirs",
        &[("a.txt", Some("base\n"), "theirs\n")],
    );
    commit(&mut repo, 4, Some(1), "new", &[("n.txt", None, "new\n")]);
    commit(&mut repo, 5, Some(2), "ahead", &[("c.txt", None, "c\n")]);
    repo.set_branch("main", oid(2));
    repo.set_branch("other", oid(3));
    repo.set_branch("feat", oid(4));
    repo.set_branch("ahead", oid(5));
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

fn entries(r: &Registry, s: &mut Session) -> usize {
    run(r, s, "repo.status", json!({}))["entries"]
        .as_array()
        .unwrap()
        .len()
}

#[test]
fn merge_commands_write() {
    let r = Registry::builtin();
    for id in ["merge.start", "merge.continue", "merge.abort"] {
        assert!(r.get(id).unwrap().writes, "{id}");
    }
    assert!(!r.get("repo.operation").unwrap().writes);
}

#[test]
fn fast_forward_and_undo() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(out["after"], oid(5).to_string());
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["backup"], "refs/project/backup/merge-1");
    assert_eq!(out["source"]["kind"], "branch");
    assert_eq!(
        out["next_undo"]["text"],
        format!(
            "Merge of ahead into `main` (fast-forward: from {} to {})",
            &oid(2).to_string()[..7],
            &oid(5).to_string()[..7]
        )
    );
    assert_eq!(entries(&r, &mut s), 0);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(entries(&r, &mut s), 0);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(5).to_string());

    // Already contained: nothing to do, nothing in the journal.
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "main" }));
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(out["backup"], Value::Null);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"].as_array().unwrap().len(), 1);
}

#[test]
fn merge_commit_without_conflicts() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "feat" }));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(
        (out["ahead"].as_u64(), out["behind"].as_u64()),
        (Some(1), Some(1))
    );
    let merge = out["after"].as_str().unwrap().to_owned();
    let show = run(&r, &mut s, "commit.show", json!({ "id": merge }));
    assert_eq!(show["commit"]["summary"], "Merge branch 'feat'");
    assert_eq!(
        show["commit"]["parents"],
        json!([oid(2).to_string(), oid(4).to_string()])
    );
    assert_eq!(show["files"][0]["path"], "n.txt");
    assert_eq!(entries(&r, &mut s), 0);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(entries(&r, &mut s), 0);

    // `no_ff` even when a fast-forward would be possible; the chosen message.
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "no_ff": true, "message": "Merge ahead" }),
    );
    assert_eq!(out["strategy"], "merge");
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["after"] }));
    assert_eq!(show["commit"]["summary"], "Merge ahead");
    assert_eq!(show["commit"]["parents"].as_array().unwrap().len(), 2);
}

#[test]
fn conflict_stays_in_progress_then_continue() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], oid(2).to_string());
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["status"]["operation"], "Merge");
    assert!(out["message"].as_str().unwrap().contains("a.txt"));

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Merge");
    assert_eq!(op["conflicts"], json!(["a.txt"]));
    assert_eq!(op["incoming"][0]["id"], oid(3).to_string());
    assert_eq!(op["incoming"][0]["label"], "other");
    assert_eq!(op["incoming"][0]["summary"], "theirs");
    assert_eq!(op["incoming"][0]["refs"], json!(["refs/heads/other"]));
    assert_eq!(op["message"], "Merge branch 'other'");
    assert_eq!(op["ready"], false);
    assert_eq!(op["continue"], "merge.continue");
    assert_eq!(op["abort"], "merge.abort");

    // During the merge: undo, another merge and the commit with conflicts are refused.
    for (id, params) in [
        ("edit.undo", json!({})),
        ("merge.start", json!({ "rev": "feat" })),
        ("merge.continue", json!({})),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }
    let err = r.run("merge.continue", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");

    // Resolved (the worktree content, with the markers) and staged: it concludes.
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);
    let out = run(&r, &mut s, "merge.continue", json!({}));
    assert_eq!(out["message"], "Merge branch 'other'");
    assert_eq!(
        out["commit"]["parents"],
        json!([oid(2).to_string(), oid(3).to_string()])
    );
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(out["can_undo"], true);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Merge of other into `main` (merge commit")
    );
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], Value::Null);
    assert_eq!(op["continue"], Value::Null);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(entries(&r, &mut s), 0);
}

#[test]
fn commit_create_concludes_a_merge_too() {
    let (r, mut s) = opened_with(repo());
    run(&r, &mut s, "merge.start", json!({ "rev": "other" }));
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let out = run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Risolto a mano" }),
    );
    assert_eq!(out["commit"]["parents"].as_array().unwrap().len(), 2);
    assert_eq!(out["status"]["operation"], Value::Null);
}

#[test]
fn abort_needs_confirm_and_saves_a_backup() {
    let (r, mut s) = opened_with(repo());
    let err = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(
        err.to_string().contains("a merge is not in progress"),
        "{err}"
    );

    run(&r, &mut s, "merge.start", json!({ "rev": "other" }));
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    match err {
        EngineError::ConfirmRequired { reason, .. } => {
            assert!(reason.contains("`a.txt`"), "{reason}");
            assert!(reason.contains("merge-abort"), "{reason}");
        }
        other => panic!("{other}"),
    }
    assert_eq!(
        run(&r, &mut s, "repo.status", json!({}))["operation"],
        "Merge"
    );
    let out = run(&r, &mut s, "merge.abort", json!({ "confirm": true }));
    assert_eq!(out["backup"], "refs/project/backup/merge-abort-1");
    assert_eq!(out["files"], json!(["a.txt"]));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(out["can_undo"], false);
    // The backup has the shape of a stash (parents HEAD and the index commit), with the file
    // as it was (the markers).
    let show = run(&r, &mut s, "commit.show", json!({ "id": out["backup"] }));
    assert_eq!(show["commit"]["parents"][0], oid(2).to_string());
    assert_eq!(show["commit"]["parents"].as_array().unwrap().len(), 2);
    assert_eq!(show["files"][0]["path"], "a.txt");
    // After the abort the merge can be redone.
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "other" }));
    assert_eq!(out["in_progress"], true);
}

#[test]
fn refusals() {
    let (r, mut s) = opened_with(repo());
    for params in [
        json!({ "rev": "feat", "no_ff": true, "ff_only": true }),
        json!({ "rev": "" }),
        json!({ "rev": "feat", "message": "  " }),
        json!({ "rev": "feat", "squash": true, "no_ff": true }),
        json!({ "rev": "feat", "squash": true, "ff_only": true }),
        json!({ "rev": "feat", "squash": true, "bogus": true }),
    ] {
        let err = r.run("merge.start", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "missing" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    let err = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "feat", "ff_only": true }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("ff_only"), "{err}");
    let err = r.run("merge.continue", &mut s, json!({})).unwrap_err();
    assert!(
        err.to_string().contains("a merge is not in progress"),
        "{err}"
    );

    // Detached HEAD.
    let mut detached = repo();
    detached.detach_head(oid(2));
    let (r, mut s) = opened_with(detached);
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "feat" }))
        .unwrap_err();
    assert!(err.to_string().contains("HEAD is detached"), "{err}");

    // Local changes to tracked files: no merge commit; the fast-forward yes, if it does not
    // touch those files.
    let mut dirty = repo();
    let (mode, b) = (0o100644, oid_of_b(&mut dirty));
    dirty.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("b.txt"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: project_domain::EntryKind::Ordinary(project_domain::TrackedInfo {
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
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "feat" }))
        .unwrap_err();
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert!(err.to_string().contains("autostash"), "{err}");
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "ahead" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(entries(&r, &mut s), 1);
}

fn oid_of_b(repo: &mut MemoryBackend) -> Oid {
    blob(repo, "b\n")
}

/// The typed results read back the JSON of the commands and give the same JSON again: the UI
/// deserializes into them.
#[test]
fn results_round_trip() {
    use crate::{
        MergeAbortResult, MergeApplied, MergeContinueResult, MergeSourceKind, MergeStartResult,
        RepoOperationResult,
    };
    use project_domain::Operation;
    fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: Value) -> T {
        let typed: T = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        typed
    }
    fn start(r: &Registry, s: &mut Session, params: Value) -> MergeStartResult {
        let out = run(r, s, "merge.start", params);
        assert!(out.get("refs_error").is_none() && out.get("status_error").is_none());
        same(out)
    }
    // Up to date: no backup.
    let (r, mut s) = opened_with(repo());
    let up = start(&r, &mut s, json!({ "rev": "main" }));
    assert_eq!(up.strategy, MergeApplied::UpToDate);
    assert_eq!(up.backup, None);
    assert_eq!(up.after, up.before);
    // Fast-forward.
    let ff = start(&r, &mut s, json!({ "rev": "ahead" }));
    assert_eq!(ff.strategy, MergeApplied::FastForward);
    assert_eq!(ff.source.kind, MergeSourceKind::Branch);
    assert!(ff.journal.can_undo);
    // Merge commit.
    let (r, mut s) = opened_with(repo());
    let merged = start(&r, &mut s, json!({ "rev": "feat" }));
    assert_eq!(merged.strategy, MergeApplied::Merge);
    assert!(!merged.in_progress && merged.backup.is_some());
    // Stopped on a conflict, then continue.
    let (r, mut s) = opened_with(repo());
    let stopped = start(&r, &mut s, json!({ "rev": "other" }));
    assert_eq!(stopped.strategy, MergeApplied::Merge);
    assert!(stopped.in_progress);
    assert_eq!(stopped.conflicts, vec![RepoPath::from("a.txt")]);
    let op: RepoOperationResult = same(run(&r, &mut s, "repo.operation", json!({})));
    assert_eq!(op.operation, Some(Operation::Merge));
    assert_eq!(op.conflicts, vec![RepoPath::from("a.txt")]);
    assert_eq!(op.incoming.len(), 1);
    assert_eq!(op.incoming[0].id, oid(3));
    assert_eq!(op.incoming[0].label, "other");
    assert_eq!(op.incoming[0].summary.as_deref(), Some("theirs"));
    assert_eq!(op.incoming[0].refs, vec!["refs/heads/other".to_string()]);
    assert_eq!(op.message.as_deref(), Some("Merge branch 'other'"));
    assert!(!op.ready);
    assert_eq!(op.continue_command.as_deref(), Some("merge.continue"));
    assert_eq!(op.abort_command.as_deref(), Some("merge.abort"));
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "both\n" }),
    );
    let out = run(&r, &mut s, "merge.continue", json!({}));
    let done: MergeContinueResult = same(out);
    assert_eq!(done.message, "Merge branch 'other'");
    assert_eq!(done.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(done.backup, None);
    assert!(done.journal.can_undo);
    let none: RepoOperationResult = same(run(&r, &mut s, "repo.operation", json!({})));
    assert_eq!(none.operation, None);
    assert!(none.incoming.is_empty() && none.conflicts.is_empty() && !none.ready);
    assert_eq!((none.continue_command, none.abort_command), (None, None));
    // Stopped, then abort.
    let (r, mut s) = opened_with(repo());
    start(&r, &mut s, json!({ "rev": "other" }));
    let aborted: MergeAbortResult =
        same(run(&r, &mut s, "merge.abort", json!({ "confirm": true })));
    assert_eq!(aborted.backup, "refs/project/backup/merge-abort-1");
    assert_eq!(aborted.files, vec![RepoPath::from("a.txt")]);
}

/// A re-read that fails gives `null` and `<key>_error`.
#[test]
fn reread_error_fields() {
    use super::super::{JournalState, reread};
    use crate::{
        MergeAbortResult, MergeApplied, MergeContinueResult, MergeSource, MergeSourceKind,
        MergeStartResult, Message,
    };
    use project_domain::{RefInfo, Status};
    let error = EngineError::NoRepo.to_string();
    let (status, status_error) = reread::<Status>(Err(EngineError::NoRepo));
    let (refs, refs_error) = reread::<Vec<RefInfo>>(Err(EngineError::NoRepo));
    let result = MergeStartResult {
        rev: "other".into(),
        source: MergeSource {
            kind: MergeSourceKind::Commit,
            name: oid(3).to_string(),
            full: None,
        },
        target: oid(3),
        branch: "main".into(),
        strategy: MergeApplied::UpToDate,
        before: oid(2),
        after: oid(2),
        ahead: 1,
        behind: 0,
        in_progress: false,
        conflicts: vec![],
        message: String::new(),
        backup: None,
        warnings: vec![Message::plain("w")],
        status: status.clone(),
        status_error: status_error.clone(),
        refs: refs.clone(),
        refs_error: refs_error.clone(),
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&result).unwrap();
    for key in ["status", "refs", "backup", "next_undo"] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(out["status_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(out["source"]["kind"], "commit");
    let back: MergeStartResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);

    let result = MergeContinueResult {
        commit: Commit {
            id: oid(9),
            parents: vec![oid(2), oid(3)],
            author: sig(9),
            committer: sig(9),
            summary: "Merge".into(),
            body: String::new(),
        },
        previous: Some(oid(2)),
        branch: None,
        backup: None,
        also_on: vec![],
        message: "Merge".into(),
        status: status.clone(),
        status_error: status_error.clone(),
        refs: refs.clone(),
        refs_error: refs_error.clone(),
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&result).unwrap();
    assert_eq!(out["branch"], Value::Null);
    assert_eq!(out["status_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    let back: MergeContinueResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);

    let result = MergeAbortResult {
        backup: "refs/project/backup/merge-abort-1".into(),
        files: vec![],
        warnings: vec![],
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&result).unwrap();
    assert_eq!(out["status"], Value::Null);
    assert_eq!(out["refs_error"], error.as_str());
    let back: MergeAbortResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);
}

#[test]
fn squash_of_a_diverged_branch_undo_and_redo() {
    let (r, mut s) = opened_with(repo());
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "feat", "squash": true }),
    );
    assert_eq!(out["strategy"], "squash");
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["backup"], "refs/project/backup/merge-1");
    let after = out["after"].as_str().unwrap().to_owned();
    let log = run(&r, &mut s, "repo.log", json!({ "limit": 1 }));
    assert_eq!(log["commits"][0]["parents"], json!([oid(2).to_string()]));
    assert_eq!(log["commits"][0]["summary"], "Squashed branch 'feat'");
    assert_eq!(log["commits"][0]["body"], "* new");
    assert_eq!(out["status"]["operation"], Value::Null);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .starts_with("Merge of feat into `main` (squash into one commit"),
        "{}",
        out["next_undo"]
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(main_at(&r, &mut s), after);
    // The branch is not recorded as merged: squashing it again finds it not up to date.
    let out = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "feat", "squash": true }),
        )
        .unwrap();
    assert_ne!(out["strategy"], "up_to_date");
}

#[test]
fn squash_of_a_branch_ahead_message_and_up_to_date() {
    let (r, mut s) = opened_with(repo());
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "ahead", "squash": true, "message": "All of it" }),
    );
    assert_eq!(out["strategy"], "squash");
    let log = run(&r, &mut s, "repo.log", json!({ "limit": 1 }));
    // No fast-forward: a new commit on `main`, not `ahead`.
    assert_ne!(log["commits"][0]["id"], oid(5).to_string());
    assert_eq!(log["commits"][0]["parents"], json!([oid(2).to_string()]));
    assert_eq!(log["commits"][0]["summary"], "All of it");
    // Already contained: nothing written.
    let entries_before = run(&r, &mut s, "edit.history", json!({}));
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": oid(1).to_string(), "squash": true }),
    );
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(run(&r, &mut s, "edit.history", json!({})), entries_before);
}

#[test]
fn squash_conflict_stays_in_progress_then_continue() {
    let (r, mut s) = opened_with(repo());
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "other", "squash": true }),
    );
    assert_eq!(out["strategy"], "squash");
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], oid(2).to_string());
    assert_eq!(out["status"]["operation"], "Squash");
    assert_eq!(out["can_undo"], false);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Squash");
    assert_eq!(op["incoming"][0]["label"], "other");
    assert_eq!(op["message"], "Squashed branch 'other'\n\n* theirs");
    assert_eq!(op["continue"], "merge.continue");
    assert_eq!(op["abort"], "merge.abort");
    // During the squash: undo, another merge, a checkout, amend, continue with conflicts.
    for (id, params) in [
        ("edit.undo", json!({})),
        ("merge.start", json!({ "rev": "feat" })),
        ("branch.checkout", json!({ "name": "feat" })),
        ("merge.continue", json!({})),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let err = r
        .run(
            "commit.create",
            &mut s,
            json!({ "message": "x", "amend": true, "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let out = run(&r, &mut s, "merge.continue", json!({}));
    assert_eq!(out["message"], "Squashed branch 'other'\n\n* theirs");
    assert_eq!(out["commit"]["parents"], json!([oid(2).to_string()]));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert!(
        out["next_undo"]["text"]
            .as_str()
            .unwrap()
            .contains("squash into one commit")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
    assert_eq!(entries(&r, &mut s), 0);
}

#[test]
fn squash_abort_needs_confirm_and_saves_a_backup() {
    let (r, mut s) = opened_with(repo());
    run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "other", "squash": true }),
    );
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert_eq!(
        run(&r, &mut s, "repo.operation", json!({}))["operation"],
        "Squash"
    );
    let out = run(&r, &mut s, "merge.abort", json!({ "confirm": true }));
    assert!(
        out["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/merge-abort-")
    );
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(entries(&r, &mut s), 0);
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
}

#[test]
fn squash_refuses_local_changes() {
    let mut dirty = repo();
    let (mode, b) = (0o100644, oid_of_b(&mut dirty));
    dirty.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("b.txt"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: project_domain::EntryKind::Ordinary(project_domain::TrackedInfo {
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
    // Even when the branch is ahead (a fast-forward would accept the change).
    let err = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "ahead", "squash": true }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(main_at(&r, &mut s), oid(2).to_string());
}
