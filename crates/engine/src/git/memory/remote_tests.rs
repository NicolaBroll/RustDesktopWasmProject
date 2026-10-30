//! Remote tests on the `MemoryBackend`, directly on the backend (the command is tested in
//! `builtin/remote.rs`).

use super::*;
use crate::{CancelToken, GitRead, GitRemotes, LogQuery};

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn request(remote: &str, branch: Option<&str>) -> FetchRequest {
    FetchRequest {
        remote: remote.to_owned(),
        branch: branch.map(str::to_owned),
        prune: false,
    }
}

#[test]
fn sample_has_origin_with_one_commit_to_fetch() {
    let mut repo = MemoryBackend::sample();
    assert_eq!(repo.remotes().unwrap(), ["origin"]);
    assert_eq!(
        repo.remote_url("origin"),
        Some("https://example.com/project.git")
    );
    assert!(!repo.commits.contains_key(&oid(13)));
    let out = repo
        .fetch(&request("origin", None), &JobContext::new())
        .unwrap();
    assert_eq!(out.messages.len(), 2, "{:?}", out.messages);
    assert!(repo.commits.contains_key(&oid(13)));
    assert_eq!(
        repo.refs["refs/remotes/origin/feature/graph"].target,
        oid(13)
    );
    // Local branches intact.
    assert_eq!(repo.refs["refs/heads/feature/graph"].target, oid(9));
    // The downloaded commit is only on `origin/feature/graph`: the log of HEAD does not have it,
    // that
    // of all refs does, at the top (it is the most recent).
    let ids = |q: LogQuery| -> Vec<Oid> { repo.log(&q).unwrap().iter().map(|c| c.id).collect() };
    assert!(!ids(LogQuery::all()).contains(&oid(13)));
    let all = ids(LogQuery::all().from_all_refs());
    assert_eq!(all.len(), 13);
    assert!(all.contains(&oid(13)));
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
fn prune_removes_the_branches_gone_and_saves_the_commits_only_they_had() {
    let mut repo = MemoryBackend::sample();
    repo.fetch(&request("origin", None), &JobContext::new())
        .unwrap();
    repo.remove_branch_on_remote("origin", "main");
    repo.remove_branch_on_remote("origin", "feature/graph");
    let prune = FetchRequest {
        prune: true,
        ..request("origin", None)
    };
    let out = repo.fetch(&prune, &JobContext::new()).unwrap();
    assert!(!repo.refs.contains_key("refs/remotes/origin/main"));
    assert!(!repo.refs.contains_key("refs/remotes/origin/feature/graph"));
    // `origin/main` is behind the local `main`: nothing to save. The commit downloaded on
    // `origin/feature/graph` had no other ref.
    assert_eq!(out.backups, ["refs/project/backup/prune-1"]);
    assert_eq!(
        repo.commit_backups(),
        [("refs/project/backup/prune-1".to_owned(), oid(13))]
    );
    assert!(
        out.messages
            .iter()
            .any(|m| m.contains("[deleted]") && m.contains("origin/main")),
        "{:?}",
        out.messages
    );
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
