//! Tests of `diff.range` on the `MemoryBackend` (with real git: `crates/git/tests/range.rs`).
//!
//! The sample repo (see `MemoryBackend::sample`): `main` at 12, `feature/graph` at 9, merge base
//! of 8 and 9 is 4, tag `v0.1.0` on 10, rename `src/main.rs` → `src/bin/app.rs` in 12. Added
//! on top: 100 (child of 12, adds `x.txt`), 101 (renames it to `y.txt` unchanged), 102 (adds the
//! binary `z.bin`), and 99, a root commit unrelated to the rest.

use super::snapshot_tests::{id, opened_with, run};
use crate::{EngineError, MemoryBackend, Registry, Session};
use project_domain::{Change, ChangedFile, Commit, Oid, RepoPath, Signature};
use serde_json::{Value, json};

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn sig() -> Signature {
    Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_800_000_000,
        offset_min: 0,
    }
}

fn add(repo: &mut MemoryBackend, n: u64, parents: &[u64], files: Vec<ChangedFile>) {
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parents.iter().map(|p| oid(*p)).collect(),
        author: sig(),
        committer: sig(),
        summary: format!("commit {n}"),
        body: String::new(),
    });
    repo.set_changes(oid(n), files);
}

fn file(path: &str, change: Change, original: Option<&str>, old: Oid, new: Oid) -> ChangedFile {
    let zero = Oid::from_bytes([0; Oid::LEN]);
    ChangedFile {
        path: RepoPath::from(path),
        change,
        original: original.map(RepoPath::from),
        score: original.map(|_| 100),
        old_mode: if old == zero { 0 } else { 0o100644 },
        new_mode: if new == zero { 0 } else { 0o100644 },
        old_oid: old,
        new_oid: new,
    }
}

fn repo() -> (Registry, Session) {
    let zero = Oid::from_bytes([0; Oid::LEN]);
    let mut repo = MemoryBackend::sample();
    let (x, z, u) = (oid(1000), oid(1001), oid(1002));
    repo.set_blob(x, "hello\n");
    repo.set_blob(z, &b"a\0b"[..]);
    repo.set_blob(u, "unrelated\n");
    add(
        &mut repo,
        100,
        &[12],
        vec![file("x.txt", Change::Added, None, zero, x)],
    );
    add(
        &mut repo,
        101,
        &[100],
        vec![file("y.txt", Change::Renamed, Some("x.txt"), x, x)],
    );
    add(
        &mut repo,
        102,
        &[101],
        vec![file("z.bin", Change::Added, None, zero, z)],
    );
    add(
        &mut repo,
        99,
        &[],
        vec![file("other.txt", Change::Added, None, zero, u)],
    );
    repo.set_branch("lonely", oid(99));
    opened_with(repo)
}

/// `(change, old path, new path)` of each file of a `diff.range` or `diff.commit` result.
fn summary(out: &Value) -> Vec<(String, String, String)> {
    out["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["change"].as_str().unwrap().to_owned(),
                f["old_path"].as_str().unwrap().to_owned(),
                f["new_path"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn invalid(r: &Registry, s: &mut Session, params: Value) -> String {
    match r.run("diff.range", s, params.clone()).unwrap_err() {
        EngineError::InvalidParams { reason, .. } => reason.to_string(),
        other => panic!("{params}: {other}"),
    }
}

#[test]
fn parent_and_child_give_the_commit_diff_with_its_rename() {
    let (r, mut s) = repo();
    let out = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": id(11), "to": id(12) }),
    );
    let commit = run(&r, &mut s, "diff.commit", json!({ "id": id(12) }));
    assert_eq!(out["files"], commit["files"]);
    assert_eq!(out["from"], id(11));
    assert_eq!(out["to"], id(12));
    assert!(out.get("base").is_none(), "{out}");
    assert!(
        summary(&out).contains(&(
            "Renamed".into(),
            "src/main.rs".into(),
            "src/bin/app.rs".into()
        )),
        "{out}"
    );
}

#[test]
fn merge_base_compares_with_the_common_ancestor() {
    let (r, mut s) = repo();
    let direct = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": id(8), "to": id(9) }),
    );
    let since = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": id(8), "to": id(9), "merge_base": true }),
    );
    let from_base = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": id(4), "to": id(9) }),
    );
    assert_eq!(since["base"], id(4));
    assert_eq!(since["from"], id(8));
    assert_eq!(since["to"], id(9));
    assert_eq!(since["files"], from_base["files"]);
    // The direct diff also undoes what 6 and 8 did on the other side.
    assert_ne!(direct["files"], since["files"]);
}

#[test]
fn unrelated_commits_compare_but_have_no_merge_base() {
    let (r, mut s) = repo();
    let out = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": "lonely", "to": "main" }),
    );
    let files = summary(&out);
    assert!(files.contains(&("Deleted".into(), "other.txt".into(), "other.txt".into())));
    assert!(files.contains(&("Added".into(), "README.md".into(), "README.md".into())));
    let reason = invalid(
        &r,
        &mut s,
        json!({ "from": "lonely", "to": "main", "merge_base": true }),
    );
    assert!(
        reason.contains("lonely") && reason.contains("main"),
        "{reason}"
    );
}

#[test]
fn exact_renames_and_binary_files_across_several_commits() {
    let (r, mut s) = repo();
    let out = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": id(100), "to": id(102) }),
    );
    assert_eq!(
        summary(&out),
        [
            ("Renamed".into(), "x.txt".into(), "y.txt".into()),
            ("Added".into(), "z.bin".into(), "z.bin".into()),
        ]
    );
    assert_eq!(out["files"][1]["binary"], true);
    // Path filter: either side of the rename keeps it.
    for path in ["x.txt", "y.txt"] {
        let only = run(
            &r,
            &mut s,
            "diff.range",
            json!({ "from": id(100), "to": id(102), "path": path }),
        );
        assert_eq!(
            summary(&only),
            [("Renamed".into(), "x.txt".into(), "y.txt".into())]
        );
    }
    let none = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": id(100), "to": id(102), "path": "README.md" }),
    );
    assert_eq!(none["files"], json!([]));
}

#[test]
fn tag_and_branch_names_resolve_to_commits() {
    let (r, mut s) = repo();
    let out = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": "v0.1.0", "to": "main" }),
    );
    assert_eq!(out["from"], id(10));
    assert_eq!(out["to"], id(12));
    let same = run(
        &r,
        &mut s,
        "diff.range",
        json!({ "from": "HEAD", "to": "main" }),
    );
    assert_eq!(same["files"], json!([]));
}

#[test]
fn a_rev_that_is_not_a_commit_is_invalid() {
    let (r, mut s) = repo();
    for bad in ["missing", "", "-x", "--all"] {
        let reason = invalid(&r, &mut s, json!({ "from": bad, "to": "main" }));
        assert!(reason.contains(bad), "{reason}");
        invalid(&r, &mut s, json!({ "from": "main", "to": bad }));
    }
}
