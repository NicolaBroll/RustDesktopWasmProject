//! Tests of `apply_lines` (stage and unstage of lines) on the `MemoryBackend`.

use super::*;
use crate::{DiffSpec, GitBackend, LineSelection, file_digest};
use project_domain::{FileDiff, LineKind, LineRef, PatchDirection};

fn p(path: &str) -> RepoPath {
    RepoPath::from(path)
}

fn letters(repo: &MemoryBackend) -> Vec<(String, char, char)> {
    repo.status()
        .unwrap()
        .entries
        .iter()
        .map(|e| (e.path.to_string(), e.index.letter(), e.worktree.letter()))
        .collect()
}

fn file(repo: &MemoryBackend, path: &str, direction: PatchDirection) -> Option<FileDiff> {
    let path = Some(p(path));
    let spec = match direction {
        PatchDirection::Stage => DiffSpec::Worktree { path },
        PatchDirection::Unstage => DiffSpec::Index { path },
    };
    let mut diff = repo.diff(&spec).unwrap();
    assert!(diff.files.len() <= 1);
    diff.files.pop()
}

fn selection(
    repo: &MemoryBackend,
    path: &str,
    direction: PatchDirection,
    pick: impl FnOnce(&FileDiff) -> Vec<LineRef>,
) -> LineSelection {
    let f = file(repo, path, direction).expect("file in the diff");
    LineSelection {
        path: p(path),
        lines: pick(&f),
        digest: file_digest(&f),
        direction,
    }
}

/// The staged content of `path`.
fn staged(repo: &MemoryBackend, path: &str) -> Option<Vec<u8>> {
    let status = repo.status().unwrap();
    let entry = status.entries.iter().find(|e| e.path == p(path))?;
    let info = tracked_info(entry)?;
    (info.mode_index != 0).then(|| repo.blob(info.oid_index).unwrap().to_vec())
}

#[test]
fn stage_of_every_line_equals_staging_the_file() {
    for path in ["src/lib.rs", "README.md"] {
        let mut by_lines = MemoryBackend::sample();
        let sel = selection(&by_lines, path, PatchDirection::Stage, FileDiff::line_refs);
        let worktree_before = by_lines.worktree.clone();
        let backup = by_lines.apply_lines(&sel).unwrap();
        assert_eq!(
            by_lines.worktree, worktree_before,
            "the worktree does not change"
        );

        let mut whole = MemoryBackend::sample();
        let expected_backup = whole.stage(&[p(path)]).unwrap();
        assert_eq!(backup, expected_backup, "{path}");
        assert_eq!(letters(&by_lines), letters(&whole), "{path}");
        assert_eq!(staged(&by_lines, path), staged(&whole, path), "{path}");
        assert_eq!(file(&by_lines, path, PatchDirection::Stage), None);
    }
}

#[test]
fn stage_and_unstage_of_one_line() {
    let mut repo = MemoryBackend::sample();
    let original = letters(&repo);
    let path = "src/lib.rs";
    let all = file(&repo, path, PatchDirection::Stage).unwrap();
    let first_added = all
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .find(|l| l.kind == LineKind::Added)
        .unwrap()
        .clone();
    let sel = selection(&repo, path, PatchDirection::Stage, |f| {
        vec![
            f.line_refs()
                .into_iter()
                .find(|r| matches!(r, LineRef::New(_)))
                .unwrap(),
        ]
    });
    assert_eq!(
        repo.apply_lines(&sel).unwrap(),
        None,
        "`.M`: niente a rischio"
    );
    let after: Vec<_> = letters(&repo)
        .into_iter()
        .filter(|(q, _, _)| q == path)
        .collect();
    assert_eq!(after, [(path.to_owned(), 'M', 'M')]);
    // Only that line is staged.
    let in_index = file(&repo, path, PatchDirection::Unstage).unwrap();
    let changed: Vec<_> = in_index
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind != LineKind::Context)
        .map(|l| (l.kind, l.content.clone()))
        .collect();
    assert_eq!(changed, [(LineKind::Added, first_added.content.clone())]);

    // And removed again: back to the starting state. The staged version was only
    // in the index (`MM`): backup.
    let sel = selection(&repo, path, PatchDirection::Unstage, FileDiff::line_refs);
    let backup = repo.apply_lines(&sel).unwrap().expect("backup");
    assert_eq!(repo.backups().last().unwrap().0, backup);
    assert_eq!(letters(&repo), original);
    assert_eq!(file(&repo, path, PatchDirection::Stage), Some(all));
}

#[test]
fn backup_holds_the_replaced_staged_version() {
    let mut repo = MemoryBackend::sample();
    let before = staged(&repo, "README.md").unwrap();
    let sel = selection(&repo, "README.md", PatchDirection::Stage, |f| {
        vec![f.line_refs()[0]]
    });
    let backup = repo.apply_lines(&sel).unwrap().unwrap();
    let (name, versions) = repo.backups().last().unwrap();
    assert_eq!(name, &backup);
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].path, p("README.md"));
    assert_eq!(repo.blob(versions[0].oid).unwrap(), before.as_slice());
    assert_ne!(staged(&repo, "README.md").unwrap(), before);
}

#[test]
fn unstage_lines_of_a_new_file() {
    let mut repo = MemoryBackend::sample();
    let path = "docs/guide.md";
    let content = staged(&repo, path).unwrap();
    let f = file(&repo, path, PatchDirection::Unstage).unwrap();
    assert_eq!(f.change, Change::Added);
    // The file's worktree (unmodified) is not in the map: afterwards it must be there, unchanged.
    let refs = f.line_refs();
    assert!(refs.len() > 1);
    let sel = selection(&repo, path, PatchDirection::Unstage, |_| vec![refs[0]]);
    assert_eq!(
        repo.apply_lines(&sel).unwrap(),
        None,
        "`A.`: niente a rischio"
    );
    assert_eq!(repo.worktree.get(&p(path)), Some(&content));
    let entry: Vec<_> = letters(&repo)
        .into_iter()
        .filter(|(q, _, _)| q == path)
        .collect();
    assert_eq!(entry, [(path.to_owned(), 'A', 'M')]);

    // All the remaining lines: it becomes untracked again.
    let sel = selection(&repo, path, PatchDirection::Unstage, FileDiff::line_refs);
    assert!(repo.apply_lines(&sel).unwrap().is_some(), "`AM`: backup");
    let entry: Vec<_> = letters(&repo)
        .into_iter()
        .filter(|(q, _, _)| q == path)
        .collect();
    assert_eq!(entry, [(path.to_owned(), '?', '?')]);
    assert_eq!(repo.worktree.get(&p(path)), Some(&content));
}

#[test]
fn intent_to_add_file() {
    let mut repo = MemoryBackend::new();
    repo.set_worktree_file("new.txt", "one\ntwo\n");
    repo.set_entries(vec![StatusEntry {
        path: p("new.txt"),
        index: Change::Unmodified,
        worktree: Change::Added,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: 0,
            mode_index: 0,
            mode_worktree: FILE_MODE,
            oid_head: ZERO,
            oid_index: ZERO,
        }),
    }]);
    let sel = selection(&repo, "new.txt", PatchDirection::Stage, |f| {
        vec![f.line_refs()[1]]
    });
    assert_eq!(repo.apply_lines(&sel).unwrap(), None);
    assert_eq!(staged(&repo, "new.txt").unwrap(), b"two\n");
    assert_eq!(letters(&repo), [("new.txt".to_owned(), 'A', 'M')]);
}

#[test]
fn errors_change_nothing() {
    let repo = MemoryBackend::sample();
    let good = selection(&repo, "src/lib.rs", PatchDirection::Stage, |f| {
        vec![f.line_refs()[0]]
    });
    type Expected = fn(&EngineError) -> bool;
    let cases: Vec<(LineSelection, Expected)> = vec![
        (
            LineSelection {
                digest: "0".repeat(32),
                ..good.clone()
            },
            |e| matches!(e, EngineError::StaleDiff { .. }),
        ),
        (
            LineSelection {
                lines: vec![LineRef::New(100_000)],
                ..good.clone()
            },
            |e| matches!(e, EngineError::InvalidParams { .. }),
        ),
        (
            LineSelection {
                path: p("note.txt"),
                ..good.clone()
            },
            |e| matches!(e, EngineError::StaleDiff { .. }),
        ),
        (
            LineSelection {
                direction: PatchDirection::Unstage,
                ..good.clone()
            },
            |e| matches!(e, EngineError::StaleDiff { .. }),
        ),
    ];
    for (sel, expected) in cases {
        let mut copy = repo.clone();
        let err = copy.apply_lines(&sel).unwrap_err();
        assert!(expected(&err), "{err}");
        assert_eq!(copy, repo, "{err}");
    }
}
