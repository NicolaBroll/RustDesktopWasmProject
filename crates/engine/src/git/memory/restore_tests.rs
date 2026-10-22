//! Test di undo e redo (`restore_refs`) sul `MemoryBackend`, attraverso il diario.

use super::*;
use crate::{CommitRequest, GitBackend, Journal, JournalEntry, LogQuery};
use project_domain::{Head, Operation};

/// Commit (o `amend`) con il messaggio dato, registrato nel diario.
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

    let (entry, backups) = journal.undo(&mut repo).unwrap();
    assert_eq!(
        entry.description,
        format!("Commit {} «Aggiorna»", &new.to_string()[..7])
    );
    assert_eq!(head(&repo), Some(before));
    // Indice e worktree come prima del commit: le modifiche tornano in stage.
    assert_eq!(repo.status().unwrap(), status_before);
    assert_eq!(repo.worktree, worktree_before);
    // Il commit annullato non è più raggiungibile: un ref di backup lo tiene.
    assert_eq!(backups, ["refs/project/backup/undo-1"]);
    assert_eq!(
        repo.commit_backups().last().unwrap(),
        &("refs/project/backup/undo-1".to_owned(), new)
    );
    assert_eq!(entry.undo_backups, backups);
    assert!(!journal.can_undo() && journal.can_redo());

    let (entry, backups) = journal.redo(&mut repo).unwrap();
    assert_eq!(entry.undo_backups, ["refs/project/backup/undo-1"]);
    // Il commit di prima è il genitore del nuovo HEAD: nessun backup.
    assert!(backups.is_empty());
    assert_eq!(head(&repo), Some(new));
    assert_eq!(repo.status().unwrap(), status_after);
    assert_eq!(repo.log(&LogQuery::page(0, 1)).unwrap()[0].id, new);
    assert!(journal.can_undo() && !journal.can_redo());

    // Niente da rifare.
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
    // Un altro programma sposta `main`.
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
    assert!(err.to_string().contains("Niente modificato"), "{err}");
    assert_eq!(repo, snapshot, "niente cambia");
    assert!(journal.can_undo(), "la voce resta");

    // Rimesso com'era, l'undo riesce.
    repo.set_branch("main", new);
    journal.undo(&mut repo).unwrap();

    // HEAD su un altro branch: anche questo è un cambiamento da fuori.
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    commit(&mut repo, &mut journal, CommitRequest::new("Aggiorna"));
    repo.set_head_branch("feature/grafo");
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
    let first = commit(&mut repo, &mut journal, CommitRequest::new("primo"));

    let (entry, backups) = journal.undo(&mut repo).unwrap();
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
        "il file torna aggiunto in stage"
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

    let (entry, backups) = journal.undo(&mut repo).unwrap();
    assert_eq!(entry.backup.as_deref(), Some("refs/project/backup/amend-1"));
    assert_eq!(head(&repo), Some(first));
    // Il commit di `amend` non è più raggiungibile: backup. Il commit riscritto era già nel
    // backup di `amend`.
    assert_eq!(backups, ["refs/project/backup/undo-1"]);
    assert!(
        repo.status()
            .unwrap()
            .entries
            .iter()
            .all(|e| !e.is_staged())
    );

    let (_, backups) = journal.redo(&mut repo).unwrap();
    assert!(
        backups.is_empty(),
        "il commit riscritto ha già il backup di amend"
    );
    assert_eq!(head(&repo), Some(amended));
}

#[test]
fn undo_with_detached_head_moves_head() {
    let mut repo = MemoryBackend::sample();
    let start = head(&repo).unwrap();
    repo.detach_head(start);
    let mut journal = Journal::new();
    let new = commit(&mut repo, &mut journal, CommitRequest::new("staccata"));
    let main = repo.refs().unwrap();

    journal.undo(&mut repo).unwrap();
    assert_eq!(repo.head().unwrap(), Head::Detached(start));
    assert_eq!(repo.refs().unwrap(), main, "nessun branch si sposta");
    // HEAD spostata da fuori: rifiutato.
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
    let a = commit(&mut repo, &mut journal, CommitRequest::new("uno"));
    repo.set_worktree_file("src/lib.rs", "nuovo\n");
    repo.stage(&[RepoPath::from("src/lib.rs")]).unwrap();
    let b = commit(&mut repo, &mut journal, CommitRequest::new("due"));
    // Un tag su `b`: resta raggiungibile, nessun backup.
    repo.set_tag("tenuto", b, None);

    let (_, backups) = journal.undo(&mut repo).unwrap();
    assert!(backups.is_empty());
    assert_eq!(head(&repo), Some(a));
    // Anche `a`, genitore di `b`, resta raggiungibile dal tag.
    let (_, backups) = journal.undo(&mut repo).unwrap();
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
