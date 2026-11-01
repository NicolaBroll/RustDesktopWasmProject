//! Tests of `commit.checkout` (S55) and of the backup of a detached HEAD left behind by a
//! checkout, on the `MemoryBackend`. Sample repo: `main` at `oid(12)`, tag `v0.1.0` at
//! `oid(10)`, `feature/graph` at `oid(9)`.

use crate::test_util::oid;
use crate::{EngineError, HeadState, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::{Commit, Head, Oid, Operation, Signature};
use serde_json::{Value, json};
use std::sync::Arc;

fn clean_sample() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo
}

/// The clean sample with commit `oid(100)` on top of `main` that no ref reaches.
fn with_loose_commit() -> MemoryBackend {
    let mut repo = clean_sample();
    let sig = Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_800_000_000,
        offset_min: 0,
    };
    repo.insert_commit(Commit {
        id: oid(100),
        parents: vec![oid(12)],
        author: sig.clone(),
        committer: sig,
        summary: "loose".into(),
        body: String::new(),
    });
    repo
}

fn open(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(opener));
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn head(s: &Session) -> Head {
    s.repo().unwrap().head().unwrap()
}

fn detached(o: Oid) -> Value {
    json!({ "detached": o.to_string() })
}

#[test]
fn checks_out_ids_tags_and_branches_detached_then_undo_and_redo() {
    let (r, mut s) = open(clean_sample());
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": "v0.1.0" }));
    assert_eq!(out["head"], detached(oid(10)));
    assert_eq!(out["previous"], json!({ "branch": "refs/heads/main" }));
    assert_eq!(out["changed"], true);
    assert_eq!(out["warnings"], json!([]));
    assert!(out.get("backup").is_none(), "{out}");
    assert_eq!(
        out["status"]["head"]["Detached"],
        json!(oid(10).to_string())
    );
    assert_eq!(head(&s), Head::Detached(oid(10)));
    assert_eq!(
        out["next_undo"]["text"],
        format!(
            "Checkout of {} (detached HEAD) (from `main`)",
            oid(10).short()
        )
    );

    // Already detached there: nothing changes, nothing in the journal.
    let short = oid(10).to_string()[..8].to_owned();
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": short }));
    assert_eq!(out["changed"], false);
    assert_eq!(s.journal().unwrap().done().len(), 1);

    // A branch name: detached at its commit, not on the branch.
    let out = run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": "feature/graph" }),
    );
    assert_eq!(out["head"], detached(oid(9)));
    assert_eq!(out["previous"], detached(oid(10)));
    // `oid(10)` is tagged: no backup.
    assert!(out.get("backup").is_none(), "{out}");

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "commit.checkout");
    assert_eq!(head(&s), Head::Detached(oid(10)));
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(matches!(head(&s), Head::Branch { name, .. } if name == "main"));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(head(&s), Head::Detached(oid(10)));

    // Undo refused if HEAD changed from outside.
    s.repo_mut()
        .unwrap()
        .checkout(&HeadState::Detached(oid(12)))
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
}

#[test]
fn the_same_commit_as_the_branch_still_detaches() {
    let (r, mut s) = open(clean_sample());
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": "main" }));
    assert_eq!(out["changed"], true);
    assert_eq!(head(&s), Head::Detached(oid(12)));
    // "Create a branch here": `branch.create` without `start` from the detached HEAD.
    let out = run(&r, &mut s, "branch.create", json!({ "name": "here" }));
    assert_eq!(out["target"], oid(12).to_string());
}

#[test]
fn refusals_change_nothing() {
    let (r, mut s) = open(clean_sample());
    for (params, invalid) in [
        (json!({}), true),
        (json!({ "rev": "" }), true),
        (json!({ "rev": "main", "x": 1 }), true),
    ] {
        let err = r
            .run("commit.checkout", &mut s, params.clone())
            .unwrap_err();
        assert_eq!(
            matches!(err, EngineError::InvalidParams { .. }),
            invalid,
            "{params}: {err}"
        );
    }
    // A rev that does not exist (the `MemoryBackend` says "not a commit", git "unknown rev").
    for rev in ["missing", "-x", "--force"] {
        let err = r
            .run("commit.checkout", &mut s, json!({ "rev": rev }))
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::InvalidParams { .. } | EngineError::UnknownRev(_)
            ),
            "{rev}: {err}"
        );
    }
    assert!(matches!(head(&s), Head::Branch { .. }));
    assert!(!s.journal().unwrap().can_undo());

    // Local changes that the checkout would overwrite.
    let (r, mut s) = open(MemoryBackend::sample());
    let status = s.repo().unwrap().status().unwrap();
    let err = r
        .run("commit.checkout", &mut s, json!({ "rev": "feature/graph" }))
        .unwrap_err();
    let EngineError::CheckoutConflict { target, files, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(target, &oid(9).to_string());
    assert!(!files.is_empty());
    assert_eq!(s.repo().unwrap().status().unwrap(), status);
    assert!(!s.journal().unwrap().can_undo());

    // Operation in progress.
    let mut repo = clean_sample();
    repo.set_operation(Some(Operation::Rebase));
    let (r, mut s) = open(repo);
    let err = r
        .run("commit.checkout", &mut s, json!({ "rev": "v0.1.0" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(matches!(head(&s), Head::Branch { .. }));
}

#[test]
fn leaving_a_commit_on_no_ref_saves_it() {
    let (r, mut s) = open(with_loose_commit());
    run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": oid(100).to_string() }),
    );
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": "v0.1.0" }));
    assert_eq!(out["backup"], "refs/project/backup/detached-1");
    let warning = out["warnings"][0]["text"].as_str().unwrap();
    assert!(warning.contains("1 commit on no branch"), "{warning}");
    assert!(
        warning.contains("refs/project/backup/detached-1"),
        "{warning}"
    );
    assert_eq!(
        s.journal()
            .unwrap()
            .done()
            .last()
            .unwrap()
            .backup
            .as_deref(),
        Some("refs/project/backup/detached-1")
    );
    assert_eq!(saved(&s, "detached-1"), Some(oid(100)));

    // Undo goes back to the loose commit, redo leaves it again (a second backup).
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(head(&s), Head::Detached(oid(100)));
    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(head(&s), Head::Detached(oid(10)));
    assert_eq!(out["backups"], json!(["refs/project/backup/detached-2"]));

    // `branch.checkout` from the loose commit saves it too.
    run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": oid(100).to_string() }),
    );
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "main" }));
    assert_eq!(out["backup"], "refs/project/backup/detached-3");
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1);
}

#[test]
fn leaving_a_commit_on_a_branch_saves_nothing() {
    let (r, mut s) = open(with_loose_commit());
    run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": "feature/graph" }),
    );
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "main" }));
    assert!(out.get("backup").is_none(), "{out}");
    assert_eq!(out["warnings"], json!([]));
    // Remote branch only (`origin/main` at `oid(11)`, not on a local branch tip but reachable
    // from `main` anyway) and a commit reachable from a branch: no backup either.
    run(
        &r,
        &mut s,
        "commit.checkout",
        json!({ "rev": "origin/main" }),
    );
    let out = run(&r, &mut s, "commit.checkout", json!({ "rev": "v0.1.0" }));
    assert!(out.get("backup").is_none(), "{out}");
    assert_eq!(saved(&s, "detached-1"), None);
}

/// The commit of the backup `refs/project/backup/<name>`, if it exists.
fn saved(s: &Session, name: &str) -> Option<Oid> {
    let repo = s.repo().unwrap();
    repo.commit_id(&format!("refs/project/backup/{name}"))
        .ok()
        .flatten()
}
