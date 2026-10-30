//! Tests of `branch.reset` and `branch.reset_preview` on the `MemoryBackend` sample (with real
//! git: `crates/git/tests/reset.rs`).
//!
//! The sample: `main` at `c12` (it renames `src/main.rs` to `src/bin/app.rs`), `origin/main` at
//! `c11`; `README.md` changed in the stage and again outside it, `docs/guide.md` new in the
//! stage, `src/lib.rs` changed outside it, `note.txt` untracked.

use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::{Change, EntryKind, Oid, Operation, RepoPath, StatusEntry};
use serde_json::{Value, json};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
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
    let refs = run(r, s, "repo.refs", json!({}));
    refs.as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "refs/heads/main")
        .map(|x| x["target"].as_str().unwrap().to_owned())
        .unwrap()
}

fn tracked_changes(out: &Value) -> Vec<String> {
    out["status"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["index"] != "Untracked")
        .map(|e| e["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn soft_moves_only_the_ref_and_undo_puts_it_back() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": oid(11).to_string(), "mode": "soft" }),
    );
    assert_eq!(out["branch"], "refs/heads/main");
    assert_eq!(out["before"], oid(12).to_string());
    assert_eq!(out["after"], oid(11).to_string());
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["index_backup"], Value::Null);
    // `c12` is not on `origin/main`: no warning.
    assert_eq!(out["published"], 0);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(main_at(&r, &mut s), oid(11).to_string());
    assert!(out["can_undo"].as_bool().unwrap());
    // The worktree changes are still there.
    assert!(tracked_changes(&out).contains(&"src/lib.rs".to_owned()));

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "branch.reset");
    assert_eq!(main_at(&r, &mut s), oid(12).to_string());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(11).to_string());
}

#[test]
fn mixed_saves_the_index_and_warns_about_published_commits() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": oid(10).to_string(), "mode": "mixed" }),
    );
    // The staged version of README.md existed only in the index.
    assert!(
        out["index_backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/index-"),
        "{out}"
    );
    // `c11` leaves `main` and is on `origin/main`.
    assert_eq!(out["published"], 1);
    let warnings = out["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    assert_eq!(warnings[0]["key"], "engine-reset-published");
    // Nothing staged afterwards.
    for e in out["status"]["entries"].as_array().unwrap() {
        assert!(
            e["index"] == "Unmodified" || e["index"] == "Untracked",
            "{e}"
        );
    }
    // Undo: the ref and the index go back, only if the index was not touched since.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(12).to_string());
}

#[test]
fn hard_asks_for_confirm_saves_the_changes_and_undo_says_where_they_are() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let preview = run(
        &r,
        &mut s,
        "branch.reset_preview",
        json!({ "target": oid(11).to_string(), "mode": "hard" }),
    );
    assert_eq!(preview["needs_confirm"], true);
    assert_eq!(
        preview["files"],
        json!(["README.md", "docs/guide.md", "src/lib.rs"])
    );
    assert_eq!(preview["expected"], oid(12).to_string());
    assert_eq!(preview["after"], oid(11).to_string());

    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": oid(11).to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    let EngineError::ConfirmRequired {
        expected, reason, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(expected.as_deref(), Some(oid(12).to_string().as_str()));
    assert!(reason.to_string().contains("src/lib.rs"), "{reason}");
    assert_eq!(main_at(&r, &mut s), oid(12).to_string(), "nothing changed");

    // `expected` from another commit: refused even with the confirmation.
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": oid(11).to_string(), "mode": "hard", "confirm": true, "expected": oid(3).to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": oid(11).to_string(), "mode": "hard", "confirm": true, "expected": oid(12).to_string() }),
    );
    assert_eq!(out["backup"], "refs/project/backup/reset-1");
    assert_eq!(out["files"], preview["files"]);
    assert!(out["stash"].is_string());
    // Only the untracked file is left.
    assert_eq!(tracked_changes(&out), Vec::<String>::new());
    assert!(
        out["status"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "note.txt")
    );

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(12).to_string());
    let warnings = undo["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w["key"] == "engine-reset-undo-backup"),
        "{undo}"
    );
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(main_at(&r, &mut s), oid(11).to_string());
}

#[test]
fn hard_without_changes_needs_no_confirm_and_detached_head_works() {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo.detach_head(oid(12));
    let (r, mut s) = opened_with(repo);
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": oid(10).to_string(), "mode": "hard" }),
    );
    assert_eq!(out["branch"], Value::Null);
    assert_eq!(out["backup"], Value::Null);
    let head = run(&r, &mut s, "repo.status", json!({}));
    assert_eq!(head["head"]["Detached"], oid(10).to_string(), "{head}");
    run(&r, &mut s, "edit.undo", json!({}));
    let head = run(&r, &mut s, "repo.status", json!({}));
    assert_eq!(head["head"]["Detached"], oid(12).to_string());
    // `main` did not move.
    assert_eq!(main_at(&r, &mut s), oid(12).to_string());
}

#[test]
fn untracked_file_in_the_way_blocks_a_hard_reset() {
    let mut repo = MemoryBackend::sample();
    // `c11` still has `src/main.rs` (renamed by `c12`).
    repo.set_entries(vec![StatusEntry {
        path: RepoPath::from("src/main.rs"),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    }]);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": oid(11).to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    let EngineError::NotAllowed(m) = &err else {
        panic!("{err}");
    };
    assert_eq!(m.key(), "engine-reset-in-the-way");
    assert_eq!(main_at(&r, &mut s), oid(12).to_string());
}

#[test]
fn refusals() {
    // Operation in progress.
    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(Operation::Merge));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": oid(11).to_string(), "mode": "soft" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // Branch without commits.
    let (r, mut s) = opened_with(MemoryBackend::new());
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": "HEAD", "mode": "soft" }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-unborn"),
        "{err}"
    );

    let (r, mut s) = opened_with(MemoryBackend::sample());
    // A rev that does not exist.
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": "nope", "mode": "soft" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    // Bad parameters.
    for params in [
        json!({ "target": "", "mode": "soft" }),
        json!({ "target": "HEAD", "mode": "keep" }),
        json!({ "target": "HEAD" }),
        json!({ "target": "HEAD", "mode": "soft", "force": true }),
        json!({ "target": "HEAD", "mode": "soft", "expected": "abc" }),
    ] {
        let err = r.run("branch.reset", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    // Intent-to-add files block a hard reset.
    let mut repo = MemoryBackend::sample();
    let mut entries = repo_entries(&repo);
    if let Some(e) = entries
        .iter_mut()
        .find(|e| e.path == RepoPath::from("docs/guide.md"))
    {
        e.index = Change::Unmodified;
        e.worktree = Change::Added;
        if let EntryKind::Ordinary(info) = &mut e.kind {
            info.mode_index = 0;
        }
    }
    repo.set_entries(entries);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "branch.reset_preview",
            &mut s,
            json!({ "target": oid(11).to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-intent-to-add"),
        "{err}"
    );
}

fn repo_entries(repo: &MemoryBackend) -> Vec<StatusEntry> {
    use crate::GitRead;
    repo.status().unwrap().entries
}

fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: Value) -> T {
    let typed: T = serde_json::from_value(out.clone()).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), out);
    typed
}

/// The typed results read back the JSON of the commands and give the same JSON again: the UI
/// deserializes into them.
#[test]
fn results_round_trip() {
    use crate::{ResetMode, ResetPreviewResult, ResetResult};
    // `mixed` with published commits: preview, then the reset.
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let params = json!({ "target": oid(10).to_string(), "mode": "mixed" });
    let preview: ResetPreviewResult = same(run(&r, &mut s, "branch.reset_preview", params.clone()));
    assert_eq!(preview.mode, ResetMode::Mixed);
    assert_eq!(preview.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!((preview.before, preview.after), (oid(12), oid(10)));
    assert_eq!(preview.expected, oid(12));
    assert!(preview.files.is_empty() && !preview.needs_confirm);
    assert_eq!(preview.published, 1);
    assert_eq!(
        preview.upstream.as_deref(),
        Some("refs/remotes/origin/main")
    );
    assert_eq!(preview.warnings.len(), 1);
    let out = run(&r, &mut s, "branch.reset", params);
    assert!(out.get("refs_error").is_none() && out.get("status_error").is_none());
    let done: ResetResult = same(out);
    assert_eq!(done.mode, ResetMode::Mixed);
    assert!(
        done.index_backup
            .as_deref()
            .is_some_and(|b| b.starts_with("refs/project/backup/index-"))
    );
    assert_eq!((done.backup, done.stash), (None, None));
    assert_eq!(done.published, 1);
    assert!(done.journal.can_undo && done.status.is_some() && done.refs.is_some());

    // `hard` with local changes: preview, then the confirmed reset with its backup.
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let target = oid(11).to_string();
    let preview: ResetPreviewResult = same(run(
        &r,
        &mut s,
        "branch.reset_preview",
        json!({ "target": target, "mode": "hard" }),
    ));
    assert!(preview.needs_confirm);
    assert_eq!(preview.files.len(), 3);
    assert_eq!((preview.published, preview.upstream), (0, None));
    let done: ResetResult = same(run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": target, "mode": "hard", "confirm": true, "expected": preview.expected.to_string() }),
    ));
    assert_eq!(done.backup.as_deref(), Some("refs/project/backup/reset-1"));
    assert!(done.stash.is_some());
    assert_eq!(done.files, preview.files);

    // Detached HEAD: `branch` is `null`.
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo.detach_head(oid(12));
    let (r, mut s) = opened_with(repo);
    let done: ResetResult = same(run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": oid(10).to_string(), "mode": "soft" }),
    ));
    assert_eq!(done.branch, None);
}

/// A re-read that fails gives `null` and `<key>_error`.
#[test]
fn reread_error_fields() {
    use super::super::{JournalState, reread};
    use crate::{Message, ResetMode, ResetResult};
    use project_domain::{RefInfo, Status};
    let error = EngineError::NoRepo.to_string();
    let (status, status_error) = reread::<Status>(Err(EngineError::NoRepo));
    let (refs, refs_error) = reread::<Vec<RefInfo>>(Err(EngineError::NoRepo));
    let result = ResetResult {
        mode: ResetMode::Hard,
        branch: None,
        before: oid(12),
        after: oid(11),
        files: vec![RepoPath::from("a.txt")],
        backup: Some("refs/project/backup/reset-1".into()),
        stash: Some(oid(9)),
        index_backup: None,
        backups: vec![],
        published: 0,
        warnings: vec![Message::plain("w")],
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&result).unwrap();
    for key in ["status", "refs", "branch", "index_backup", "next_undo"] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(out["mode"], "hard");
    assert_eq!(out["stash"], oid(9).to_string());
    assert_eq!(out["status_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    let back: ResetResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);
}
