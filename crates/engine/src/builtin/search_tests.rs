//! Tests of `repo.search` on the `MemoryBackend` (with real git: `crates/git/tests/search.rs`).
//!
//! The sample repo (commits 1..=12 on `main`, `feature/graph` merged at 10, `src/main.rs`
//! renamed to `src/bin/app.rs` in 12) plus commit 14 on the unmerged branch `wip`.

use crate::{
    CancelToken, EngineError, GitRead, JobContext, LogQuery, MemoryBackend, MemoryOpener, Registry,
    Session,
};
use project_domain::{Commit, Oid, Signature};
use serde_json::{Value, json};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn repo() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    let carla = Signature {
        name: "Carla Neri".into(),
        email: "carla@example.org".into(),
        time: 1_800_000_000,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id: oid(14),
        parents: vec![oid(11)],
        author: carla.clone(),
        committer: carla,
        summary: "Try a.b*[ literally".into(),
        body: "Second paragraph with NEEDLE".into(),
    });
    repo.set_branch("wip", oid(14));
    repo
}

fn opened() -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo());
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

/// The page of `repo.search`, read through the typed result
/// [`RepoSearchResult`](crate::RepoSearchResult)
/// (which gives the same JSON again: the UI deserializes into it).
fn search(r: &Registry, s: &mut Session, params: Value) -> (Vec<Oid>, u64, bool) {
    let out = r
        .run("repo.search", s, params)
        .unwrap_or_else(|e| panic!("{e}"));
    let typed: crate::RepoSearchResult = serde_json::from_value(out.clone()).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), out);
    let total = u64::try_from(typed.total).unwrap();
    (typed.ids, total, typed.has_more)
}

fn ids(r: &Registry, s: &mut Session, params: Value) -> Vec<Oid> {
    search(r, s, params).0
}

#[test]
fn message_is_literal_and_case_insensitive() {
    let (r, mut s) = opened();
    // Subject and body; `all` to reach `wip`.
    let found = ids(&r, &mut s, json!({ "query": "A.B*[", "all": true }));
    assert_eq!(found, [oid(14)]);
    let found = ids(&r, &mut s, json!({ "query": "needle", "all": true }));
    assert_eq!(found, [oid(14)]);
    // `.` is not "any character".
    assert!(ids(&r, &mut s, json!({ "query": "Lo. parser" })).is_empty());
    // "lanes" in three messages, children before parents as in `repo.log`.
    let found = ids(&r, &mut s, json!({ "query": "LANES", "in": ["message"] }));
    assert_eq!(found, [oid(9), oid(7), oid(5)]);
}

#[test]
fn author_by_name_and_by_email() {
    let (r, mut s) = opened();
    let bruno = [oid(12), oid(9), oid(7), oid(5), oid(3)];
    let by_name = ids(
        &r,
        &mut s,
        json!({ "query": "bruno verdi", "in": ["author"] }),
    );
    assert_eq!(by_name, bruno);
    let by_email = ids(
        &r,
        &mut s,
        json!({ "query": "BRUNO@EXAMPLE", "in": ["author"] }),
    );
    assert_eq!(by_email, bruno);
    // Only in the fields asked for.
    assert!(ids(&r, &mut s, json!({ "query": "bruno", "in": ["message"] })).is_empty());
}

#[test]
fn short_hash_and_or_of_fields() {
    let (r, mut s) = opened();
    let hex = oid(4).to_string();
    let short = hex[..7].to_ascii_uppercase();
    assert_eq!(
        ids(&r, &mut s, json!({ "query": short, "in": ["id"] })),
        [oid(4)]
    );
    // Not hexadecimal: no id match; a non-prefix piece of the id does not match.
    assert!(ids(&r, &mut s, json!({ "query": &hex[1..8], "in": ["id"] })).is_empty());
    // OR of the fields: "Ada" in the author and "Lanes" in a message by Bruno.
    let any = ids(&r, &mut s, json!({ "query": "a" }));
    assert_eq!(any.len(), 12);
}

#[test]
fn path_without_rename_following() {
    let (r, mut s) = opened();
    // The rename commit touches both names; the earlier history only the old name.
    let new = ids(
        &r,
        &mut s,
        json!({ "query": "a", "path": "src/bin/app.rs" }),
    );
    assert_eq!(new, [oid(12)]);
    let old = ids(&r, &mut s, json!({ "query": "a", "path": "src/main.rs" }));
    assert_eq!(old, [oid(12), oid(3)]);
    // A folder: everything under it; the merge that brings it into `main` counts.
    let tests = ids(&r, &mut s, json!({ "query": "a", "path": "tests" }));
    assert_eq!(tests, [oid(10), oid(9)]);
}

#[test]
fn all_reaches_unmerged_branches_in_log_order() {
    let (r, mut s) = opened();
    assert!(ids(&r, &mut s, json!({ "query": "carla" })).is_empty());
    let (found, total, _) = search(&r, &mut s, json!({ "query": "e", "all": true }));
    let log: Vec<Oid> = repo()
        .log(&LogQuery::all().from_all_refs())
        .unwrap()
        .iter()
        .map(|c| c.id)
        .filter(|id| found.contains(id))
        .collect();
    assert_eq!(found, log);
    assert!(found.contains(&oid(14)));
    assert_eq!(total as usize, found.len());
}

#[test]
fn pages_are_contiguous() {
    let (r, mut s) = opened();
    let (all, total, more) = search(&r, &mut s, json!({ "query": "e", "all": true }));
    assert!(!more);
    assert!(total > 5);
    let mut joined = Vec::new();
    for skip in (0..total).step_by(2) {
        let (page, t, more) = search(
            &r,
            &mut s,
            json!({ "query": "e", "all": true, "skip": skip, "limit": 2 }),
        );
        assert_eq!(t, total);
        assert_eq!(more, skip + 2 < total);
        joined.extend(page);
    }
    assert_eq!(joined, all);
    let (page, _, more) = search(&r, &mut s, json!({ "query": "e", "skip": 100 }));
    assert!(page.is_empty() && !more);
}

#[test]
fn invalid_params() {
    let (r, mut s) = opened();
    for params in [
        json!({ "query": "" }),
        json!({ "query": "two\nlines" }),
        json!({ "query": "a", "in": [] }),
        json!({ "query": "a", "in": ["summary"] }),
        json!({ "query": "a", "stashes": true }),
        json!({ "query": "a", "path": "../x" }),
        json!({ "query": "a", "regex": true }),
        json!({}),
    ] {
        let e = r.run("repo.search", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(
                e,
                EngineError::InvalidParams {
                    id: "repo.search",
                    ..
                }
            ),
            "{params}: {e}"
        );
    }
}

#[test]
fn can_be_cancelled() {
    let (r, mut s) = opened();
    let token = CancelToken::new();
    token.cancel();
    let job = JobContext::new().with_cancel(token);
    let e = r
        .run_with("repo.search", &mut s, json!({ "query": "a" }), job)
        .unwrap_err();
    assert!(matches!(e, EngineError::Cancelled { .. }), "{e}");
}
