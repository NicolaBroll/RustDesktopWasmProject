//! Integration tests of `repo.search` with `"in": ["content"]` (S61) on the `CliBackend`:
//! text added, removed, only moved inside a file (not a match for `-S`, a match for `-G`),
//! regex and invalid regex, `ignore_case`, path filter, `all`, non-UTF-8 file content, a text
//! that looks like an option, pages without a total, cancellation.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{CancelToken, EngineError, JobContext, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn session(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

/// `(ids, has_more)` of a content search with `params`; `total` must be `null`.
fn content(repo: &Repo, mut params: Value) -> (Vec<Oid>, bool) {
    params["in"] = json!(["content"]);
    let (r, mut s) = session(repo);
    let out = r
        .run("repo.search", &mut s, params.clone())
        .unwrap_or_else(|e| panic!("{params}: {e}"));
    assert!(out["total"].is_null(), "{out}");
    let ids = out["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().parse().unwrap())
        .collect();
    (ids, out["has_more"].as_bool().unwrap())
}

fn ids(repo: &Repo, params: Value) -> Vec<Oid> {
    content(repo, params).0
}

fn error(repo: &Repo, params: Value) -> EngineError {
    let (r, mut s) = session(repo);
    r.run("repo.search", &mut s, params).unwrap_err()
}

/// Steps as in the `MemoryBackend` tests: add, unrelated, move inside the file, remove,
/// `FOO()` in another folder, binary file.
struct History {
    repo: Repo,
    added: Oid,
    moved: Oid,
    removed: Oid,
    upper: Oid,
    binary: Oid,
}

fn history() -> History {
    let repo = Repo::new();
    let added = repo.commit_file("f", "foo()\nx\ny\nz\n", "add");
    repo.commit_file("other", "nothing\n", "unrelated");
    let moved = repo.commit_file("f", "x\ny\nz\nfoo()\n", "move");
    let removed = repo.commit_file("f", "x\ny\nz\n", "remove");
    std::fs::create_dir_all(repo.path().join("dir")).unwrap();
    let upper = repo.commit_file("dir/g", "FOO()\n", "upper");
    std::fs::write(repo.path().join("bin"), b"\0foo()\n").unwrap();
    repo.stage(&["bin"]);
    let binary = repo.commit("binary");
    History {
        repo,
        added,
        moved,
        removed,
        upper,
        binary,
    }
}

#[test]
fn literal_text_added_removed_and_moved() {
    let h = history();
    // A user configuration that would change the result is overridden.
    h.repo.git(&["config", "diff.renames", "copies"]);
    h.repo.git(&["config", "log.follow", "true"]);
    let (found, more) = content(&h.repo, json!({ "query": "foo()" }));
    // The move inside `f` keeps the count: not a match.
    assert_eq!(found, [h.binary, h.removed, h.added]);
    assert!(!more);
    assert_eq!(
        ids(&h.repo, json!({ "query": "foo()", "ignore_case": true })),
        [h.binary, h.upper, h.removed, h.added]
    );
}

#[test]
fn regex_matches_added_and_removed_lines() {
    let h = history();
    let re = json!({ "query": "^fo+\\(", "regex": true });
    // The move is a removed and an added line; the binary file is skipped.
    assert_eq!(ids(&h.repo, re), [h.removed, h.moved, h.added]);
    let re = json!({ "query": "^fo+\\(", "regex": true, "ignore_case": true });
    assert_eq!(ids(&h.repo, re), [h.upper, h.removed, h.moved, h.added]);
    let e = error(
        &h.repo,
        json!({ "query": "fo(", "in": ["content"], "regex": true }),
    );
    match e {
        EngineError::InvalidParams { id, reason } => {
            assert_eq!(id, "repo.search");
            let text = reason.to_string();
            assert!(
                text.contains("Unmatched") || text.contains("paren"),
                "{text}"
            );
        }
        other => panic!("{other}"),
    }
}

#[test]
fn path_filter_and_all() {
    let h = history();
    let repo = &h.repo;
    assert_eq!(
        ids(
            repo,
            json!({ "query": "foo", "ignore_case": true, "path": "dir" })
        ),
        [h.upper]
    );
    assert_eq!(
        ids(repo, json!({ "query": "foo()", "path": "f" })),
        [h.removed, h.added]
    );
    repo.branch("side");
    repo.checkout("side");
    let only = repo.commit_file("s", "needle\n", "side only");
    repo.checkout("main");
    assert!(ids(repo, json!({ "query": "needle" })).is_empty());
    assert_eq!(ids(repo, json!({ "query": "needle", "all": true })), [only]);
}

#[test]
fn non_utf8_content_and_option_like_text() {
    let repo = Repo::new();
    std::fs::write(repo.path().join("latin"), b"caf\xe9 -n needle\n").unwrap();
    repo.stage(&["latin"]);
    let latin = repo.commit("latin-1 file");
    repo.commit_file("a", "--all\n", "dash");
    assert_eq!(ids(&repo, json!({ "query": "needle" })), [latin]);
    assert_eq!(ids(&repo, json!({ "query": "-n needle" })), [latin]);
    // Glued to `-S`/`-G`: never an option of its own.
    assert_eq!(ids(&repo, json!({ "query": "--all" })).len(), 1);
    assert_eq!(ids(&repo, json!({ "query": "^-", "regex": true })).len(), 1);
    assert_eq!(
        ids(&repo, json!({ "query": "caf.", "regex": true })),
        [latin]
    );
}

#[test]
fn pages_without_total() {
    let repo = Repo::new();
    let mut all = Vec::new();
    for i in 0..7 {
        all.push(repo.commit_file(&format!("f{i}"), "token\n", &format!("step {i}")));
        repo.commit_file("noise", &format!("{i}\n"), "noise");
    }
    all.reverse();
    let mut joined = Vec::new();
    for skip in (0..7).step_by(3) {
        let (page, more) = content(&repo, json!({ "query": "token", "skip": skip, "limit": 3 }));
        assert_eq!(more, skip + 3 < 7, "skip {skip}");
        joined.extend(page);
    }
    assert_eq!(joined, all);
    assert_eq!(
        content(&repo, json!({ "query": "token", "skip": 7 })),
        (vec![], false)
    );
}

#[test]
fn errors_and_cancel() {
    let h = history();
    for params in [
        json!({ "query": "a", "in": ["content", "id"] }),
        json!({ "query": "a", "regex": true }),
    ] {
        let e = error(&h.repo, params.clone());
        assert!(
            matches!(e, EngineError::InvalidParams { .. }),
            "{params}: {e}"
        );
    }
    let (r, mut s) = session(&h.repo);
    let token = CancelToken::new();
    token.cancel();
    let e = r
        .run_with(
            "repo.search",
            &mut s,
            json!({ "query": "foo", "in": ["content"] }),
            JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(matches!(e, EngineError::Cancelled { .. }), "{e}");
    // Empty repo: nothing to search, no error.
    assert_eq!(
        content(&Repo::new(), json!({ "query": "x" })),
        (vec![], false)
    );
}
