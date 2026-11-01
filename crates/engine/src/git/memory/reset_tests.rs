//! [`GitRefs::reset`] on the [`MemoryBackend`]: the checks between the command and the write
//! and the undo/redo refusals (the command flow is in `builtin/reset_tests.rs`).

use crate::test_util::oid;
use crate::{
    EngineError, GitIndex, GitRead, GitRefs, HeadState, Journal, JournalEntry, MemoryBackend,
    RefMove, RefRestore, ResetMode, ResetRequest, reset_files,
};
use project_domain::{Change, Conflict, EntryKind, Oid, RepoPath, StatusEntry, UnmergedInfo};

fn request(mode: ResetMode, expected: Oid, target: Oid, files: Vec<RepoPath>) -> ResetRequest {
    let main = "refs/heads/main".to_owned();
    ResetRequest {
        mode,
        refs: RefRestore {
            command: "branch.reset",
            head_expected: HeadState::Branch(main.clone()),
            head_target: HeadState::Branch(main.clone()),
            refs: vec![RefMove {
                name: main,
                expected: Some(expected),
                target: Some(target),
            }],
            reflog: "project: reset".to_owned(),
        },
        files,
        strict: false,
    }
}

fn main_at(repo: &MemoryBackend) -> Oid {
    repo.refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == "refs/heads/main")
        .unwrap()
        .target
}

#[test]
fn refs_changed_between_the_confirmation_and_the_write() {
    let mut repo = MemoryBackend::sample();
    let files = reset_files(&repo.status().unwrap()).unwrap();
    // The command read `main` on `c11`, but now it is on `c12`.
    let before = repo.clone();
    let err = repo
        .reset(&request(ResetMode::Hard, oid(11), oid(10), files.clone()))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-moved"),
        "{err}"
    );
    assert_eq!(repo, before, "nothing changed");
    // A confirmed plan different from the files now.
    let err = repo
        .reset(&request(
            ResetMode::Hard,
            oid(12),
            oid(10),
            files[1..].to_vec(),
        ))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-files-changed"),
        "{err}"
    );
    assert_eq!(repo, before);
    // A target that is not a commit.
    let err = repo
        .reset(&request(ResetMode::Soft, oid(12), oid(999_999), Vec::new()))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    assert_eq!(repo, before);
}

#[test]
fn hard_saves_conflicted_files_too() {
    let mut repo = MemoryBackend::sample();
    repo.set_worktree_file("a.txt", "<<<<<<< ours\n=======\n>>>>>>> theirs\n");
    let mut entries = repo.status().unwrap().entries;
    entries.push(StatusEntry {
        path: RepoPath::from("a.txt"),
        index: Change::Unmerged,
        worktree: Change::Unmerged,
        kind: EntryKind::Unmerged(UnmergedInfo {
            conflict: Conflict::BothAdded,
            submodule: None,
            mode_base: 0,
            mode_ours: 0o100644,
            mode_theirs: 0o100644,
            mode_worktree: 0o100644,
            oid_base: oid(0),
            oid_ours: oid(1_001),
            oid_theirs: oid(1_002),
        }),
    });
    repo.set_entries(entries);
    let files = reset_files(&repo.status().unwrap()).unwrap();
    assert!(files.contains(&RepoPath::from("a.txt")));
    let out = repo
        .reset(&request(ResetMode::Hard, oid(12), oid(11), files))
        .unwrap();
    let backup = &repo.worktree_backups()[0];
    assert_eq!(Some(&backup.name), out.backup.as_ref());
    assert_eq!(
        backup.files[&RepoPath::from("a.txt")].content.as_deref(),
        Some(&b"<<<<<<< ours\n=======\n>>>>>>> theirs\n"[..])
    );
    assert!(
        repo.status()
            .unwrap()
            .entries
            .iter()
            .all(|e| matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored)),
        "only untracked files are left"
    );
}

#[test]
fn undo_refuses_when_something_changed_from_outside() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    repo.reset(&request(ResetMode::Soft, oid(12), oid(10), Vec::new()))
        .unwrap();
    journal.record(JournalEntry::reset(
        Some("refs/heads/main"),
        oid(12),
        oid(10),
        ResetMode::Soft,
        None,
        None,
    ));
    // `main` moved from a terminal.
    repo.set_branch("main", oid(11));
    let err = journal.undo(&mut repo).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(main_at(&repo), oid(11));
    assert!(journal.can_undo(), "the entry stays");

    // A mixed whose index has staged changes again: the undo would rewrite them.
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    repo.reset(&request(ResetMode::Mixed, oid(12), oid(10), Vec::new()))
        .unwrap();
    journal.record(JournalEntry::reset(
        Some("refs/heads/main"),
        oid(12),
        oid(10),
        ResetMode::Mixed,
        None,
        None,
    ));
    repo.stage(&[RepoPath::from("src/lib.rs")]).unwrap();
    let err = journal.undo(&mut repo).unwrap_err();
    let EngineError::UndoConflict { refs, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(refs[0].key(), "engine-reset-undo-staged");
    assert_eq!(main_at(&repo), oid(10));

    // A hard with local changes made after it: the undo would overwrite them.
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    let mut journal = Journal::new();
    repo.reset(&request(ResetMode::Hard, oid(12), oid(10), Vec::new()))
        .unwrap();
    journal.record(JournalEntry::reset(
        Some("refs/heads/main"),
        oid(12),
        oid(10),
        ResetMode::Hard,
        None,
        None,
    ));
    let mut entries = MemoryBackend::sample().status().unwrap().entries;
    entries.retain(|e| e.path == RepoPath::from("src/lib.rs"));
    repo.set_entries(entries);
    let err = journal.undo(&mut repo).unwrap_err();
    let EngineError::UndoConflict { refs, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(refs[0].key(), "engine-reset-undo-changed");
    assert_eq!(main_at(&repo), oid(10));
}
