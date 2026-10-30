//! Tests of `file.blame` and `file.log` on the `MemoryBackend` (with real git:
//! `crates/git/tests/file.rs`).
//!
//! The repo: `c1` (root) adds `a.txt` = one/two/three; `c2` changes `two` → `TWO`; `c3`
//! renames `a.txt` → `b.txt`; `c4` adds `four` to `b.txt` and the binary `bin.dat`; `side` at
//! `c5` from `c4` changes `b.txt` (`five`), `main` at `c6`, merge of `c4` and `c5`
//! with the content of `c5`, then `c7` on `main` changes `TWO` → `2`. HEAD on `main`.

use crate::{CancelToken, EngineError, JobContext, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::{
    Change, ChangedFile, Commit, EntryKind, Oid, RepoPath, Signature, StatusEntry, TrackedInfo,
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

fn blob(repo: &mut MemoryBackend, content: &str) -> Oid {
    let hash = content.bytes().fold(1_469_598_103_934_665_603_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(1_099_511_628_211)
    });
    let id = oid(hash);
    repo.set_blob(id, content);
    id
}

/// A change: (path, original for a rename, content before, content after).
type Edit<'a> = (&'a str, Option<&'a str>, Option<&'a str>, &'a str);

fn commit(repo: &mut MemoryBackend, n: u64, parents: &[u64], summary: &str, files: &[Edit]) {
    let changes = files
        .iter()
        .map(|&(path, original, old, new)| {
            let old_oid = old.map_or(ZERO, |c| blob(repo, c));
            let new_oid = blob(repo, new);
            let change = match (original, old) {
                (Some(_), _) => Change::Renamed,
                (None, Some(_)) => Change::Modified,
                (None, None) => Change::Added,
            };
            ChangedFile {
                path: RepoPath::from(path),
                change,
                original: original.map(RepoPath::from),
                score: original.map(|_| 100),
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid,
                new_oid,
            }
        })
        .collect();
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parents.iter().map(|p| oid(*p)).collect(),
        author: sig(n as i64),
        committer: sig(n as i64),
        summary: summary.into(),
        body: String::new(),
    });
    repo.set_changes(oid(n), changes);
}

const V1: &str = "one\ntwo\nthree\n";
const V2: &str = "one\nTWO\nthree\n";
const V4: &str = "one\nTWO\nthree\nfour\n";
const V5: &str = "one\nTWO\nthree\nfour\nfive\n";
const V7: &str = "one\n2\nthree\nfour\nfive\n";

fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    commit(&mut repo, 1, &[], "add a", &[("a.txt", None, None, V1)]);
    commit(&mut repo, 2, &[1], "TWO", &[("a.txt", None, Some(V1), V2)]);
    commit(
        &mut repo,
        3,
        &[2],
        "rename",
        &[("b.txt", Some("a.txt"), Some(V2), V2)],
    );
    let four = [
        ("b.txt", None, Some(V2), V4),
        ("bin.dat", None, None, "x\0y\n"),
    ];
    commit(&mut repo, 4, &[3], "four", &four);
    commit(&mut repo, 5, &[4], "five", &[("b.txt", None, Some(V4), V5)]);
    // The merge takes the content of `side`: the file differs from the first parent only.
    commit(
        &mut repo,
        6,
        &[4, 5],
        "Merge side",
        &[("b.txt", None, Some(V4), V5)],
    );
    commit(
        &mut repo,
        7,
        &[6],
        "two is 2",
        &[("b.txt", None, Some(V5), V7)],
    );
    repo.set_branch("side", oid(5));
    repo.set_branch("main", oid(7));
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

/// For each line: (text, summary of its commit or `None`, orig_line).
fn lines(out: &Value) -> Vec<(String, Option<String>, u64)> {
    let commits = out["commits"].as_array().unwrap();
    out["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let commit = l["commit"]
                .as_u64()
                .map(|i| commits[i as usize]["summary"].as_str().unwrap().to_owned());
            (
                l["text"].as_str().unwrap().to_owned(),
                commit,
                l["orig_line"].as_u64().unwrap(),
            )
        })
        .collect()
}

fn line(text: &str, commit: Option<&str>, orig: u64) -> (String, Option<String>, u64) {
    (text.to_owned(), commit.map(str::to_owned), orig)
}

#[test]
fn file_commands_are_reads() {
    let r = Registry::builtin();
    for id in ["file.blame", "file.log"] {
        assert!(!r.get(id).unwrap().writes, "{id}");
    }
}

#[test]
fn blame_at_head_follows_renames_and_merges() {
    let (r, mut s) = opened_with(repo());
    let out = run(
        &r,
        &mut s,
        "file.blame",
        json!({ "path": "b.txt", "rev": "HEAD" }),
    );
    assert_eq!(out["path"], "b.txt");
    assert_eq!(out["rev"], json!(oid(7).to_string()));
    assert_eq!(
        lines(&out),
        [
            line("one", Some("add a"), 1),
            line("2", Some("two is 2"), 2),
            line("three", Some("add a"), 3),
            line("four", Some("four"), 4),
            // First parents only (git blames `five` on `c5`).
            line("five", Some("Merge side"), 5),
        ]
    );
    // Commits in order of first appearance, with the name the file had.
    let commits = out["commits"].as_array().unwrap();
    assert_eq!(commits[0]["path"], "a.txt");
    assert_eq!(commits[0]["boundary"], true);
    assert_eq!(commits[0]["previous"], Value::Null);
    assert_eq!(commits[1]["id"], json!(oid(7).to_string()));
    assert_eq!(commits[1]["previous"], json!([oid(6).to_string(), "b.txt"]));
    assert_eq!(commits[1]["boundary"], false);
    assert_eq!(commits.len(), 4);
}

#[test]
fn blame_at_an_old_rev() {
    let (r, mut s) = opened_with(repo());
    let c2 = oid(2).to_string();
    let out = run(
        &r,
        &mut s,
        "file.blame",
        json!({ "path": "a.txt", "rev": &c2[..8] }),
    );
    assert_eq!(out["rev"], json!(c2));
    assert_eq!(
        lines(&out),
        [
            line("one", Some("add a"), 1),
            line("TWO", Some("TWO"), 2),
            line("three", Some("add a"), 3),
        ]
    );
}

#[test]
fn blame_of_the_worktree_has_uncommitted_lines() {
    let mut repo = repo();
    let b7 = blob(&mut repo, V7);
    repo.set_entries(vec![StatusEntry {
        path: RepoPath::from("b.txt"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: b7,
            oid_index: b7,
        }),
    }]);
    repo.set_worktree_file("b.txt", "zero\none\n2\nthree\nfour\nfive\n");
    let (r, mut s) = opened_with(repo);
    let out = run(&r, &mut s, "file.blame", json!({ "path": "b.txt" }));
    assert_eq!(out["rev"], Value::Null);
    let got = lines(&out);
    assert_eq!(got[0], line("zero", None, 1));
    assert_eq!(got[1], line("one", Some("add a"), 1));
    assert_eq!(got[2], line("2", Some("two is 2"), 2));
    assert_eq!(got.len(), 6);
}

#[test]
fn blame_errors() {
    let (r, mut s) = opened_with(repo());
    let err = |s: &mut Session, p: Value| r.run("file.blame", s, p).unwrap_err();
    assert!(matches!(
        err(&mut s, json!({ "path": "a.txt" })),
        EngineError::InvalidParams { .. }
    ));
    assert!(matches!(
        err(&mut s, json!({ "path": "a.txt", "rev": "HEAD" })),
        EngineError::InvalidParams { .. }
    ));
    assert!(matches!(
        err(&mut s, json!({ "path": "b.txt", "rev": "nope" })),
        EngineError::UnknownRev(_)
    ));
    assert!(matches!(
        err(&mut s, json!({ "path": "bin.dat" })),
        EngineError::NotAllowed(_)
    ));
    assert!(matches!(
        err(&mut s, json!({ "path": "../x" })),
        EngineError::InvalidParams { .. }
    ));
    assert!(matches!(
        err(&mut s, json!({ "path": "b.txt", "extra": 1 })),
        EngineError::InvalidParams { .. }
    ));
    // On a branch without commits.
    let (r, mut s) = opened_with(MemoryBackend::new());
    let e = r
        .run("file.blame", &mut s, json!({ "path": "x" }))
        .unwrap_err();
    assert!(matches!(e, EngineError::InvalidParams { .. }), "{e}");
}

#[test]
fn blame_and_log_can_be_cancelled() {
    let (r, mut s) = opened_with(repo());
    for (id, params) in [
        ("file.blame", json!({ "path": "b.txt" })),
        ("file.log", json!({ "path": "b.txt" })),
    ] {
        let token = CancelToken::new();
        token.cancel();
        let job = JobContext::new().with_cancel(token);
        let e = r.run_with(id, &mut s, params, job).unwrap_err();
        assert!(matches!(e, EngineError::Cancelled { .. }), "{id}: {e}");
    }
}

/// For each commit of the history: (summary, path, old_path, status).
fn history(out: &Value) -> Vec<(String, String, Option<String>, String)> {
    out["commits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["commit"]["summary"].as_str().unwrap().to_owned(),
                e["path"].as_str().unwrap().to_owned(),
                e["old_path"].as_str().map(str::to_owned),
                e["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn entry(
    summary: &str,
    path: &str,
    old: Option<&str>,
    status: &str,
) -> (String, String, Option<String>, String) {
    (
        summary.into(),
        path.into(),
        old.map(str::to_owned),
        status.into(),
    )
}

#[test]
fn log_follows_renames_by_default() {
    let (r, mut s) = opened_with(repo());
    let out = run(&r, &mut s, "file.log", json!({ "path": "b.txt" }));
    assert_eq!(out["has_more"], false);
    assert_eq!(
        history(&out),
        [
            entry("two is 2", "b.txt", None, "Modified"),
            entry("five", "b.txt", None, "Modified"),
            entry("four", "b.txt", None, "Modified"),
            entry("rename", "b.txt", Some("a.txt"), "Renamed"),
            entry("TWO", "a.txt", None, "Modified"),
            entry("add a", "a.txt", None, "Added"),
        ]
    );
    let out = run(
        &r,
        &mut s,
        "file.log",
        json!({ "path": "b.txt", "follow": false }),
    );
    let got = history(&out);
    assert_eq!(got.len(), 4);
    assert_eq!(got[3], entry("rename", "b.txt", None, "Added"));
    // The old name, without following: up to its disappearance.
    let out = run(
        &r,
        &mut s,
        "file.log",
        json!({ "path": "a.txt", "follow": false }),
    );
    assert_eq!(
        history(&out),
        [
            entry("rename", "a.txt", None, "Deleted"),
            entry("TWO", "a.txt", None, "Modified"),
            entry("add a", "a.txt", None, "Added"),
        ]
    );
}

#[test]
fn log_pages_and_rev() {
    let (r, mut s) = opened_with(repo());
    let all = history(&run(&r, &mut s, "file.log", json!({ "path": "b.txt" })));
    let mut paged = Vec::new();
    for skip in (0..all.len()).step_by(2) {
        let out = run(
            &r,
            &mut s,
            "file.log",
            json!({ "path": "b.txt", "skip": skip, "limit": 2 }),
        );
        assert_eq!(out["has_more"], skip + 2 < all.len(), "skip {skip}");
        // The typed result reads the page back and gives the same JSON again.
        let typed: crate::FileLogResult = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        assert_eq!(
            typed.commits.len(),
            out["commits"].as_array().unwrap().len()
        );
        paged.extend(history(&out));
    }
    assert_eq!(paged, all);
    let out = run(
        &r,
        &mut s,
        "file.log",
        json!({ "path": "b.txt", "rev": "side" }),
    );
    assert_eq!(history(&out)[0], entry("five", "b.txt", None, "Modified"));
    assert_eq!(history(&out).len(), 5);
    let e = r
        .run(
            "file.log",
            &mut s,
            json!({ "path": "b.txt", "rev": "nope" }),
        )
        .unwrap_err();
    assert!(matches!(e, EngineError::UnknownRev(_)), "{e}");
    let e = r
        .run("file.log", &mut s, json!({ "path": "b.txt", "max": 1 }))
        .unwrap_err();
    assert!(matches!(e, EngineError::InvalidParams { .. }), "{e}");
    // Missing file and empty repo: empty history.
    let out = run(&r, &mut s, "file.log", json!({ "path": "nope" }));
    assert_eq!(out["commits"], json!([]));
    let (r, mut s) = opened_with(MemoryBackend::new());
    let out = run(&r, &mut s, "file.log", json!({ "path": "x" }));
    assert_eq!(out["commits"], json!([]));
}
