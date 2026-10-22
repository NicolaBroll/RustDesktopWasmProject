//! Test dei remoti sul `MemoryBackend`, direttamente sul backend (il comando è provato in
//! `builtin/remote.rs`).

use super::*;
use crate::{CancelToken, GitBackend};

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn request(remote: &str, branch: Option<&str>) -> FetchRequest {
    FetchRequest {
        remote: remote.to_owned(),
        branch: branch.map(str::to_owned),
    }
}

#[test]
fn sample_has_origin_with_one_commit_to_fetch() {
    let mut repo = MemoryBackend::sample();
    assert_eq!(repo.remotes().unwrap(), ["origin"]);
    assert_eq!(
        repo.remote_url("origin"),
        Some("https://example.com/progetto.git")
    );
    assert!(!repo.commits.contains_key(&oid(13)));
    let out = repo
        .fetch(&request("origin", None), &JobContext::new())
        .unwrap();
    assert_eq!(out.messages.len(), 2, "{:?}", out.messages);
    assert!(repo.commits.contains_key(&oid(13)));
    assert_eq!(
        repo.refs["refs/remotes/origin/feature/grafo"].target,
        oid(13)
    );
    // Branch locali intatti.
    assert_eq!(repo.refs["refs/heads/feature/grafo"].target, oid(9));
}

#[test]
fn a_branch_removed_from_the_remote_stays_without_prune() {
    let mut repo = MemoryBackend::sample();
    repo.remove_branch_on_remote("origin", "main");
    repo.fetch(&request("origin", None), &JobContext::new())
        .unwrap();
    assert_eq!(repo.refs["refs/remotes/origin/main"].target, oid(11));
}

#[test]
fn only_the_requested_branch() {
    let mut repo = MemoryBackend::sample();
    repo.fetch(&request("origin", Some("main")), &JobContext::new())
        .unwrap();
    assert!(!repo.commits.contains_key(&oid(13)));
    let err = repo
        .fetch(&request("origin", Some("x")), &JobContext::new())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn failures_and_cancel_change_nothing() {
    let mut repo = MemoryBackend::sample();
    let before = repo.clone();
    repo.fail_fetch("origin", Some(FetchFailure::Auth("no".into())));
    let err = repo
        .fetch(&request("origin", None), &JobContext::new())
        .unwrap_err();
    assert!(matches!(err, EngineError::Auth { .. }), "{err}");
    repo.fail_fetch("origin", None);

    let token = CancelToken::new();
    token.cancel();
    let err = repo
        .fetch(
            &request("origin", None),
            &JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled { .. }), "{err}");
    assert_eq!(repo, before);
}
