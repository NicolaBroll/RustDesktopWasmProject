//! Tests of `conflict.show` and `conflict.resolve` on the `MemoryBackend` (with real git:
//! `crates/git/tests/conflict.rs`).
//!
//! The repo: `c1` (root: `a.txt` = "base", `b.txt`); `main` at `c2` (`a.txt` = "ours");
//! `other` at `c3` from `c1` (`a.txt` = "theirs": `BothModified` conflict); `gone` at `c4` from `c1`
//! (deletes `a.txt`: `DeletedByThem` conflict). HEAD on `main`, clean worktree.

use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session, worktree_digest};
use project_domain::{
    Change, ChangedFile, Commit, Conflict, EntryKind, Oid, RepoPath, Signature, StatusEntry,
    UnmergedInfo,
};
use serde_json::{Value, json};
use std::sync::Arc;

const ZERO: Oid = Oid::from_bytes([0; Oid::LEN]);

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

fn blob(repo: &mut MemoryBackend, content: &[u8]) -> Oid {
    let hash = content.iter().fold(1_469_598_103_934_665_603_u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    });
    let id = oid(hash);
    repo.set_blob(id, content);
    id
}

/// Commit `n` with the files `files` (path, content before, content after; `None` = absent).
fn commit(
    repo: &mut MemoryBackend,
    n: u64,
    parent: Option<u64>,
    files: &[(&str, Option<&str>, Option<&str>)],
) {
    let changes = files
        .iter()
        .map(|(path, old, new)| {
            let old_oid = old.map(|c| blob(repo, c.as_bytes()));
            let new_oid = new.map(|c| blob(repo, c.as_bytes()));
            ChangedFile {
                path: RepoPath::from(*path),
                change: match (old, new) {
                    (None, _) => Change::Added,
                    (_, None) => Change::Deleted,
                    _ => Change::Modified,
                },
                original: None,
                score: None,
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: if new.is_some() { 0o100644 } else { 0 },
                old_oid: old_oid.unwrap_or(ZERO),
                new_oid: new_oid.unwrap_or(ZERO),
            }
        })
        .collect();
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parent.map(oid).into_iter().collect(),
        author: sig(n as i64),
        committer: sig(n as i64),
        summary: format!("c{n}"),
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
        &[
            ("a.txt", None, Some("base\n")),
            ("b.txt", None, Some("b\n")),
        ],
    );
    commit(
        &mut repo,
        2,
        Some(1),
        &[("a.txt", Some("base\n"), Some("ours\n"))],
    );
    commit(
        &mut repo,
        3,
        Some(1),
        &[("a.txt", Some("base\n"), Some("theirs\n"))],
    );
    commit(&mut repo, 4, Some(1), &[("a.txt", Some("base\n"), None)]);
    repo.set_branch("main", oid(2));
    repo.set_branch("other", oid(3));
    repo.set_branch("gone", oid(4));
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

/// The repo with the merge of `rev` stopped on a conflict.
fn merging(rev: &str) -> (Registry, Session) {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "merge.start", json!({ "rev": rev }));
    assert_eq!(out["in_progress"], true);
    (r, s)
}

fn status(r: &Registry, s: &mut Session) -> Value {
    run(r, s, "repo.status", json!({}))
}

#[test]
fn only_resolve_writes() {
    let r = Registry::builtin();
    assert!(r.get("conflict.resolve").unwrap().writes);
    assert!(!r.get("conflict.show").unwrap().writes);
}

#[test]
fn show_both_modified() {
    let (r, mut s) = merging("other");
    let out = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    assert_eq!(out["path"], "a.txt");
    assert_eq!(out["kind"], "BothModified");
    let text = |v: &Value| v["text"].as_str().map(str::to_owned);
    assert_eq!(text(&out["base"]).as_deref(), Some("base\n"));
    assert_eq!(text(&out["ours"]).as_deref(), Some("ours\n"));
    assert_eq!(text(&out["theirs"]).as_deref(), Some("theirs\n"));
    let worktree = text(&out["worktree"]).unwrap();
    assert!(worktree.starts_with("<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> "));
    assert_eq!(out["worktree"]["binary"], false);
    assert_eq!(out["worktree"]["size"], worktree.len());
    assert_eq!(out["marker_size"], 7);
    assert_eq!(out["digest"], worktree_digest(Some(worktree.as_bytes())));
}

#[test]
fn resolve_then_continue() {
    let (r, mut s) = merging("other");
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    let out = run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "unita\n" }),
    );
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert_eq!(backup, "refs/project/backup/conflict-1");
    assert_eq!(out["index_backup"], Value::Null);
    let entries = out["status"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["path"], "a.txt");
    assert_eq!(entries[0]["index"], "Modified");
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);

    // The backup has the file with the markers, at its path.
    let diff = run(
        &r,
        &mut s,
        "diff.commit",
        json!({ "id": backup, "path": "a.txt" }),
    );
    let lines: Vec<&str> = diff["files"][0]["hunks"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l["content"].as_str())
        .collect();
    assert!(lines.iter().any(|l| l.contains("<<<<<<< HEAD")), "{diff}");

    // The resolved file is no longer in conflict.
    let err = r
        .run("conflict.show", &mut s, json!({ "path": "a.txt" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let out = run(&r, &mut s, "merge.continue", json!({}));
    assert_eq!(out["in_progress"], Value::Null);
    assert_eq!(status(&r, &mut s)["entries"], json!([]));
}

#[test]
fn stale_digest_changes_nothing() {
    let (r, mut s) = merging("other");
    let before = status(&r, &mut s);
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "a.txt", "digest": worktree_digest(Some(b"other")), "content": "x\n" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(status(&r, &mut s), before);
    // No backup created: the next one is still the first.
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    let out = run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "x\n" }),
    );
    assert_eq!(out["backup"], "refs/project/backup/conflict-1");
}

#[test]
fn not_a_conflict_is_refused() {
    let (r, mut s) = merging("other");
    let before = status(&r, &mut s);
    for (id, params) in [
        ("conflict.show", json!({ "path": "b.txt" })),
        ("conflict.show", json!({ "path": "manca.txt" })),
        (
            "conflict.resolve",
            json!({ "path": "b.txt", "digest": worktree_digest(None), "delete": true }),
        ),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }
    assert_eq!(status(&r, &mut s), before);
}

#[test]
fn invalid_params() {
    let (r, mut s) = merging("other");
    let d = worktree_digest(None);
    for (id, params) in [
        ("conflict.show", json!({})),
        ("conflict.show", json!({ "path": "../a.txt" })),
        ("conflict.show", json!({ "path": "a.txt", "x": 1 })),
        ("conflict.resolve", json!({ "path": "a.txt", "digest": d })),
        (
            "conflict.resolve",
            json!({ "path": "a.txt", "digest": d, "content": "x", "delete": true }),
        ),
        (
            "conflict.resolve",
            json!({ "path": "a.txt", "digest": d, "delete": false }),
        ),
        (
            "conflict.resolve",
            json!({ "path": "a.txt", "content": "x" }),
        ),
        (
            "conflict.resolve",
            json!({ "path": "/a.txt", "digest": d, "content": "x" }),
        ),
    ] {
        let err = r.run(id, &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{id} {params}: {err}"
        );
    }
}

#[test]
fn delete_for_deleted_by_them() {
    let (r, mut s) = merging("gone");
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    assert_eq!(shown["kind"], "DeletedByThem");
    assert_eq!(shown["theirs"], Value::Null);
    assert_eq!(shown["ours"]["text"], "ours\n");
    assert_eq!(shown["worktree"]["text"], "ours\n");
    let out = run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "delete": true }),
    );
    assert!(out["backup"].is_string());
    let entries = out["status"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["index"], "Deleted");
    assert_eq!(run(&r, &mut s, "repo.operation", json!({}))["ready"], true);
}

/// A hand-written conflict: stages with non-UTF-8 content and absent worktree.
#[test]
fn binary_versions_and_missing_worktree() {
    let mut repo = repo();
    let base = blob(&mut repo, b"base\n");
    let ours = blob(&mut repo, &[0xff, 0xfe, 0x00]);
    repo.set_entries(vec![StatusEntry {
        path: RepoPath::from("a.txt"),
        index: Change::Deleted,
        worktree: Change::Unmerged,
        kind: EntryKind::Unmerged(UnmergedInfo {
            conflict: Conflict::DeletedByUs,
            submodule: None,
            mode_base: 0o100644,
            mode_ours: 0,
            mode_theirs: 0o100644,
            mode_worktree: 0,
            oid_base: base,
            oid_ours: ZERO,
            oid_theirs: ours,
        }),
    }]);
    let (r, mut s) = opened_with(repo);
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    assert_eq!(shown["ours"], Value::Null);
    assert_eq!(shown["worktree"], Value::Null);
    assert_eq!(
        shown["theirs"],
        json!({ "text": null, "binary": true, "size": 3 })
    );
    assert_eq!(shown["digest"], worktree_digest(None));
    // Without a file in the worktree no backup; the new content creates it.
    let out = run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "nuovo\n" }),
    );
    assert_eq!(out["backup"], Value::Null);
    let entries = out["status"]["entries"].as_array().unwrap();
    assert_eq!(entries[0]["index"], "Added");
}

#[test]
fn submodule_and_link_are_refused() {
    for (mode_ours, mode_worktree) in [(0o160000, 0o160000), (0o100644, 0o120000)] {
        let mut repo = repo();
        repo.set_entries(vec![StatusEntry {
            path: RepoPath::from("a.txt"),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(UnmergedInfo {
                conflict: Conflict::BothModified,
                submodule: None,
                mode_base: 0o100644,
                mode_ours,
                mode_theirs: 0o100644,
                mode_worktree,
                oid_base: oid(1),
                oid_ours: oid(2),
                oid_theirs: oid(3),
            }),
        }]);
        let (r, mut s) = opened_with(repo);
        let err = r
            .run("conflict.show", &mut s, json!({ "path": "a.txt" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = r
            .run(
                "conflict.resolve",
                &mut s,
                json!({ "path": "a.txt", "digest": worktree_digest(None), "content": "x" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }
}
