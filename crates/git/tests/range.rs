//! Integration tests of `diff.range` and of `CliBackend::commit_id`/`common_base` on temporary
//! repos (on the `MemoryBackend`: `crates/engine/src/builtin/range_tests.rs`).

mod common;

use common::Repo;
use project_domain::{Diff, LineKind, RepoPath};
use project_engine::{DiffSpec, EngineError, GitRead, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    registry
        .run(
            "repo.open",
            &mut session,
            json!({ "path": repo.path_str() }),
        )
        .unwrap();
    (registry, session)
}

fn range(repo: &Repo, params: Value) -> Value {
    let (registry, mut session) = open(repo);
    registry
        .run("diff.range", &mut session, params.clone())
        .unwrap_or_else(|e| panic!("diff.range {params}: {e}"))
}

fn invalid(repo: &Repo, params: Value) -> String {
    let (registry, mut session) = open(repo);
    match registry.run("diff.range", &mut session, params.clone()) {
        Err(EngineError::InvalidParams { reason, .. }) => reason.to_string(),
        other => panic!("diff.range {params}: {other:?}"),
    }
}

/// `(change, old path, new path)` of each file.
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

fn lines_added(diff: &Diff, path: &str) -> Vec<String> {
    let file = diff
        .files
        .iter()
        .find(|f| f.new_path.to_str() == Some(path))
        .unwrap();
    file.hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.to_string())
        .collect()
}

fn same(change: &str, path: &str) -> (String, String, String) {
    (change.to_owned(), path.to_owned(), path.to_owned())
}

/// `main`: base (`a.txt`) → `m.txt`; `feat` from base: `f.txt`, then `a.txt` changed.
fn diverged() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\ntwo\n", "base");
    repo.branch("feat");
    repo.commit_file("m.txt", "main\n", "main side");
    repo.checkout("feat");
    repo.commit_file("f.txt", "feat\n", "feat 1");
    repo.commit_file("a.txt", "one\ntwo\nthree\n", "feat 2");
    repo.checkout("main");
    repo
}

#[test]
fn two_commits_compared_directly_and_against_the_merge_base() {
    let repo = diverged();
    let (main, feat) = (repo.rev("main"), repo.rev("feat"));
    let base = repo.rev("main~1");

    let out = range(&repo, json!({ "from": "main", "to": "feat" }));
    assert_eq!(out["from"], main.to_string());
    assert_eq!(out["to"], feat.to_string());
    assert!(out.get("base").is_none(), "{out}");
    assert_eq!(
        summary(&out),
        [
            same("Modified", "a.txt"),
            same("Added", "f.txt"),
            same("Deleted", "m.txt")
        ]
    );
    // Same files and hunks as the backend gives for the two ids (same parser as `diff.commit`).
    let diff: Diff = serde_json::from_value(out).unwrap();
    let direct = repo
        .open()
        .diff(&DiffSpec::Range {
            from: main,
            to: feat,
            path: None,
        })
        .unwrap();
    assert_eq!(diff, direct);
    assert_eq!(lines_added(&diff, "a.txt"), ["three"]);

    let since = range(
        &repo,
        json!({ "from": "main", "to": "feat", "merge_base": true }),
    );
    assert_eq!(since["base"], base.to_string());
    assert_eq!(since["from"], main.to_string());
    assert_eq!(
        summary(&since),
        [same("Modified", "a.txt"), same("Added", "f.txt")]
    );
    // The other way round: what `main` brings.
    let back = range(
        &repo,
        json!({ "from": "feat", "to": "main", "merge_base": true }),
    );
    assert_eq!(summary(&back), [same("Added", "m.txt")]);
    assert_eq!(repo.open().common_base(&main, &feat).unwrap(), Some(base));
}

#[test]
fn unrelated_histories_have_no_merge_base() {
    let repo = diverged();
    repo.git(&["checkout", "--quiet", "--orphan", "lonely"]);
    repo.git(&["rm", "--quiet", "-r", "--cached", "."]);
    std::fs::remove_file(repo.path().join("a.txt")).unwrap();
    std::fs::remove_file(repo.path().join("m.txt")).unwrap();
    repo.commit_file("o.txt", "other\n", "root");

    let out = range(&repo, json!({ "from": "main", "to": "lonely" }));
    assert_eq!(
        summary(&out),
        [
            same("Deleted", "a.txt"),
            same("Deleted", "m.txt"),
            same("Added", "o.txt")
        ]
    );
    let reason = invalid(
        &repo,
        json!({ "from": "main", "to": "lonely", "merge_base": true }),
    );
    assert!(
        reason.contains("main") && reason.contains("lonely"),
        "{reason}"
    );
    let backend = repo.open();
    assert_eq!(
        backend
            .common_base(&repo.rev("main"), &repo.rev("lonely"))
            .unwrap(),
        None
    );
}

#[test]
fn renames_across_several_commits_and_the_path_filter() {
    let repo = Repo::new();
    let text: String = (1..=20).map(|i| format!("line {i}\n")).collect();
    let first = repo.commit_file("old.txt", &text, "add");
    repo.commit_file("other.txt", "x\n", "other");
    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write("new.txt", &format!("{text}line 21\n"));
    repo.stage(&["new.txt"]);
    repo.commit("move");

    let out = range(&repo, json!({ "from": first.to_string(), "to": "HEAD" }));
    assert_eq!(
        summary(&out),
        [
            ("Renamed".into(), "old.txt".into(), "new.txt".into()),
            same("Added", "other.txt")
        ]
    );
    for path in ["old.txt", "new.txt"] {
        let only = range(
            &repo,
            json!({ "from": first.to_string(), "to": "HEAD", "path": path }),
        );
        assert_eq!(
            summary(&only),
            [("Renamed".into(), "old.txt".into(), "new.txt".into())],
            "{path}"
        );
    }
    // Literal pathspec: `*` is a name, not a glob.
    let none = range(
        &repo,
        json!({ "from": first.to_string(), "to": "HEAD", "path": "*" }),
    );
    assert_eq!(none["files"], json!([]));
    let err = Registry::builtin()
        .run(
            "diff.range",
            &mut open(&repo).1,
            json!({ "from": "HEAD~1", "to": "HEAD", "path": "../x" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
}

#[test]
fn binary_files_have_no_hunks() {
    let repo = Repo::new();
    let first = repo.commit_file("data.bin", "a\0b", "one");
    repo.commit_file("data.bin", "a\0c", "two");
    let out = range(&repo, json!({ "from": first.to_string(), "to": "HEAD" }));
    assert_eq!(out["files"][0]["binary"], true);
    assert_eq!(out["files"][0]["hunks"], json!([]));
}

#[test]
fn tags_branches_and_revs_that_start_with_a_dash() {
    let repo = diverged();
    let main = repo.rev("main");
    repo.annotated_tag("v1", "main~1", "release");
    repo.git(&["update-ref", "refs/heads/-dash", "feat"]);

    let out = range(&repo, json!({ "from": "v1", "to": "-dash" }));
    // The annotated tag is peeled to its commit.
    assert_eq!(out["from"], repo.rev("main~1").to_string());
    assert_eq!(out["to"], repo.rev("feat").to_string());
    assert_eq!(
        repo.open().commit_id("v1").unwrap(),
        Some(repo.rev("main~1"))
    );

    // Options and things that are not commits are refused, never passed as options.
    let tree = repo.git(&["rev-parse", "main^{tree}"]);
    let blob = repo.git(&["rev-parse", "main:a.txt"]);
    for bad in [
        "--all",
        "-x",
        "--output=/tmp/x",
        "missing",
        "main..feat",
        "",
        &tree,
        &blob,
    ] {
        let reason = invalid(&repo, json!({ "from": bad, "to": "main" }));
        assert!(reason.contains(bad), "{bad}: {reason}");
        invalid(&repo, json!({ "from": "main", "to": bad }));
    }
    assert_eq!(repo.open().commit_id("--all").unwrap(), None);
    // Nothing moved.
    assert_eq!(repo.rev("main"), main);
}

#[test]
fn same_commit_gives_no_files() {
    let repo = diverged();
    let out = range(&repo, json!({ "from": "main", "to": "HEAD" }));
    assert_eq!(out["files"], json!([]));
    let path = RepoPath::from("a.txt");
    let diff = repo
        .open()
        .diff(&DiffSpec::Range {
            from: repo.rev("main"),
            to: repo.rev("main"),
            path: Some(path),
        })
        .unwrap();
    assert!(diff.files.is_empty());
}
