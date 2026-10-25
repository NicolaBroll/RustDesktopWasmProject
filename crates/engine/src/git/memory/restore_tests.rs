//! Undo and redo tests (`restore_refs`) on the `MemoryBackend`, through the journal.

use super::*;
use crate::{CommitRequest, GitBackend, Journal, JournalEntry, LogQuery};
use project_domain::{Head, Operation};

/// Commit (or `amend`) with the given message, recorded in the journal.
fn commit(repo: &mut MemoryBackend, journal: &mut Journal, request: CommitRequest) -> Oid {
    let amend = request.amend;
    let out = repo.commit(&request).unwrap();
    journal.record(JournalEntry::commit(&out, amend).unwrap());
    out.commit.id
}

fn head(repo: &MemoryBackend) -> Option<Oid> {
    repo.head().unwrap().target()
}

#[test]
fn undo_and_redo_of_a_commit() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    let before = head(&repo).unwrap();
    let status_before = repo.status().unwrap();
    let worktree_before = repo.worktree.clone();
    let new = commit(&mut repo, &mut journal, CommitRequest::new("Aggiorna"));
    let status_after = repo.status().unwrap();

    let (entry, outcome) = journal.undo(&mut repo).unwrap();
    let backups = outcome.backups;
    assert_eq!(
        entry.description.to_string(),
        format!("Commit {} “Aggiorna”", &new.to_string()[..7])
    );
    assert_eq!(head(&repo), Some(before));
    // Index and worktree as before the commit: the changes go back to the stage.
    assert_eq!(repo.status().unwrap(), status_before);
    assert_eq!(repo.worktree, worktree_before);
    // The undone commit is no longer reachable: a backup ref holds it.
    assert_eq!(backups, ["refs/project/backup/undo-1"]);
    assert_eq!(
        repo.commit_backups().last().unwrap(),
        &("refs/project/backup/undo-1".to_owned(), new)
    );
    assert_eq!(entry.undo_backups, backups);
    assert!(!journal.can_undo() && journal.can_redo());

    let (entry, outcome) = journal.redo(&mut repo).unwrap();
    let backups = outcome.backups;
    assert_eq!(entry.undo_backups, ["refs/project/backup/undo-1"]);
    // The previous commit is the parent of the new HEAD: no backup.
    assert!(backups.is_empty());
    assert_eq!(head(&repo), Some(new));
    assert_eq!(repo.status().unwrap(), status_after);
    assert_eq!(repo.log(&LogQuery::page(0, 1)).unwrap()[0].id, new);
    assert!(journal.can_undo() && !journal.can_redo());

    // Nothing to redo.
    assert!(matches!(
        journal.redo(&mut repo),
        Err(EngineError::NotAllowed(_))
    ));
}

#[test]
fn undo_refused_if_the_branch_moved() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    let new = commit(&mut repo, &mut journal, CommitRequest::new("Aggiorna"));
    // Another program moves `main`.
    let other = MemoryBackend::fake_oid(4);
    repo.set_branch("main", other);
    let snapshot = repo.clone();
    let err = journal.undo(&mut repo).unwrap_err();
    let EngineError::UndoConflict { command, refs } = &err else {
        panic!("{err}");
    };
    assert_eq!(*command, "edit.undo");
    assert_eq!(refs.len(), 1);
    assert!(refs[0].contains("refs/heads/main"), "{refs:?}");
    assert!(err.to_string().contains("Nothing changed"), "{err}");
    assert_eq!(repo, snapshot, "nothing changes");
    assert!(journal.can_undo(), "the entry stays");

    // Put back as it was, the undo succeeds.
    repo.set_branch("main", new);
    journal.undo(&mut repo).unwrap();

    // HEAD on another branch: this too is a change from outside.
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    commit(&mut repo, &mut journal, CommitRequest::new("Aggiorna"));
    repo.set_head_branch("feature/graph");
    let err = journal.undo(&mut repo).unwrap_err();
    assert!(err.to_string().contains("HEAD"), "{err}");
}

#[test]
fn redo_refused_if_the_branch_moved_after_undo() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    commit(&mut repo, &mut journal, CommitRequest::new("Aggiorna"));
    journal.undo(&mut repo).unwrap();
    repo.set_branch("main", MemoryBackend::fake_oid(4));
    let snapshot = repo.clone();
    let err = journal.redo(&mut repo).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::UndoConflict {
                command: "edit.redo",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(repo, snapshot);
    assert!(journal.can_redo());
}

#[test]
fn undo_of_the_first_commit_deletes_the_branch() {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    repo.set_worktree_file("a.txt", "a\n");
    repo.set_entries(vec![StatusEntry {
        path: RepoPath::from("a.txt"),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    }]);
    repo.stage(&[RepoPath::from("a.txt")]).unwrap();
    let staged = repo.status().unwrap();
    let mut journal = Journal::new();
    let first = commit(&mut repo, &mut journal, CommitRequest::new("first"));

    let (entry, outcome) = journal.undo(&mut repo).unwrap();
    let backups = outcome.backups;
    assert_eq!(entry.refs[0].before, None);
    assert!(repo.refs().unwrap().is_empty());
    assert_eq!(
        repo.head().unwrap(),
        Head::Branch {
            name: "main".to_owned(),
            target: None
        }
    );
    assert_eq!(
        repo.status().unwrap(),
        staged,
        "the file comes back added in the stage"
    );
    assert_eq!(backups, ["refs/project/backup/undo-1"]);

    journal.redo(&mut repo).unwrap();
    assert_eq!(head(&repo), Some(first));
    assert!(repo.status().unwrap().entries.is_empty());
}

#[test]
fn undo_of_an_amend_goes_back_to_the_rewritten_commit() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    let first = commit(&mut repo, &mut journal, CommitRequest::new("Aggiorna"));
    let amended = commit(
        &mut repo,
        &mut journal,
        CommitRequest::new("Aggiorna meglio").amend(),
    );
    assert_ne!(first, amended);

    let (entry, outcome) = journal.undo(&mut repo).unwrap();
    let backups = outcome.backups;
    assert_eq!(entry.backup.as_deref(), Some("refs/project/backup/amend-1"));
    assert_eq!(head(&repo), Some(first));
    // The `amend` commit is no longer reachable: backup. The rewritten commit was already in the
    // `amend` backup.
    assert_eq!(backups, ["refs/project/backup/undo-1"]);
    assert!(
        repo.status()
            .unwrap()
            .entries
            .iter()
            .all(|e| !e.is_staged())
    );

    let (_, outcome) = journal.redo(&mut repo).unwrap();
    let backups = outcome.backups;
    assert!(
        backups.is_empty(),
        "the rewritten commit already has the amend backup"
    );
    assert_eq!(head(&repo), Some(amended));
}

#[test]
fn undo_with_detached_head_moves_head() {
    let mut repo = MemoryBackend::sample();
    let start = head(&repo).unwrap();
    repo.detach_head(start);
    let mut journal = Journal::new();
    let new = commit(&mut repo, &mut journal, CommitRequest::new("detached"));
    let main = repo.refs().unwrap();

    journal.undo(&mut repo).unwrap();
    assert_eq!(repo.head().unwrap(), Head::Detached(start));
    assert_eq!(repo.refs().unwrap(), main, "no branch moves");
    // HEAD moved from outside: rejected.
    repo.detach_head(MemoryBackend::fake_oid(3));
    assert!(matches!(
        journal.redo(&mut repo),
        Err(EngineError::UndoConflict { .. })
    ));
    repo.detach_head(start);
    journal.redo(&mut repo).unwrap();
    assert_eq!(repo.head().unwrap(), Head::Detached(new));
}

#[test]
fn several_undos_in_a_row_and_no_backup_when_still_reachable() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    let start = head(&repo).unwrap();
    let a = commit(&mut repo, &mut journal, CommitRequest::new("one"));
    repo.set_worktree_file("src/lib.rs", "new\n");
    repo.stage(&[RepoPath::from("src/lib.rs")]).unwrap();
    let b = commit(&mut repo, &mut journal, CommitRequest::new("two"));
    // A tag on `b`: stays reachable, no backup.
    repo.set_tag("kept", b, None);

    let (_, outcome) = journal.undo(&mut repo).unwrap();
    let backups = outcome.backups;
    assert!(backups.is_empty());
    assert_eq!(head(&repo), Some(a));
    // `a` too, parent of `b`, stays reachable from the tag.
    let (_, outcome) = journal.undo(&mut repo).unwrap();
    let backups = outcome.backups;
    assert!(backups.is_empty());
    assert_eq!(head(&repo), Some(start));
    assert!(matches!(
        journal.undo(&mut repo),
        Err(EngineError::NotAllowed(_))
    ));
    journal.redo(&mut repo).unwrap();
    journal.redo(&mut repo).unwrap();
    assert_eq!(head(&repo), Some(b));
}

#[test]
fn refused_during_an_operation() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    commit(&mut repo, &mut journal, CommitRequest::new("Aggiorna"));
    repo.set_operation(Some(Operation::Rebase));
    let snapshot = repo.clone();
    let err = journal.undo(&mut repo).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo, snapshot);
    assert!(journal.can_undo());
}
