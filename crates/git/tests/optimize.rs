//! Integration tests of the commit-graph on the `CliBackend` (`repo.open` → `commit_graph`,
//! `repo.optimize`, S36): before no graph, after the graph is there and the log, refs, status
//! and journal are unchanged (also with a detached HEAD and a merge in progress); graph found
//! through alternates and in a linked worktree; `core.commitGraph=false` refused; empty repo; a
//! stale `commit-graph.lock` (left by a killed git) reported with git's text and left in place;
//! cancellation; shallow repo and replace refs refused.

mod common;

use common::{
    Repo, repo_detached_head, repo_with_all_refs, repo_with_history, repo_with_merge_conflict,
};
use project_engine::{CancelToken, EngineError, GitMaintenance, JobContext, Registry, Session};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn graph_file(objects: &Path) -> PathBuf {
    objects.join("info/commit-graph")
}

fn objects(repo: &Repo) -> PathBuf {
    repo.path().join(".git/objects")
}

fn session(path: &str) -> (Registry, Session, Value) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    let opened = r.run("repo.open", &mut s, json!({ "path": path })).unwrap();
    (r, s, opened)
}

/// Everything a read sees: the log (all refs and stash bases, and from HEAD), refs and status.
fn snapshot(r: &Registry, s: &mut Session) -> Vec<Value> {
    vec![
        r.run("repo.log", s, json!({ "all": true, "stashes": true }))
            .unwrap(),
        r.run("repo.log", s, json!({ "skip": 1, "limit": 2 }))
            .unwrap(),
        r.run("repo.refs", s, json!({})).unwrap(),
        r.run("repo.status", s, json!({})).unwrap(),
    ]
}

#[test]
fn optimize_writes_the_graph_and_the_reads_stay_the_same() {
    let (h, _origin) = repo_with_all_refs();
    let (r, mut s, opened) = session(h.repo.path_str());
    assert_eq!(opened["commit_graph"], false);
    assert!(!graph_file(&objects(&h.repo)).exists());
    let before = snapshot(&r, &mut s);

    let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(out, json!({ "commit_graph": true }));
    assert!(graph_file(&objects(&h.repo)).is_file());
    h.repo.git(&["commit-graph", "verify"]);
    assert_eq!(snapshot(&r, &mut s), before);
    // Nothing to undo: no ref moved.
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["done"], json!([]));

    let (_, _, reopened) = session(h.repo.path_str());
    assert_eq!(reopened["commit_graph"], true);
    // Again: the graph is rewritten, still valid.
    let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(out["commit_graph"], true);
    h.repo.git(&["commit-graph", "verify"]);
}

#[test]
fn detached_head_with_a_commit_on_no_branch() {
    // `--reachable` starts from the refs: the detached commit is outside the graph, git still
    // reads it from the objects.
    let h = repo_detached_head();
    let lone = h
        .repo
        .commit_file("lone.txt", "x\n", "only on detached HEAD");
    let (r, mut s, _) = session(h.repo.path_str());
    let before = snapshot(&r, &mut s);
    r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(snapshot(&r, &mut s), before);
    let log = r.run("repo.log", &mut s, json!({ "limit": 1 })).unwrap();
    assert_eq!(log["commits"][0]["id"], lone.to_string());
}

#[test]
fn merge_in_progress_is_left_as_it_is() {
    let repo = repo_with_merge_conflict();
    let (r, mut s, _) = session(repo.path_str());
    let before = snapshot(&r, &mut s);
    let op = r.run("repo.operation", &mut s, json!({})).unwrap();
    r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(snapshot(&r, &mut s), before);
    assert_eq!(r.run("repo.operation", &mut s, json!({})).unwrap(), op);
}

#[test]
fn graph_of_an_alternate_counts_and_the_write_goes_to_the_own_objects() {
    let h = repo_with_history();
    let clone = Repo::new();
    std::fs::write(
        objects(&clone).join("info/alternates"),
        format!("{}\n", objects(&h.repo).display()),
    )
    .unwrap();
    clone.git(&[
        "fetch",
        "--quiet",
        "--update-head-ok",
        h.repo.path_str(),
        "main:main",
    ]);
    h.repo.git(&["commit-graph", "write", "--reachable"]);

    let (r, mut s, opened) = session(clone.path_str());
    assert_eq!(opened["commit_graph"], true);
    let log = r.run("repo.log", &mut s, json!({})).unwrap();

    std::fs::remove_file(graph_file(&objects(&h.repo))).unwrap();
    let (r, mut s, opened) = session(clone.path_str());
    assert_eq!(opened["commit_graph"], false);
    let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(out["commit_graph"], true);
    assert!(graph_file(&objects(&clone)).is_file());
    assert!(!graph_file(&objects(&h.repo)).exists());
    assert_eq!(r.run("repo.log", &mut s, json!({})).unwrap(), log);
}

#[test]
fn linked_worktree_uses_the_common_object_directory() {
    let h = repo_with_history();
    let tmp = tempfile::tempdir().unwrap();
    let wt = tmp.path().join("wt");
    h.repo.git(&[
        "worktree",
        "add",
        "--quiet",
        wt.to_str().unwrap(),
        "feature",
    ]);
    let (r, mut s, opened) = session(wt.to_str().unwrap());
    assert_eq!(opened["commit_graph"], false);
    let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(out["commit_graph"], true);
    assert!(graph_file(&objects(&h.repo)).is_file());
}

#[test]
fn disabled_by_core_commit_graph() {
    let h = repo_with_history();
    h.repo.git(&["config", "core.commitGraph", "off"]);
    let (r, mut s, opened) = session(h.repo.path_str());
    assert_eq!(opened["commit_graph"], false);
    let err = r.run("repo.optimize", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("core.commitGraph"), "{err}");
    assert!(!graph_file(&objects(&h.repo)).exists());

    // A graph written anyway (git itself writes nothing with the setting) is present but not
    // used.
    h.repo.git(&[
        "-c",
        "core.commitGraph=true",
        "commit-graph",
        "write",
        "--reachable",
    ]);
    let state = h.repo.open().commit_graph().unwrap();
    assert!(state.present && !state.enabled);
    let (_, _, opened) = session(h.repo.path_str());
    assert_eq!(opened["commit_graph"], false);
}

#[test]
fn shallow_repo_is_refused() {
    let h = repo_with_history();
    let clone = Repo::new();
    let url = format!("file://{}", h.repo.path_str());
    clone.git(&[
        "fetch",
        "--quiet",
        "--update-head-ok",
        "--depth=1",
        &url,
        "main:main",
    ]);
    let (r, mut s, opened) = session(clone.path_str());
    assert_eq!(opened["commit_graph"], false);
    let err = r.run("repo.optimize", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(!graph_file(&objects(&clone)).exists());
}

#[test]
fn replace_refs_are_refused() {
    let h = repo_with_history();
    let head = h.repo.git(&["rev-parse", "HEAD"]);
    let parent = h.repo.git(&["rev-parse", "HEAD~1"]);
    h.repo.git(&["replace", &head, &parent]);
    let (r, mut s, opened) = session(h.repo.path_str());
    assert_eq!(opened["commit_graph"], false);
    let err = r.run("repo.optimize", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(!graph_file(&objects(&h.repo)).exists());

    // Replace refs turned off: allowed again, and the graph counts.
    h.repo.git(&["config", "core.useReplaceRefs", "false"]);
    let (r, mut s, _) = session(h.repo.path_str());
    let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(out["commit_graph"], true);
}

#[test]
fn empty_repo_has_nothing_to_write() {
    let repo = Repo::new();
    let (r, mut s, _) = session(repo.path_str());
    let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
    assert_eq!(out["commit_graph"], false);
}

#[test]
fn stale_lock_is_reported_and_left_in_place() {
    let h = repo_with_history();
    h.repo.git(&["commit-graph", "write", "--reachable"]);
    let graph = std::fs::read(graph_file(&objects(&h.repo))).unwrap();
    h.repo.commit_file("c.txt", "c\n", "after the graph");
    let lock = objects(&h.repo).join("info/commit-graph.lock");
    std::fs::write(&lock, b"partial").unwrap();

    let (r, mut s, _) = session(h.repo.path_str());
    let err = r.run("repo.optimize", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err:?}");
    assert!(err.to_string().contains("commit-graph.lock"), "{err}");
    // The lock may belong to a running git: not removed; the old graph is untouched.
    assert_eq!(std::fs::read(&lock).unwrap(), b"partial");
    assert_eq!(std::fs::read(graph_file(&objects(&h.repo))).unwrap(), graph);
}

#[test]
fn cancelled_job_stops_git_without_a_lock() {
    let h = repo_with_history();
    let token = CancelToken::new();
    token.cancel();
    let job = JobContext::new().with_cancel(token);
    let err = h.repo.open().write_commit_graph(&job).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Cancelled {
                command: "repo.optimize"
            }
        ),
        "{err}"
    );
    assert!(!objects(&h.repo).join("info/commit-graph.lock").exists());
    assert!(!graph_file(&objects(&h.repo)).exists());
}

/// Cancelled while git runs (at its first progress report): git gets SIGTERM and removes its
/// lock. On a small repo git may finish first; either way no lock is left and the graph, if
/// any, is the old one or a complete new one.
#[test]
fn cancel_while_git_runs_leaves_no_lock() {
    let h = repo_with_history();
    h.repo.git(&["commit-graph", "write", "--reachable"]);
    let old = std::fs::read(graph_file(&objects(&h.repo))).unwrap();
    h.repo.commit_file("c.txt", "c\n", "after the graph");
    let token = CancelToken::new();
    let cancel = token.clone();
    let job = JobContext::new()
        .with_cancel(token)
        .with_progress(move |_| cancel.cancel());
    let result = h.repo.open().write_commit_graph(&job);
    assert!(
        matches!(result, Ok(()) | Err(EngineError::Cancelled { .. })),
        "{result:?}"
    );
    assert!(!objects(&h.repo).join("info/commit-graph.lock").exists());
    let graph = std::fs::read(graph_file(&objects(&h.repo))).unwrap();
    if result.is_err() {
        assert_eq!(graph, old);
    }
    h.repo.git(&["commit-graph", "verify"]);
}
