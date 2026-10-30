//! Tests of `GitRead::diff` on `MemoryBackend`.

use super::{DiffSpec, GitRead, LogQuery, MemoryBackend};
use crate::EngineError;
use project_domain::{
    Change, Diff, EntryKind, FileDiff, LineKind, Oid, RepoPath, StatusEntry, TrackedInfo,
    parse_diff,
};

fn sample() -> MemoryBackend {
    MemoryBackend::sample()
}

fn commit(rev: &str) -> DiffSpec {
    DiffSpec::Commit {
        rev: rev.to_owned(),
        path: None,
    }
}

fn commit_path(rev: &str, path: &str) -> DiffSpec {
    DiffSpec::Commit {
        rev: rev.to_owned(),
        path: Some(RepoPath::from(path)),
    }
}

fn worktree() -> DiffSpec {
    DiffSpec::Worktree { path: None }
}

fn index() -> DiffSpec {
    DiffSpec::Index { path: None }
}

/// `(status, old path, new path)` of each file.
fn summary(diff: &Diff) -> Vec<(Change, String, String)> {
    diff.files
        .iter()
        .map(|f| (f.change, f.old_path.to_string(), f.new_path.to_string()))
        .collect()
}

fn file<'a>(diff: &'a Diff, path: &str) -> &'a FileDiff {
    diff.files
        .iter()
        .find(|f| f.new_path.to_str() == Some(path))
        .unwrap_or_else(|| panic!("missing {path}"))
}

/// The diff rewritten in git's format and reread by the parser is identical: the fields are those
/// `git diff` would give.
fn assert_round_trip(diff: &Diff) {
    assert_eq!(&parse_diff(&diff.to_patch()).unwrap(), diff);
}

#[test]
fn every_commit_has_the_files_of_commit_show_with_hunks() {
    let repo = sample();
    let zero = Oid::from_bytes([0; Oid::LEN]);
    for c in repo.log(&LogQuery::all()).unwrap() {
        let rev = c.id.to_string();
        let details = repo.show_commit(&rev).unwrap();
        let diff = repo.diff(&commit(&rev)).unwrap();
        let expected: Vec<_> = details
            .files
            .iter()
            .map(|f| (f.change, f.old_path().to_string(), f.path.to_string()))
            .collect();
        assert_eq!(summary(&diff), expected, "{}", c.summary);
        for (f, changed) in diff.files.iter().zip(&details.files) {
            assert!(!f.binary, "{}", f.new_path);
            assert_eq!(f.similarity, changed.score);
            assert_eq!(f.old_oid, Some(changed.old_oid));
            assert_eq!(f.new_oid, Some(changed.new_oid));
            // Only `.gitkeep` is empty: all the other files have lines.
            let empty = f.new_path.to_str() == Some(".gitkeep");
            assert_eq!(f.hunks.is_empty(), empty, "{}", f.new_path);
            match f.change {
                Change::Added => {
                    assert_eq!((f.old_mode, f.new_mode), (None, Some(0o100644)));
                    assert_eq!(f.old_oid, Some(zero));
                }
                Change::Deleted => {
                    assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), None));
                    assert_eq!(f.new_oid, Some(zero));
                }
                _ => assert_eq!((f.old_mode, f.new_mode), (Some(0o100644), Some(0o100644))),
            }
        }
        assert_round_trip(&diff);
    }
}

#[test]
fn rename_with_changes_in_the_head_commit() {
    let repo = sample();
    let diff = repo.diff(&commit("HEAD")).unwrap();
    let renamed = file(&diff, "src/bin/app.rs");
    assert_eq!(renamed.change, Change::Renamed);
    assert_eq!(renamed.old_path.to_str(), Some("src/main.rs"));
    assert_eq!(renamed.similarity, Some(92));
    // One changed line and one added, with the context around.
    assert_eq!(renamed.line_counts(), (2, 1));
    let hunk = &renamed.hunks[0];
    assert_eq!((hunk.old_start, hunk.new_start), (1, 1));
    let removed: Vec<_> = hunk
        .lines
        .iter()
        .filter(|l| l.kind == LineKind::Removed)
        .map(|l| (l.old_line, l.content.to_string()))
        .collect();
    assert_eq!(removed, [(Some(2), "// last change: commit 3".to_owned())]);
    let last = hunk.lines.last().unwrap();
    assert_eq!(last.kind, LineKind::Added);
    assert_eq!(last.content.to_str(), Some("// line added by: commit 12"));

    // Asked for by one of the two paths, it stays a rename.
    for path in ["src/bin/app.rs", "src/main.rs"] {
        let only = repo.diff(&commit_path("HEAD", path)).unwrap();
        assert_eq!(only.files, std::slice::from_ref(renamed), "{path}");
    }
}

#[test]
fn merge_root_and_empty_file() {
    let repo = sample();
    // The merge brings the Cargo.toml of `feature/graph`: the diff is that of the branch, starting
    // from the Cargo.toml of the first parent.
    let merge = repo.diff(&commit("v0.1.0")).unwrap();
    let feature = repo.diff(&commit("feature/graph")).unwrap();
    let cargo = file(&merge, "Cargo.toml");
    assert_eq!(cargo.new_oid, file(&feature, "Cargo.toml").new_oid);
    assert_eq!(cargo.line_counts(), (2, 1));

    let root = repo.log(&LogQuery::all()).unwrap().pop().unwrap();
    let diff = repo.diff(&commit(&root.id.to_string())).unwrap();
    assert_eq!(
        summary(&diff),
        [
            (Change::Added, ".gitignore".into(), ".gitignore".into()),
            (Change::Added, ".gitkeep".into(), ".gitkeep".into()),
        ]
    );
    let gitignore = file(&diff, ".gitignore");
    assert_eq!(gitignore.hunks.len(), 1);
    assert_eq!(gitignore.hunks[0].old_count, 0);
    assert_eq!(gitignore.hunks[0].header(), b"@@ -0,0 +1,4 @@");
    // A new empty file has no hunks and is not binary, as in git.
    let gitkeep = file(&diff, ".gitkeep");
    assert!(gitkeep.hunks.is_empty() && !gitkeep.binary);

    // Deleted in commit 3 (the third from the bottom of the log).
    let structure = repo.log(&LogQuery::all()).unwrap();
    let structure = &structure[structure.len() - 3];
    assert_eq!(structure.summary, "Project structure");
    let deleted = repo
        .diff(&commit_path(&structure.id.to_string(), ".gitkeep"))
        .unwrap();
    assert_eq!(
        summary(&deleted),
        [(Change::Deleted, ".gitkeep".into(), ".gitkeep".into())]
    );
    assert!(deleted.files[0].hunks.is_empty());
}

#[test]
fn index_and_worktree_consistent_with_the_status() {
    let repo = sample();
    let status = repo.status().unwrap();
    let info = |path: &str| match &status
        .entries
        .iter()
        .find(|e| e.path.to_str() == Some(path))
        .unwrap()
        .kind
    {
        EntryKind::Ordinary(info) => info.clone(),
        other => panic!("{other:?}"),
    };

    let staged = repo.diff(&index()).unwrap();
    assert_eq!(
        summary(&staged),
        [
            (Change::Modified, "README.md".into(), "README.md".into()),
            (
                Change::Added,
                "docs/guide.md".into(),
                "docs/guide.md".into()
            ),
        ]
    );
    let readme = file(&staged, "README.md");
    assert_eq!(readme.old_oid, Some(info("README.md").oid_head));
    assert_eq!(readme.new_oid, Some(info("README.md").oid_index));
    assert_eq!(readme.line_counts(), (2, 1));
    // HEAD has the README of commit 6, the last one that changed it.
    assert_eq!(
        readme.hunks[0].lines[1].content.to_str(),
        Some("last change: commit 6")
    );
    let guide = file(&staged, "docs/guide.md");
    assert_eq!(guide.new_oid, Some(info("docs/guide.md").oid_index));
    assert!(!guide.hunks.is_empty());

    // The untracked file (`note.txt`) does not appear.
    let unstaged = repo.diff(&worktree()).unwrap();
    assert_eq!(
        summary(&unstaged),
        [
            (Change::Modified, "README.md".into(), "README.md".into()),
            (Change::Modified, "src/lib.rs".into(), "src/lib.rs".into()),
        ]
    );
    // The worktree starts from the index.
    let readme_wt = file(&unstaged, "README.md");
    assert_eq!(readme_wt.old_oid, Some(info("README.md").oid_index));
    let added: Vec<_> = readme_wt
        .hunks
        .iter()
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == LineKind::Added)
        .map(|l| l.content.to_string())
        .collect();
    assert_eq!(
        added,
        [
            "last change: unstaged change",
            "line added by: unstaged change"
        ]
    );
    let lib = file(&unstaged, "src/lib.rs");
    assert_eq!(lib.old_oid, Some(info("src/lib.rs").oid_index));
    assert!(
        lib.new_oid
            .is_some_and(|id| id != Oid::from_bytes([0; Oid::LEN]))
    );
    assert_round_trip(&staged);
    assert_round_trip(&unstaged);

    // Limited to a single file.
    let path = |p: &str| Some(RepoPath::from(p));
    let only = repo.diff(&DiffSpec::Index {
        path: path("README.md"),
    });
    assert_eq!(only.unwrap().files, std::slice::from_ref(readme));
    let only = repo.diff(&DiffSpec::Worktree {
        path: path("src/lib.rs"),
    });
    assert_eq!(only.unwrap().files, std::slice::from_ref(lib));
    for p in ["note.txt", "src/lib.rs", "src", "none"] {
        let none = repo.diff(&DiffSpec::Index { path: path(p) }).unwrap();
        assert!(none.files.is_empty(), "{p}");
    }
}

#[test]
fn empty_repo_unknown_content_and_unknown_rev() {
    let mut repo = MemoryBackend::new();
    assert_eq!(repo.diff(&worktree()).unwrap(), Diff::default());
    assert_eq!(repo.diff(&index()).unwrap(), Diff::default());
    assert!(matches!(
        repo.diff(&commit("HEAD")),
        Err(EngineError::UnknownRev(_))
    ));

    // Branch without commits: a staged file is added relative to the empty tree. Without the
    // blob content the file shows up as binary.
    let blob = MemoryBackend::fake_oid(77);
    let entry = |path: &str| StatusEntry {
        path: RepoPath::from(path),
        index: Change::Added,
        worktree: Change::Unmodified,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: 0,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: Oid::from_bytes([0; Oid::LEN]),
            oid_index: blob,
        }),
    };
    repo.set_entries(vec![entry("a.txt")]);
    let diff = repo.diff(&index()).unwrap();
    assert_eq!(diff.files[0].change, Change::Added);
    assert!(diff.files[0].binary && diff.files[0].hunks.is_empty());

    repo.set_blob(blob, "one\ntwo");
    let diff = repo.diff(&index()).unwrap();
    let f = &diff.files[0];
    assert!(!f.binary);
    assert_eq!(f.hunks[0].header(), b"@@ -0,0 +1,2 @@");
    assert!(f.hunks[0].lines[1].no_newline);
    assert_round_trip(&diff);

    // A NUL in the content: binary, as for git.
    repo.set_blob(blob, b"\0\x01".to_vec());
    assert!(repo.diff(&index()).unwrap().files[0].binary);

    let sample = sample();
    for rev in ["none", ""] {
        let err = sample.diff(&commit(rev)).unwrap_err();
        assert!(
            matches!(err, EngineError::UnknownRev(ref r) if r == rev),
            "{err}"
        );
    }
}
