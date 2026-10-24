//! Stash tests of the `MemoryBackend` (the commands are in `builtin/stash_tests.rs`).

use super::*;
use crate::{GitBackend, StashPush};

#[test]
fn push_keeps_refs_stash_on_the_top_and_drop_backs_it_up() {
    let mut repo = MemoryBackend::sample();
    let head = repo.head().unwrap().target().unwrap();
    let a = repo
        .stash_push(&StashPush {
            paths: Some(vec![RepoPath::from("src/lib.rs")]),
            ..StashPush::default()
        })
        .unwrap();
    let b = repo.stash_push(&StashPush::default()).unwrap();
    assert_eq!((a.base, b.base), (head, head));
    let ids: Vec<Oid> = repo.stashes().unwrap().iter().map(|s| s.id).collect();
    assert_eq!(ids, [b.id, a.id]);
    let stash_ref = |repo: &MemoryBackend| {
        repo.refs()
            .unwrap()
            .into_iter()
            .find(|r| r.name == STASH_REF)
            .map(|r| r.target)
    };
    assert_eq!(stash_ref(&repo), Some(b.id));
    // The stash commit has the base as first parent and the files of the saved worktree.
    let details = repo.show_commit(&b.id.to_string()).unwrap();
    assert_eq!(details.commit.parents[0], head);
    assert_eq!(details.files.len(), 2, "{:?}", details.files);

    let dropped = repo
        .stash_drop(&StashDrop {
            command: "stash.drop",
            id: b.id,
            backup: Some("stash"),
        })
        .unwrap();
    assert_eq!(
        dropped.backup.as_deref(),
        Some("refs/project/backup/stash-1")
    );
    assert_eq!(
        repo.commit_backups().last(),
        Some(&("refs/project/backup/stash-1".to_owned(), b.id))
    );
    assert_eq!(stash_ref(&repo), Some(a.id));
    let err = repo
        .stash_drop(&StashDrop {
            command: "stash.drop",
            id: b.id,
            backup: None,
        })
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    repo.stash_drop(&StashDrop {
        command: "stash.drop",
        id: a.id,
        backup: None,
    })
    .unwrap();
    assert_eq!(stash_ref(&repo), None);
    // `store` puts it back with its files.
    repo.stash_store(&b.id, "On main: again").unwrap();
    let list = repo.stashes().unwrap();
    assert_eq!(
        (list[0].id, list[0].message.as_str()),
        (b.id, "On main: again")
    );
    let applied = repo
        .stash_apply(&StashApply {
            command: "stash.apply",
            id: b.id,
            restore_index: true,
        })
        .unwrap();
    assert!(applied.conflicts.is_empty());
    assert_eq!(repo.status().unwrap().entries.len(), 3);
}

#[test]
fn set_stash_is_a_stash_without_files() {
    let mut repo = MemoryBackend::sample();
    let head = repo.head().unwrap().target().unwrap();
    repo.set_stash(head);
    let list = repo.stashes().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, head);
    let before = repo.status().unwrap();
    repo.stash_apply(&StashApply {
        command: "stash.apply",
        id: head,
        restore_index: false,
    })
    .unwrap();
    assert_eq!(repo.status().unwrap(), before);
}
