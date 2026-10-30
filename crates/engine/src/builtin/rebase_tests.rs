//! Tests of `rebase.plan`, `rebase.start`, `rebase.continue`, `rebase.abort` and `repo.operation`
//! with a rebase on the `MemoryBackend` (with real git: `crates/git/tests/rebase.rs`).
//!
//! The repo: `c1` (root: `a.txt` = "base", `b.txt`), `c2` "one" (`a.txt` = "one"), `c3` "two"
//! (adds `c.txt`), `c4` "three" (`a.txt` = "three"); `main` at `c4`, HEAD on `main`, clean
//! worktree. `side` at `c5` from `c1` (adds `n.txt`); `merged` at `c6`, merge of `c4` and `c5`.

use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::{Change, ChangedFile, Commit, EntryKind, Oid, RepoPath, Signature};
use serde_json::{Value, json};
use std::sync::Arc;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn id(n: u64) -> String {
    oid(n).to_string()
}

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

fn commit(
    repo: &mut MemoryBackend,
    n: u64,
    parents: &[u64],
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
    let author = if n == 3 { "Bea" } else { "Ada" };
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
    commit(
        &mut repo,
        1,
        &[],
        "base",
        &[("a.txt", None, "base\n"), ("b.txt", None, "b\n")],
    );
    commit(
        &mut repo,
        2,
        &[1],
        "one",
        &[("a.txt", Some("base\n"), "one\n")],
    );
    commit(&mut repo, 3, &[2], "two", &[("c.txt", None, "c\n")]);
    commit(
        &mut repo,
        4,
        &[3],
        "three",
        &[("a.txt", Some("one\n"), "three\n")],
    );
    commit(&mut repo, 5, &[1], "side", &[("n.txt", None, "n\n")]);
    commit(
        &mut repo,
        6,
        &[4, 5],
        "Merge branch 'side'",
        &[("n.txt", None, "n\n")],
    );
    repo.set_branch("main", oid(4));
    repo.set_branch("side", oid(5));
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

/// `(summary, body)` of the commits of `main` after `c1`, oldest first.
fn history(r: &Registry, s: &mut Session) -> Vec<(String, String)> {
    let log = run(r, s, "repo.log", json!({ "rev": "main" }));
    let mut out: Vec<(String, String)> = log["commits"]
        .as_array()
        .unwrap()
        .iter()
        .take_while(|c| c["id"] != id(1))
        .map(|c| {
            (
                c["summary"].as_str().unwrap().to_owned(),
                c["body"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    out.reverse();
    out
}

fn step(action: &str, n: u64) -> Value {
    json!({ "action": action, "commit": id(n) })
}

fn start(steps: Vec<Value>) -> Value {
    json!({ "base": id(1), "steps": steps })
}

fn files_at_head(r: &Registry, s: &mut Session, rev: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut next = rev.to_owned();
    // The files changed along the first parents (enough for these small histories).
    loop {
        let c = run(r, s, "commit.show", json!({ "id": next }));
        for f in c["files"].as_array().unwrap() {
            let path = f["path"].as_str().unwrap().to_owned();
            if !out.contains(&path) {
                out.push(path);
            }
        }
        match c["commit"]["parents"].as_array().unwrap().first() {
            Some(p) => next = p.as_str().unwrap().to_owned(),
            None => break,
        }
    }
    out.sort();
    out
}

#[test]
fn plan_lists_the_range_oldest_first() {
    let mut repo = repo();
    repo.set_remote_branch("origin/main", oid(3));
    let (r, mut s) = opened_with(repo);
    let plan = run(&r, &mut s, "rebase.plan", json!({ "base": id(1) }));
    assert_eq!(plan["branch"], "main");
    assert_eq!(plan["base"], id(1));
    let steps = plan["steps"].as_array().unwrap();
    let ids: Vec<&str> = steps
        .iter()
        .map(|s| s["commit"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [id(2), id(3), id(4)]);
    assert!(steps.iter().all(|s| s["action"] == "pick"));
    assert_eq!(steps[1]["summary"], "two");
    assert_eq!(plan["published"], json!([id(2), id(3)]));
}

#[test]
fn plan_refusals() {
    let (r, mut s) = opened_with(repo());
    for (base, needle) in [(id(4), "is HEAD"), (id(5), "not an ancestor")] {
        let err = r
            .run("rebase.plan", &mut s, json!({ "base": base }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(err.to_string().contains(needle), "{err}");
    }
    let err = r
        .run("rebase.plan", &mut s, json!({ "base": "nope" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    let err = r
        .run("rebase.plan", &mut s, json!({ "base": "" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    let err = r
        .run("rebase.plan", &mut s, json!({ "base": id(1), "x": 1 }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

    let mut merged = repo();
    merged.set_head_branch("merged");
    let (r, mut s) = opened_with(merged);
    let err = r
        .run("rebase.plan", &mut s, json!({ "base": id(1) }))
        .unwrap_err();
    assert!(err.to_string().contains("merge commits"), "{err}");

    let mut detached = repo();
    detached.detach_head(oid(4));
    let (r, mut s) = opened_with(detached);
    let err = r
        .run("rebase.plan", &mut s, json!({ "base": id(1) }))
        .unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");
    let err = r
        .run("rebase.start", &mut s, start(vec![step("drop", 2)]))
        .unwrap_err();
    assert!(err.to_string().contains("detached"), "{err}");
}

#[test]
fn reorder_and_fixup_with_message_then_undo_redo() {
    let (r, mut s) = opened_with(repo());
    let mut fixup = step("fixup", 4);
    fixup["message"] = json!("one and three\n\nwith a body");
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        start(vec![step("pick", 3), step("pick", 2), fixup]),
    );
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["before"], id(4));
    assert_eq!(out["backup"], "refs/project/backup/rebase-1");
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["can_undo"], true);
    let after = main_at(&r, &mut s);
    assert_eq!(out["after"], after);
    assert_eq!(out["status"]["head"]["Branch"]["name"], "main");
    assert_eq!(out["status"]["entries"], json!([]));
    assert_eq!(
        history(&r, &mut s),
        [
            ("two".to_owned(), String::new()),
            ("one and three".to_owned(), "with a body".to_owned())
        ]
    );
    // The author of a moved commit stays.
    let log = run(&r, &mut s, "repo.log", json!({ "rev": "main" }));
    assert_eq!(log["commits"][1]["author"]["name"], "Bea");
    let hist = run(&r, &mut s, "edit.history", json!({}));
    let last = hist["done"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["command"], "rebase.start");
    assert_eq!(
        last["description"]["text"],
        format!(
            "Rebase of `main` (3 commits, from {} to {})",
            &id(4)[..7],
            &after[..7]
        )
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), id(4));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(main_at(&r, &mut s), after);
}

#[test]
fn squash_without_message_joins_the_messages() {
    let (r, mut s) = opened_with(repo());
    run(
        &r,
        &mut s,
        "rebase.start",
        start(vec![step("pick", 2), step("squash", 3), step("pick", 4)]),
    );
    assert_eq!(
        history(&r, &mut s),
        [
            ("one".to_owned(), "two".to_owned()),
            ("three".to_owned(), String::new())
        ]
    );
}

#[test]
fn reword_and_drop() {
    let (r, mut s) = opened_with(repo());
    let mut reword = step("reword", 2);
    reword["message"] = json!("ONE");
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        start(vec![reword, step("drop", 3), step("pick", 4)]),
    );
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(
        history(&r, &mut s),
        [
            ("ONE".to_owned(), String::new()),
            ("three".to_owned(), String::new())
        ]
    );
    assert_eq!(files_at_head(&r, &mut s, "main"), ["a.txt", "b.txt"]);
}

#[test]
fn unchanged_leading_picks_keep_their_commits() {
    let (r, mut s) = opened_with(repo());
    run(
        &r,
        &mut s,
        "rebase.start",
        start(vec![step("pick", 2), step("pick", 3), step("drop", 4)]),
    );
    assert_eq!(main_at(&r, &mut s), id(3));
}

#[test]
fn start_refusals_change_nothing() {
    let (r, mut s) = opened_with(repo());
    // Invalid plan: the step is named.
    let err = r
        .run(
            "rebase.start",
            &mut s,
            start(vec![step("pick", 2), step("reword", 3), step("pick", 4)]),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert!(err.to_string().contains("step 2"), "{err}");
    let err = r
        .run("rebase.start", &mut s, start(vec![step("pick", 2)]))
        .unwrap_err();
    assert!(err.to_string().contains("missing"), "{err}");
    let err = r
        .run(
            "rebase.start",
            &mut s,
            json!({ "base": id(1), "steps": [{ "action": "edit", "commit": id(2) }] }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    // The current history: nothing to do.
    let err = r
        .run(
            "rebase.start",
            &mut s,
            start(vec![step("pick", 2), step("pick", 3), step("pick", 4)]),
        )
        .unwrap_err();
    assert!(err.to_string().contains("change nothing"), "{err}");
    assert_eq!(main_at(&r, &mut s), id(4));

    // Local changes: refused, never autostash.
    let mut dirty = repo();
    dirty.set_worktree_file("a.txt", "dirty\n");
    let a = blob(&mut dirty, "three\n");
    dirty.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("a.txt"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: EntryKind::Ordinary(project_domain::TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: a,
            oid_index: a,
        }),
    }]);
    let (r, mut s) = opened_with(dirty);
    let err = r
        .run(
            "rebase.start",
            &mut s,
            start(vec![step("pick", 3), step("pick", 2), step("pick", 4)]),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("a.txt"), "{err}");
}

#[test]
fn published_commits_need_confirm() {
    let mut published = repo();
    published.set_remote_branch("origin/main", oid(2));
    let (r, mut s) = opened_with(published);
    // Only `c4` is rewritten: `c2` (published) stays as it is.
    let mut reword = step("reword", 4);
    reword["message"] = json!("THREE");
    run(
        &r,
        &mut s,
        "rebase.start",
        start(vec![step("pick", 2), step("pick", 3), reword.clone()]),
    );
    run(&r, &mut s, "edit.undo", json!({}));
    // Rewriting `c2` too needs the confirmation.
    let plan = start(vec![step("pick", 3), step("pick", 2), reword]);
    let err = r.run("rebase.start", &mut s, plan.clone()).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(err.to_string().contains(&id(2)[..7]), "{err}");
    assert_eq!(main_at(&r, &mut s), id(4));
    let mut confirmed = plan;
    confirmed["confirm"] = json!(true);
    let out = run(&r, &mut s, "rebase.start", confirmed);
    assert_eq!(out["in_progress"], false);
}

/// `c4` before `c2`: `c4` changes `a.txt` from "one", which is not there yet.
fn conflicted() -> (Registry, Session, Value) {
    let (r, mut s) = opened_with(repo());
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        start(vec![step("pick", 4), step("pick", 2), step("pick", 3)]),
    );
    (r, s, out)
}

#[test]
fn conflict_stays_in_progress_then_continue() {
    let (r, mut s, out) = conflicted();
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], id(4));
    assert_eq!(out["can_undo"], false);
    assert!(
        out["message"].as_str().unwrap().contains("CONFLICT"),
        "{out}"
    );
    assert_eq!(out["status"]["operation"], "Rebase");
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Rebase");
    assert_eq!(op["continue"], "rebase.continue");
    assert_eq!(op["abort"], "rebase.abort");
    assert_eq!(op["onto"], id(1));
    assert_eq!(op["rebase_branch"], "refs/heads/main");
    // The typed result of `repo.operation` reads it back and gives the same JSON again.
    let typed: crate::RepoOperationResult = serde_json::from_value(op.clone()).unwrap();
    assert_eq!(serde_json::to_value(&typed).unwrap(), op);
    assert_eq!(typed.onto, Some(oid(1)));
    assert_eq!(typed.rebase_branch.as_deref(), Some("refs/heads/main"));
    assert!(r.run("edit.undo", &mut s, json!({})).is_err());
    assert!(r.run("rebase.start", &mut s, start(vec![])).is_err());
    let err = r.run("rebase.continue", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("a.txt"), "{err}");

    // Resolved with the markers' text, staged: `c2` then conflicts too.
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let next = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(next["in_progress"], true);
    assert_eq!(next["conflicts"], json!(["a.txt"]));
    assert_eq!(next["command"], "rebase.start");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let done = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(done["in_progress"], false);
    assert_eq!(done["status"]["operation"], json!(null));
    assert_eq!(done["status"]["head"]["Branch"]["name"], "main");
    assert_eq!(done["backup"], "refs/project/backup/rebase-1");
    assert_eq!(done["warnings"], json!([]));
    assert_eq!(
        history(&r, &mut s)
            .into_iter()
            .map(|(s, _)| s)
            .collect::<Vec<_>>(),
        ["three", "one", "two"]
    );
    let hist = run(&r, &mut s, "edit.history", json!({}));
    let last = hist["done"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(last["command"], "rebase.continue");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(main_at(&r, &mut s), id(4));
}

#[test]
fn conflict_resolved_to_nothing_is_dropped() {
    let (r, mut s, _) = conflicted();
    // `a.txt` as HEAD (`c1`): nothing left of `c4`.
    let resolved = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": resolved["digest"], "content": "base\n" }),
    );
    let done = run(&r, &mut s, "rebase.continue", json!({}));
    assert_eq!(done["in_progress"], false);
    let warnings = done["warnings"].to_string();
    assert!(warnings.contains("dropped"), "{warnings}");
    assert!(warnings.contains("instead of"), "{warnings}");
    assert_eq!(
        history(&r, &mut s)
            .into_iter()
            .map(|(s, _)| s)
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
}

#[test]
fn conflict_then_abort() {
    let (r, mut s, _) = conflicted();
    let err = r.run("rebase.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(err.to_string().contains("a.txt"), "{err}");
    let aborted = run(&r, &mut s, "rebase.abort", json!({ "confirm": true }));
    assert_eq!(aborted["backup"], "refs/project/backup/rebase-abort-1");
    assert_eq!(aborted["files"], json!(["a.txt"]));
    assert_eq!(aborted["status"]["operation"], json!(null));
    assert_eq!(aborted["status"]["head"]["Branch"]["name"], "main");
    assert_eq!(aborted["status"]["entries"], json!([]));
    assert_eq!(aborted["can_undo"], false);
    assert_eq!(main_at(&r, &mut s), id(4));
    for id in ["rebase.continue", "rebase.abort"] {
        let err = r
            .run(id, &mut s, json!({ "confirm": true }))
            .or_else(|_| r.run(id, &mut s, json!({})))
            .unwrap_err();
        assert!(err.to_string().contains("not in progress"), "{id}: {err}");
    }
}

#[test]
fn a_rebase_from_the_terminal_is_offered_but_not_journaled() {
    let mut repo = repo();
    repo.set_operation(Some(project_domain::Operation::Rebase));
    let (r, mut s) = opened_with(repo);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["continue"], "rebase.continue");
    assert_eq!(op["abort"], "rebase.abort");
    // The in-memory repo has no data to go on with; the abort works.
    assert!(r.run("rebase.continue", &mut s, json!({})).is_err());
    let aborted = run(&r, &mut s, "rebase.abort", json!({ "confirm": true }));
    assert_eq!(aborted["status"]["operation"], json!(null));
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
    use crate::{
        MergeAbortResult, RebaseContinueResult, RebasePlanResult, RebasePlanStep, RebaseStartResult,
    };
    use project_domain::RebaseAction;
    let mut published = repo();
    published.set_remote_branch("origin/main", oid(2));
    let (r, mut s) = opened_with(published);
    let plan: RebasePlanResult = same(run(&r, &mut s, "rebase.plan", json!({ "base": id(1) })));
    assert_eq!(plan.base, oid(1));
    assert_eq!(plan.branch, "main");
    assert_eq!(plan.published, vec![oid(2)]);
    assert_eq!(
        plan.steps[0],
        RebasePlanStep {
            action: RebaseAction::Pick,
            commit: oid(2),
            summary: "one".into(),
        }
    );
    // Concluded right away.
    let mut reword = step("reword", 4);
    reword["message"] = json!("THREE");
    let out = run(
        &r,
        &mut s,
        "rebase.start",
        start(vec![step("pick", 2), step("pick", 3), reword]),
    );
    assert!(out.get("refs_error").is_none() && out.get("status_error").is_none());
    let done: RebaseStartResult = same(out);
    assert!(!done.in_progress && done.journal.can_undo);
    assert_eq!(done.backup, "refs/project/backup/rebase-1");

    // Stopped, continued (stops again), concluded.
    let (r, mut s, out) = conflicted();
    let stopped: RebaseStartResult = same(out);
    assert!(stopped.in_progress);
    assert_eq!(stopped.conflicts, vec![RepoPath::from("a.txt")]);
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let next: RebaseContinueResult = same(run(&r, &mut s, "rebase.continue", json!({})));
    assert!(next.in_progress);
    assert_eq!(next.command.as_deref(), Some("rebase.start"));
    assert_eq!(next.base, Some(oid(1)));
    assert_eq!(next.before, Some(oid(4)));
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let done: RebaseContinueResult = same(run(&r, &mut s, "rebase.continue", json!({})));
    assert!(!done.in_progress && done.journal.can_undo);
    assert_eq!(done.branch.as_deref(), Some("main"));
    assert_eq!(done.backup.as_deref(), Some("refs/project/backup/rebase-1"));

    // Stopped, then aborted: the shape of `merge.abort`.
    let (r, mut s, _) = conflicted();
    let aborted: MergeAbortResult =
        same(run(&r, &mut s, "rebase.abort", json!({ "confirm": true })));
    assert_eq!(aborted.backup, "refs/project/backup/rebase-abort-1");
    assert_eq!(aborted.files, vec![RepoPath::from("a.txt")]);
}

/// A re-read that fails gives `null` and `<key>_error`; the unknown fields of a rebase from the
/// terminal are `null`.
#[test]
fn reread_error_fields() {
    use super::super::{JournalState, reread};
    use crate::{Message, RebaseContinueResult, RebaseStartResult};
    use project_domain::{RefInfo, Status};
    let error = EngineError::NoRepo.to_string();
    let (status, status_error) = reread::<Status>(Err(EngineError::NoRepo));
    let (refs, refs_error) = reread::<Vec<RefInfo>>(Err(EngineError::NoRepo));
    let result = RebaseStartResult {
        base: oid(1),
        branch: "main".into(),
        before: oid(4),
        after: oid(9),
        in_progress: false,
        conflicts: vec![],
        message: String::new(),
        backup: "refs/project/backup/rebase-1".into(),
        warnings: vec![Message::plain("w")],
        status: status.clone(),
        status_error: status_error.clone(),
        refs: refs.clone(),
        refs_error: refs_error.clone(),
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&result).unwrap();
    for key in ["status", "refs", "next_undo"] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(out["status_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    let back: RebaseStartResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);

    let result = RebaseContinueResult {
        command: None,
        base: None,
        branch: None,
        before: None,
        after: oid(9),
        in_progress: false,
        conflicts: vec![],
        message: String::new(),
        backup: None,
        warnings: vec![],
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::default(),
    };
    let out = serde_json::to_value(&result).unwrap();
    for key in [
        "command", "base", "branch", "before", "backup", "status", "refs",
    ] {
        assert_eq!(out[key], Value::Null, "{key}");
    }
    assert_eq!(out["status_error"], error.as_str());
    assert_eq!(out["refs_error"], error.as_str());
    let back: RebaseContinueResult = serde_json::from_value(out).unwrap();
    assert_eq!(back, result);
}
