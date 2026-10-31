//! Tests of `discard_lines` and `restore_lines` on the `MemoryBackend`.

use super::*;
use crate::{DiffSpec, GitIndex, Journal, JournalEntry, file_digest};
use project_domain::{FileDiff, LineRef};

fn p(path: &str) -> RepoPath {
    RepoPath::from(path)
}

fn file(repo: &MemoryBackend, path: &str) -> Option<FileDiff> {
    let mut diff = repo
        .diff(&DiffSpec::Worktree {
            path: Some(p(path)),
        })
        .unwrap();
    assert!(diff.files.len() <= 1);
    diff.files.pop()
}

fn selection(
    repo: &MemoryBackend,
    path: &str,
    pick: impl FnOnce(&FileDiff) -> Vec<LineRef>,
) -> DiscardLines {
    let f = file(repo, path).expect("file in the diff");
    DiscardLines {
        path: p(path),
        lines: pick(&f),
        digest: file_digest(&f),
    }
}

/// The staged content of `path`.
fn staged(repo: &MemoryBackend, path: &str) -> Vec<u8> {
    let (_, oid) = repo.index_tree()[&p(path)];
    repo.blob(oid).unwrap().to_vec()
}

fn restore(redo: bool, out: &DiscardLinesOutcome) -> LinesRestore {
    LinesRestore {
        command: if redo { "edit.redo" } else { "edit.undo" },
        redo,
        file: out.file.clone(),
    }
}

#[test]
fn every_line_equals_discarding_the_file() {
    for path in ["src/lib.rs", "README.md"] {
        let mut repo = MemoryBackend::sample();
        let index_before = repo.index_tree();
        let before = repo.worktree_file(&p(path)).unwrap().to_vec();
        let sel = selection(&repo, path, FileDiff::line_refs);
        let out = repo.discard_lines(&sel).unwrap();
        assert_eq!(out.backup, "refs/project/backup/discard-1");
        assert_eq!(repo.line_backups()[0].1, p(path));
        assert_eq!(repo.blob(out.file.before).unwrap(), &before[..]);
        assert_eq!(repo.worktree_file(&p(path)).unwrap(), staged(&repo, path));
        assert_eq!(repo.index_tree(), index_before, "the index does not change");
        assert_eq!(file(&repo, path), None, "{path}: no unstaged changes left");
    }
}

#[test]
fn one_line_then_undo_and_redo() {
    let mut repo = MemoryBackend::sample();
    let mut journal = Journal::new();
    let path = "src/lib.rs";
    let status_before = repo.status().unwrap();
    let before = repo.worktree_file(&p(path)).unwrap().to_vec();
    let all = file(&repo, path).unwrap();
    assert!(all.line_refs().len() > 1, "the sample has several lines");
    let first = all.line_refs()[0];
    let sel = selection(&repo, path, |_| vec![first]);
    let out = repo.discard_lines(&sel).unwrap();
    journal.record(JournalEntry::discard_lines(&out, repo.head_state()));
    let after = repo.worktree_file(&p(path)).unwrap().to_vec();
    assert_ne!(after, before);
    assert_eq!(repo.blob(out.file.after).unwrap(), &after[..]);
    // The other changes stay: one changed line less.
    let left = file(&repo, path).unwrap();
    assert_eq!(left.line_refs().len(), all.line_refs().len() - 1);

    // Undo: the bytes before; redo: the bytes written.
    repo.restore_lines(&restore(false, &out)).unwrap();
    assert_eq!(repo.worktree_file(&p(path)).unwrap(), &before[..]);
    assert_eq!(repo.status().unwrap(), status_before);
    // Undo again: the file no longer has the written bytes.
    assert!(matches!(
        repo.restore_lines(&restore(false, &out)),
        Err(EngineError::UndoConflict {
            command: "edit.undo",
            ..
        })
    ));
    repo.restore_lines(&restore(true, &out)).unwrap();
    assert_eq!(repo.worktree_file(&p(path)).unwrap(), &after[..]);

    // Changed before the undo: conflict, nothing written.
    repo.set_worktree_file(path, b"other\n".to_vec());
    assert!(matches!(
        repo.restore_lines(&restore(false, &out)),
        Err(EngineError::UndoConflict { .. })
    ));
    assert_eq!(repo.worktree_file(&p(path)).unwrap(), b"other\n");
}

#[test]
fn undo_of_a_file_back_to_the_index() {
    // Every line discarded: the status entry disappears; undo recreates it.
    let mut repo = MemoryBackend::sample();
    let path = "src/lib.rs";
    let status_before = repo.status().unwrap();
    let sel = selection(&repo, path, FileDiff::line_refs);
    let out = repo.discard_lines(&sel).unwrap();
    assert!(
        !repo
            .status()
            .unwrap()
            .entries
            .iter()
            .any(|e| e.path == p(path))
    );
    repo.restore_lines(&restore(false, &out)).unwrap();
    assert_eq!(repo.status().unwrap(), status_before);
    repo.restore_lines(&restore(true, &out)).unwrap();
    assert_eq!(repo.worktree_file(&p(path)).unwrap(), staged(&repo, path));
}

#[test]
fn refusals_change_nothing() {
    let repo = MemoryBackend::sample();
    let check = |sel: DiscardLines, what: &str| {
        let mut r = repo.clone();
        let err = r.discard_lines(&sel).unwrap_err();
        assert_eq!(r.worktree, repo.worktree, "{what}");
        assert_eq!(r.status().unwrap(), repo.status().unwrap(), "{what}");
        assert!(r.line_backups().is_empty(), "{what}");
        err
    };
    // Stale digest.
    let mut sel = selection(&repo, "src/lib.rs", FileDiff::line_refs);
    sel.digest = "0".repeat(32);
    assert!(matches!(
        check(sel, "digest"),
        EngineError::StaleDiff { .. }
    ));
    // A context line.
    let f = file(&repo, "src/lib.rs").unwrap();
    let context = f.hunks[0]
        .lines
        .iter()
        .find(|l| l.line_ref().is_none())
        .and_then(|l| l.new_line)
        .unwrap();
    let sel = selection(&repo, "src/lib.rs", |_| vec![LineRef::New(context)]);
    assert!(matches!(
        check(sel, "context"),
        EngineError::InvalidParams { .. }
    ));
    // Untracked, added in the stage only.
    for path in ["note.txt", "docs/guide.md"] {
        let sel = DiscardLines {
            path: p(path),
            lines: vec![LineRef::New(1)],
            digest: "0".repeat(32),
        };
        assert!(matches!(check(sel, path), EngineError::NotAllowed(_)));
    }
    // Backup failure: nothing written.
    let mut failing = repo.clone();
    failing.fail_line_backups("cannot lock ref");
    let sel = selection(&repo, "src/lib.rs", FileDiff::line_refs);
    let err = failing.discard_lines(&sel).unwrap_err();
    assert!(err.to_string().contains("cannot lock ref"), "{err}");
    assert_eq!(failing.worktree, repo.worktree);
}
