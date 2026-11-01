//! Pull and push tests directly on the `MemoryBackend` (the commands are tested in
//! `builtin/sync_tests.rs`).

use super::*;
use crate::GitRefs;
use crate::GitRemotes;
use crate::git::memory::merge::merge_partial;
use crate::git::{HeadState, RefMove};
use crate::test_util::oid;
use project_domain::RepoPath;

fn entry(oid: Oid) -> (u32, Oid) {
    (0o100644, oid)
}

#[test]
fn three_way_merge_of_trees() {
    let path = |p: &str| RepoPath::from(p);
    let base: Tree = [(path("a"), entry(oid(1))), (path("b"), entry(oid(2)))].into();
    let ours: Tree = [(path("a"), entry(oid(3))), (path("b"), entry(oid(2)))].into();
    let theirs: Tree = [(path("a"), entry(oid(1))), (path("c"), entry(oid(4)))].into();
    // `a` changed by us, `b` removed by them, `c` added by them.
    let (merged, conflicts) = merge_partial(&base, &ours, &theirs);
    assert!(conflicts.is_empty());
    assert_eq!(
        merged,
        [(path("a"), entry(oid(3))), (path("c"), entry(oid(4)))].into()
    );
    let theirs: Tree = [(path("a"), entry(oid(5)))].into();
    let (_, conflicts) = merge_partial(&base, &ours, &theirs);
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, path("a"));
    // The same change on both sides is not a conflict.
    let (merged, conflicts) = merge_partial(&base, &ours, &ours);
    assert!(conflicts.is_empty());
    assert_eq!(merged, ours);
}

#[test]
fn upstream_of_the_sample_branches() {
    let mut repo = MemoryBackend::sample();
    let up = repo.upstream("feature/graph").unwrap().unwrap();
    assert_eq!(up.remote, "origin");
    assert_eq!(up.merge, "refs/heads/feature/graph");
    assert_eq!(
        up.tracking.as_deref(),
        Some("refs/remotes/origin/feature/graph")
    );
    assert_eq!(up.merge_branch(), Some("feature/graph"));
    repo.add_remote("origin/x", "/x");
    repo.set_upstream("main", "origin/x/y");
    let up = repo.upstream("main").unwrap().unwrap();
    assert_eq!(
        (up.remote.as_str(), up.merge.as_str()),
        ("origin/x", "refs/heads/y")
    );
    repo.create_branch("lonely", &oid(12)).unwrap();
    assert_eq!(repo.upstream("lonely").unwrap(), None);
}

#[test]
fn missing_commits_are_newest_first() {
    let repo = MemoryBackend::sample();
    let ids: Vec<Oid> = repo
        .missing_commits(&oid(12), &[oid(10)], 10)
        .unwrap()
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, [oid(12), oid(11)]);
    assert_eq!(
        repo.missing_commits(&oid(12), &[oid(10)], 1).unwrap().len(),
        1
    );
    assert!(
        repo.missing_commits(&oid(10), &[oid(12)], 5)
            .unwrap()
            .is_empty()
    );
}

fn reset(expected: Oid, target: Oid) -> RefRestore {
    RefRestore {
        command: "edit.undo",
        head_expected: HeadState::Branch("refs/heads/main".into()),
        head_target: HeadState::Branch("refs/heads/main".into()),
        refs: vec![RefMove {
            name: "refs/heads/main".into(),
            expected: Some(expected),
            target: Some(target),
        }],
        reflog: "project: undo".into(),
    }
}

#[test]
fn reset_keep_moves_the_branch_with_the_worktree_but_keeps_local_changes() {
    // 12 renames `src/main.rs` and changes `src/lib.rs`: with only the untracked `note.txt` one
    // can go back to 11, and the file stays.
    let mut repo = MemoryBackend::sample();
    let untracked: Vec<_> = repo
        .status()
        .unwrap()
        .entries
        .into_iter()
        .filter(|e| e.path == RepoPath::from("note.txt"))
        .collect();
    repo.set_entries(untracked.clone());
    let out = repo.reset_branch(&reset(oid(12), oid(11))).unwrap();
    assert_eq!(out.backups, ["refs/project/backup/undo-1"]);
    assert_eq!(repo.head().unwrap().target(), Some(oid(11)));
    let after = repo.status().unwrap().entries;
    assert_eq!(after, untracked);
    // The HEAD tree has `src/main.rs` again, with no staged changes for it.
    assert!(
        repo.head_tree()
            .contains_key(&RepoPath::from("src/main.rs"))
    );
    assert!(
        !after.iter().any(|e| e.path == RepoPath::from("src/main.rs")
            || e.path == RepoPath::from("src/bin/app.rs"))
    );

    // The branch moved from outside: `UndoConflict`.
    let err = repo.reset_branch(&reset(oid(12), oid(11))).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");

    // A local change to a file that would change (`src/lib.rs`): rejected, nothing changes.
    let mut repo = MemoryBackend::sample();
    let snapshot = repo.clone();
    let err = repo.reset_branch(&reset(oid(12), oid(11))).unwrap_err();
    assert!(err.to_string().contains("src/lib.rs"), "{err}");
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo, snapshot);
}

#[test]
fn push_copies_the_commits_to_the_remote() {
    let mut repo = MemoryBackend::sample();
    let request = PushRequest {
        remote: "origin".into(),
        branch: "main".into(),
        remote_branch: "main".into(),
        set_upstream: false,
        lease: None,
        backup: None,
    };
    let out = repo.push(&request, &JobContext::new()).unwrap();
    assert_eq!(out.kind, PushKind::FastForward);
    assert_eq!(repo.branch_on_remote("origin", "main"), Some(oid(12)));
    assert_eq!(repo.refs["refs/remotes/origin/main"].target, oid(12));
    // A second repo that downloads from the remote receives the commit.
    assert!(repo.remotes["origin"].commits.contains_key(&oid(12)));
    // The expected lease differs from the remote (moved by someone else): rejected.
    repo.set_branch_on_remote("origin", "main", oid(10));
    let lease = PushRequest {
        lease: Some(Some(oid(11))),
        ..request
    };
    let err = repo.push(&lease, &JobContext::new()).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::StaleLease,
                ..
            }
        ),
        "{err}"
    );
}
