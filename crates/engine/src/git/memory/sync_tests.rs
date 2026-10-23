//! Test di pull e push direttamente sul `MemoryBackend` (i comandi sono provati in
//! `builtin/sync_tests.rs`).

use super::*;
use crate::git::{HeadState, RefMove};
use project_domain::RepoPath;

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn entry(oid: Oid) -> (u32, Oid) {
    (0o100644, oid)
}

#[test]
fn three_way_merge_of_trees() {
    let path = |p: &str| RepoPath::from(p);
    let base: Tree = [(path("a"), entry(oid(1))), (path("b"), entry(oid(2)))].into();
    let ours: Tree = [(path("a"), entry(oid(3))), (path("b"), entry(oid(2)))].into();
    let theirs: Tree = [(path("a"), entry(oid(1))), (path("c"), entry(oid(4)))].into();
    // `a` cambiato da noi, `b` tolto da loro, `c` aggiunto da loro.
    let merged = merge_trees(&base, &ours, &theirs).unwrap();
    assert_eq!(
        merged,
        [(path("a"), entry(oid(3))), (path("c"), entry(oid(4)))].into()
    );
    let theirs: Tree = [(path("a"), entry(oid(5)))].into();
    assert_eq!(merge_trees(&base, &ours, &theirs).unwrap_err(), ["a"]);
    // Lo stesso cambiamento dai due lati non è un conflitto.
    assert_eq!(merge_trees(&base, &ours, &ours).unwrap(), ours);
}

#[test]
fn upstream_of_the_sample_branches() {
    let mut repo = MemoryBackend::sample();
    let up = repo.upstream("feature/grafo").unwrap().unwrap();
    assert_eq!(up.remote, "origin");
    assert_eq!(up.merge, "refs/heads/feature/grafo");
    assert_eq!(
        up.tracking.as_deref(),
        Some("refs/remotes/origin/feature/grafo")
    );
    assert_eq!(up.merge_branch(), Some("feature/grafo"));
    repo.add_remote("origin/x", "/x");
    repo.set_upstream("main", "origin/x/y");
    let up = repo.upstream("main").unwrap().unwrap();
    assert_eq!(
        (up.remote.as_str(), up.merge.as_str()),
        ("origin/x", "refs/heads/y")
    );
    repo.create_branch("solo", &oid(12)).unwrap();
    assert_eq!(repo.upstream("solo").unwrap(), None);
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
    // 12 rinomina `src/main.rs` e cambia `src/lib.rs`: con solo `note.txt` non tracciato si
    // può tornare a 11, e il file resta.
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
    // L'albero di HEAD ha di nuovo `src/main.rs`, senza modifiche in stage per lui.
    assert!(
        repo.head_tree()
            .contains_key(&RepoPath::from("src/main.rs"))
    );
    assert!(
        !after.iter().any(|e| e.path == RepoPath::from("src/main.rs")
            || e.path == RepoPath::from("src/bin/app.rs"))
    );

    // Il branch spostato da fuori: `UndoConflict`.
    let err = repo.reset_branch(&reset(oid(12), oid(11))).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");

    // Una modifica locale a un file che cambierebbe (`src/lib.rs`): rifiutato, niente cambia.
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
    // Un secondo repo che scarica dal remoto riceve il commit.
    assert!(repo.remotes["origin"].commits.contains_key(&oid(12)));
    // Il lease atteso diverso dal remoto (spostato da un altro): rifiutato.
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
