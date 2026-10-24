//! Tests of `worktree.discard` and its undo/redo on the `MemoryBackend`, through the journal.

use super::*;
use crate::{Journal, JournalEntry, discard_plan};

/// The discard as `worktree.discard` does it: the plan read first and then the backend.
fn planned(repo: &mut MemoryBackend, paths: &[RepoPath]) -> Result<DiscardOutcome, EngineError> {
    let plan = discard_plan(&repo.status()?, paths)?;
    repo.discard(paths, &plan)
}

fn p(s: &str) -> RepoPath {
    RepoPath::from(s)
}

/// Discards `paths` and records the entry in the journal.
fn discard(repo: &mut MemoryBackend, journal: &mut Journal, paths: &[&str]) -> DiscardOutcome {
    let paths: Vec<RepoPath> = paths.iter().map(|s| p(s)).collect();
    let out = planned(repo, &paths).unwrap();
    journal.record(JournalEntry::discard(&out, repo.head_state()));
    out
}

#[test]
fn discard_then_undo_and_redo() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    let status_before = repo.status().unwrap();
    let worktree_before = repo.worktree.clone();
    let index_before = repo.index_tree();

    // README.md `MM`, src/lib.rs `.M`, note.txt `??` (inside the root: not requested).
    let out = discard(&mut repo, &mut journal, &["README.md", "src"]);
    let paths: Vec<_> = out.files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(paths, [p("README.md"), p("src/lib.rs")]);
    assert_eq!(out.backup, "refs/project/backup/discard-1");
    assert_eq!(repo.worktree_backups()[0].stash, out.stash);
    // Worktree like the index, index unchanged, untracked intact.
    assert_eq!(repo.index_tree(), index_before);
    let status = repo.status().unwrap();
    assert!(
        status
            .entries
            .iter()
            .all(|e| e.worktree == Change::Unmodified || e.worktree == Change::Untracked)
    );
    let readme = status
        .entries
        .iter()
        .find(|e| e.path == p("README.md"))
        .unwrap();
    assert_eq!(readme.index, Change::Modified, "the staged change stays");
    assert!(!status.entries.iter().any(|e| e.path == p("src/lib.rs")));
    assert_eq!(
        repo.worktree_file(&p("note.txt")),
        worktree_before.get(&p("note.txt")).map(Vec::as_slice)
    );
    let index = repo.index_tree();
    let (_, readme_index) = index[&p("README.md")];
    assert_eq!(repo.worktree_file(&p("README.md")), repo.blob(readme_index));

    let entry = &journal.done()[0];
    assert_eq!(entry.command, "worktree.discard");
    assert_eq!(
        entry.description.to_string(),
        "Discard changes to 2 files (“README.md”, “src/lib.rs”)"
    );
    assert_eq!(
        entry.backup.as_deref(),
        Some("refs/project/backup/discard-1")
    );

    // Undo: everything as before, byte for byte.
    let (undone, outcome) = journal.undo(&mut repo).unwrap();
    assert_eq!(undone.command, "worktree.discard");
    assert!(outcome.backups.is_empty() && outcome.warnings.is_empty());
    assert_eq!(repo.status().unwrap(), status_before);
    assert_eq!(repo.worktree, worktree_before);

    // Redo: discarded again, without another backup.
    journal.redo(&mut repo).unwrap();
    assert_eq!(repo.status().unwrap(), status);
    assert_eq!(repo.worktree_backups().len(), 1);
    journal.undo(&mut repo).unwrap();
    assert_eq!(repo.worktree, worktree_before);
}

#[test]
fn undo_of_a_deleted_file_deletes_it_again() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    // src/lib.rs deleted in the worktree (`.D`).
    let mut entries = repo.status().unwrap().entries;
    let lib = entries
        .iter_mut()
        .find(|e| e.path == p("src/lib.rs"))
        .unwrap();
    lib.worktree = Change::Deleted;
    if let EntryKind::Ordinary(info) = &mut lib.kind {
        info.mode_worktree = 0;
    }
    repo.set_entries(entries);
    repo.worktree.remove(&p("src/lib.rs"));
    let before = (repo.status().unwrap(), repo.worktree.clone());

    let out = discard(&mut repo, &mut journal, &["src/lib.rs"]);
    assert_eq!(out.files[0].change, Change::Deleted);
    assert!(repo.worktree_file(&p("src/lib.rs")).is_some(), "ricreato");
    journal.undo(&mut repo).unwrap();
    assert_eq!((repo.status().unwrap(), repo.worktree.clone()), before);
}

#[test]
fn undo_refused_if_the_file_changed_after_the_discard() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    discard(&mut repo, &mut journal, &["src/lib.rs"]);
    // The file is changed again (and reappears unstaged).
    let mut entries = repo.status().unwrap().entries;
    entries.push(StatusEntry {
        path: p("src/lib.rs"),
        index: Change::Unmodified,
        worktree: Change::Modified,
        kind: EntryKind::Ordinary(project_domain::TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: repo.index_tree()[&p("src/lib.rs")].1,
            oid_index: repo.index_tree()[&p("src/lib.rs")].1,
        }),
    });
    entries.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
    repo.set_entries(entries);
    repo.set_worktree_file("src/lib.rs", "new work\n");
    let snapshot = repo.clone();
    let err = journal.undo(&mut repo).unwrap_err();
    let EngineError::UndoConflict { command, refs } = &err else {
        panic!("{err}");
    };
    assert_eq!(*command, "edit.undo");
    assert!(refs[0].contains("src/lib.rs"), "{refs:?}");
    assert_eq!(repo, snapshot, "niente cambiato");
    assert!(journal.can_undo(), "the entry stays");
}

#[test]
fn undo_refused_if_the_index_changed() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    discard(&mut repo, &mut journal, &["README.md"]);
    // The staged version of README.md goes back as in HEAD.
    repo.unstage(&[p("README.md")]).unwrap();
    let snapshot = repo.clone();
    let err = journal.undo(&mut repo).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].contains("stage")),
        "{err}"
    );
    assert_eq!(repo, snapshot);
}

#[test]
fn redo_refused_if_the_file_changed_after_the_undo() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    discard(&mut repo, &mut journal, &["src/lib.rs"]);
    journal.undo(&mut repo).unwrap();
    repo.set_worktree_file("src/lib.rs", "yet more\n");
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
}

#[test]
fn refusals_change_nothing() {
    let mut repo = MemoryBackend::sample();
    let snapshot = repo.clone();
    for paths in [
        vec![p("note.txt")],
        vec![p("docs")],
        vec![p("does-not-exist")],
        vec![],
    ] {
        let err = planned(&mut repo, &paths).unwrap_err();
        assert!(
            matches!(err, EngineError::NotAllowed(_)),
            "{paths:?}: {err}"
        );
        assert_eq!(repo, snapshot);
    }
    let mut empty = MemoryBackend::new();
    assert!(planned(&mut empty, &[p("a")]).is_err());

    // A plan different from the confirmed one: rejected.
    let plan = discard_plan(&repo.status().unwrap(), &[p("README.md")]).unwrap();
    let err = repo
        .discard(&[p("README.md"), p("src")], &plan)
        .unwrap_err();
    assert!(
        err.to_string().contains("changed since the confirmation"),
        "{err}"
    );
    assert_eq!(repo, snapshot);
}

#[test]
fn undo_refused_if_head_moved() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    discard(&mut repo, &mut journal, &["src/lib.rs"]);
    let head = repo.head().unwrap().target().unwrap();
    repo.detach_head(head);
    let snapshot = repo.clone();
    let err = journal.undo(&mut repo).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].starts_with("HEAD")),
        "{err}"
    );
    assert_eq!(repo, snapshot);
    repo.set_head_branch("main");
    journal.undo(&mut repo).unwrap();
    // And the redo with HEAD moved.
    repo.detach_head(head);
    assert!(matches!(
        journal.redo(&mut repo),
        Err(EngineError::UndoConflict {
            command: "edit.redo",
            ..
        })
    ));
}
