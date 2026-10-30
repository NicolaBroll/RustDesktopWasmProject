//! Stage and unstage tests on the `MemoryBackend`.

use super::*;
use crate::{DiffSpec, GitIndex, GitRead};
use project_domain::{Conflict, Diff, LineKind};

fn p(path: &str) -> RepoPath {
    RepoPath::from(path)
}

fn ps(list: &[&str]) -> Vec<RepoPath> {
    list.iter().map(|x| p(x)).collect()
}

/// `(path, X, Y)` of each status entry.
fn entries(repo: &MemoryBackend) -> Vec<(String, char, char)> {
    repo.status()
        .unwrap()
        .entries
        .iter()
        .map(|e| (e.path.to_string(), e.index.letter(), e.worktree.letter()))
        .collect()
}

fn e(path: &str, x: char, y: char) -> (String, char, char) {
    (path.to_owned(), x, y)
}

fn worktree(repo: &MemoryBackend) -> Diff {
    repo.diff(&DiffSpec::Worktree { path: None }).unwrap()
}

fn index(repo: &MemoryBackend) -> Diff {
    repo.diff(&DiffSpec::Index { path: None }).unwrap()
}

fn paths(diff: &Diff) -> Vec<String> {
    diff.files.iter().map(|f| f.new_path.to_string()).collect()
}

/// The content of the lines of a kind of file `path` in the diff.
fn lines(diff: &Diff, path: &str, kind: LineKind) -> Vec<String> {
    diff.files
        .iter()
        .filter(|f| f.new_path == p(path))
        .flat_map(|f| &f.hunks)
        .flat_map(|h| &h.lines)
        .filter(|l| l.kind == kind)
        .map(|l| l.content.to_string())
        .collect()
}

/// Id of the file `path` in HEAD in the sample repo.
fn head_oid(repo: &MemoryBackend, path: &str) -> Oid {
    repo.head_tree()[&p(path)].1
}

#[test]
fn sample_starts_as_documented() {
    let repo = MemoryBackend::sample();
    assert_eq!(
        entries(&repo),
        [
            e("README.md", 'M', 'M'),
            e("docs/guide.md", 'A', '.'),
            e("src/lib.rs", '.', 'M'),
            e("note.txt", '?', '?'),
        ]
    );
    // The HEAD tree rebuilt from the commits agrees with the status entries.
    let head = repo.head_tree();
    assert!(head.contains_key(&p("src/bin/app.rs")));
    assert!(
        !head.contains_key(&p("src/main.rs")),
        "renamed in commit 12"
    );
    assert!(!head.contains_key(&p(".gitkeep")), "deleted in commit 3");
    for entry in &repo.status().unwrap().entries {
        if let Some(info) = tracked_info(entry)
            && info.mode_head != 0
        {
            assert_eq!(head[&entry.path], (info.mode_head, info.oid_head));
        }
    }
}

#[test]
fn stage_updates_status_and_diffs() {
    let mut repo = MemoryBackend::sample();
    repo.stage(&ps(&["README.md", "src/lib.rs", "note.txt"]))
        .unwrap();
    assert_eq!(
        entries(&repo),
        [
            e("README.md", 'M', '.'),
            e("docs/guide.md", 'A', '.'),
            e("note.txt", 'A', '.'),
            e("src/lib.rs", 'M', '.'),
        ]
    );
    assert!(worktree(&repo).files.is_empty());
    let staged = index(&repo);
    assert_eq!(
        paths(&staged),
        ["README.md", "docs/guide.md", "note.txt", "src/lib.rs"]
    );
    // The staged content is the worktree content.
    let readme = lines(&staged, "README.md", LineKind::Added);
    assert!(
        readme.contains(&"line added by: unstaged change".to_owned()),
        "{readme:?}"
    );
    assert_eq!(
        lines(&staged, "note.txt", LineKind::Added),
        ["Notes not to lose."]
    );
}

#[test]
fn unstage_restores_head_and_keeps_the_worktree() {
    let mut repo = MemoryBackend::sample();
    let original = repo.clone();
    repo.unstage(&ps(&["README.md", "docs/guide.md"])).unwrap();
    assert_eq!(
        entries(&repo),
        [
            e("README.md", '.', 'M'),
            e("src/lib.rs", '.', 'M'),
            e("docs/guide.md", '?', '?'),
            e("note.txt", '?', '?'),
        ]
    );
    assert!(index(&repo).files.is_empty());
    // Now everything is unstaged, from HEAD to the worktree: both the change that was staged and
    // the other.
    let added = lines(&worktree(&repo), "README.md", LineKind::Added);
    assert!(
        added
            .iter()
            .any(|l| l.contains("line added by: staged change")),
        "{added:?}"
    );
    assert!(
        added.iter().any(|l| l.contains("unstaged change")),
        "{added:?}"
    );
    // The added file is not lost: staged again, it is identical to before.
    repo.stage(&ps(&["docs/guide.md"])).unwrap();
    assert_eq!(
        repo.diff(&DiffSpec::Index {
            path: Some(p("docs/guide.md"))
        })
        .unwrap()
        .files[0]
            .hunks,
        original
            .diff(&DiffSpec::Index {
                path: Some(p("docs/guide.md"))
            })
            .unwrap()
            .files[0]
            .hunks
    );
}

#[test]
fn stage_then_unstage_is_a_round_trip() {
    let original = MemoryBackend::sample();
    let mut repo = original.clone();
    repo.stage(&ps(&["src/lib.rs", "note.txt"])).unwrap();
    repo.unstage(&ps(&["src/lib.rs", "note.txt"])).unwrap();
    assert_eq!(repo.status().unwrap(), original.status().unwrap());
    assert_eq!(worktree(&repo), worktree(&original));
    assert_eq!(index(&repo), index(&original));
}

#[test]
fn folders_clean_files_and_missing_paths() {
    let mut repo = MemoryBackend::sample();
    let before = repo.status().unwrap();
    // An unchanged tracked file: nothing to do, as in git.
    repo.stage(&ps(&["Cargo.toml"])).unwrap();
    repo.unstage(&ps(&["Cargo.toml"])).unwrap();
    assert_eq!(repo.status().unwrap(), before);

    // An unknown path: error and nothing changes, also for the other paths.
    let err = repo.stage(&ps(&["src/lib.rs", "missing"])).unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { command, stderr } if command == "add" && stderr.contains("missing")),
        "{err}"
    );
    let err = repo.unstage(&ps(&["README.md", "missing"])).unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { command, .. } if command == "restore"),
        "{err}"
    );
    // An untracked file is not known to git: unstage gives an error.
    let err = repo.unstage(&ps(&["note.txt"])).unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert_eq!(repo.status().unwrap(), before);

    // A folder includes the files it contains (`src` does not include `src2`, which is not
    // here: only the paths under `src/`).
    repo.stage(&ps(&["src"])).unwrap();
    assert!(entries(&repo).contains(&e("src/lib.rs", 'M', '.')));
    repo.unstage(&ps(&["docs"])).unwrap();
    assert!(entries(&repo).contains(&e("docs/guide.md", '?', '?')));
    assert!(matches!(
        repo.stage(&ps(&["sr"])),
        Err(EngineError::Git { .. })
    ));
}

#[test]
fn ignored_files_are_refused() {
    let mut repo = MemoryBackend::sample();
    let mut list = repo.status().unwrap().entries;
    list.push(StatusEntry {
        path: p("build/out.log"),
        index: Change::Ignored,
        worktree: Change::Ignored,
        kind: EntryKind::Ignored,
    });
    repo.set_entries(list);
    let before = repo.status().unwrap();
    let err = repo.stage(&ps(&["note.txt", "build/out.log"])).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("build/out.log"), "{err}");
    assert_eq!(
        repo.status().unwrap(),
        before,
        "not even note.txt is staged"
    );
    // Inside a folder it is skipped.
    repo.stage(&ps(&["build"])).unwrap();
    assert_eq!(repo.status().unwrap(), before);
}

#[test]
fn deletions_in_the_worktree() {
    let mut repo = MemoryBackend::sample();
    let cargo = head_oid(&repo, "Cargo.toml");
    let mut list = repo.status().unwrap().entries;
    list.push(StatusEntry {
        path: p("Cargo.toml"),
        index: Change::Unmodified,
        worktree: Change::Deleted,
        kind: EntryKind::Ordinary(TrackedInfo {
            submodule: None,
            mode_head: FILE_MODE,
            mode_index: FILE_MODE,
            mode_worktree: 0,
            oid_head: cargo,
            oid_index: cargo,
        }),
    });
    repo.set_entries(list);
    repo.stage(&ps(&["Cargo.toml"])).unwrap();
    assert!(entries(&repo).contains(&e("Cargo.toml", 'D', '.')));
    let staged = index(&repo);
    let file = staged
        .files
        .iter()
        .find(|f| f.old_path == p("Cargo.toml"))
        .unwrap();
    assert_eq!(file.change, Change::Deleted);
    // Again: the deletion is already staged.
    repo.stage(&ps(&["Cargo.toml"])).unwrap();
    assert!(entries(&repo).contains(&e("Cargo.toml", 'D', '.')));

    repo.unstage(&ps(&["Cargo.toml"])).unwrap();
    assert!(entries(&repo).contains(&e("Cargo.toml", '.', 'D')));
    assert!(
        !repo.worktree.contains_key(&p("Cargo.toml")),
        "not restored"
    );
}

#[test]
fn staged_rename_splits_on_unstage() {
    let mut repo = MemoryBackend::sample();
    let app = head_oid(&repo, "src/bin/app.rs");
    let mut list = repo.status().unwrap().entries;
    list.insert(
        3,
        StatusEntry {
            path: p("src/main.rs"),
            index: Change::Renamed,
            worktree: Change::Unmodified,
            kind: EntryKind::Renamed {
                info: TrackedInfo {
                    submodule: None,
                    mode_head: FILE_MODE,
                    mode_index: FILE_MODE,
                    mode_worktree: FILE_MODE,
                    oid_head: app,
                    oid_index: app,
                },
                original: p("src/bin/app.rs"),
                score: 100,
            },
        },
    );
    repo.set_entries(list);
    let renamed = repo.clone();

    // The two paths staged again: nothing to do.
    repo.stage(&ps(&["src/bin/app.rs", "src/main.rs"])).unwrap();
    assert_eq!(repo.status().unwrap(), renamed.status().unwrap());

    repo.unstage(&ps(&["src/main.rs"])).unwrap();
    let now = entries(&repo);
    assert!(now.contains(&e("src/bin/app.rs", 'D', '.')), "{now:?}");
    assert!(now.contains(&e("src/main.rs", '?', '?')), "{now:?}");

    let mut repo = renamed;
    repo.unstage(&ps(&["src/bin/app.rs", "src/main.rs"]))
        .unwrap();
    let now = entries(&repo);
    assert!(now.contains(&e("src/bin/app.rs", '.', 'D')), "{now:?}");
    assert!(now.contains(&e("src/main.rs", '?', '?')), "{now:?}");
    // The new file stayed in the worktree: it is staged again with the deletion (the
    // `MemoryBackend` does not detect renames: two entries).
    repo.stage(&ps(&["src/bin/app.rs", "src/main.rs"])).unwrap();
    let now = entries(&repo);
    assert!(now.contains(&e("src/bin/app.rs", 'D', '.')), "{now:?}");
    assert!(now.contains(&e("src/main.rs", 'A', '.')), "{now:?}");
}

#[test]
fn conflicts_resolve_on_stage_and_refuse_unstage() {
    let mut repo = MemoryBackend::sample();
    let ours = head_oid(&repo, "Cargo.toml");
    repo.set_entries(vec![StatusEntry {
        path: p("Cargo.toml"),
        index: Change::Unmerged,
        worktree: Change::Unmerged,
        kind: EntryKind::Unmerged(UnmergedInfo {
            conflict: Conflict::BothModified,
            submodule: None,
            mode_base: FILE_MODE,
            mode_ours: FILE_MODE,
            mode_theirs: FILE_MODE,
            mode_worktree: FILE_MODE,
            oid_base: MemoryBackend::fake_oid(77),
            oid_ours: ours,
            oid_theirs: MemoryBackend::fake_oid(78),
        }),
    }]);
    repo.set_worktree_file(
        "Cargo.toml",
        "<<<<<<< HEAD\nnostra\n=======\nloro\n>>>>>>>\n",
    );
    let before = repo.status().unwrap();
    let err = repo.unstage(&ps(&["Cargo.toml"])).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = repo.unstage(&ps(&["Cargo.toml", "README.md"])).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.status().unwrap(), before);

    repo.stage(&ps(&["Cargo.toml"])).unwrap();
    assert_eq!(entries(&repo), [e("Cargo.toml", 'M', '.')]);
    assert_eq!(
        lines(&index(&repo), "Cargo.toml", LineKind::Added),
        ["<<<<<<< HEAD", "nostra", "=======", "loro", ">>>>>>>"]
    );
}

#[test]
fn branch_without_commits() {
    let mut repo = MemoryBackend::new();
    repo.set_worktree_file("a.txt", "a\n");
    repo.set_worktree_file("b.txt", "b\n");
    repo.set_entries(vec![untracked(p("a.txt")), untracked(p("b.txt"))]);
    repo.stage(&ps(&["a.txt", "b.txt"])).unwrap();
    assert_eq!(entries(&repo), [e("a.txt", 'A', '.'), e("b.txt", 'A', '.')]);
    assert_eq!(paths(&index(&repo)), ["a.txt", "b.txt"]);
    repo.unstage(&ps(&["a.txt"])).unwrap();
    assert_eq!(entries(&repo), [e("b.txt", 'A', '.'), e("a.txt", '?', '?')]);
    assert_eq!(repo.worktree[&p("a.txt")], b"a\n");
    assert!(matches!(
        repo.unstage(&ps(&["c.txt"])),
        Err(EngineError::Git { .. })
    ));
}

#[test]
fn staged_versions_unique_to_the_index_are_backed_up() {
    let mut repo = MemoryBackend::sample();
    // README.md is `MM`: the staged version is neither in HEAD nor in the worktree.
    let staged = match &repo.status().unwrap().entries[0].kind {
        EntryKind::Ordinary(info) => info.oid_index,
        other => panic!("{other:?}"),
    };
    let staged_content = repo.blob(staged).unwrap().to_vec();

    // Nothing at risk: no backup.
    assert_eq!(repo.stage(&ps(&["src/lib.rs", "note.txt"])).unwrap(), None);
    assert!(repo.backups().is_empty());

    let name = repo.unstage(&ps(&["README.md"])).unwrap().unwrap();
    assert_eq!(name, "refs/project/backup/index-1");
    let (saved_name, versions) = &repo.backups()[0];
    assert_eq!(saved_name, &name);
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].path, p("README.md"));
    assert_eq!(repo.blob(versions[0].oid).unwrap(), staged_content);

    // `AD` inside a requested folder: that too.
    let mut list = repo.status().unwrap().entries;
    for entry in &mut list {
        if entry.path == p("docs/guide.md") {
            entry.worktree = Change::Deleted;
        }
    }
    repo.set_entries(list);
    let name = repo.stage(&ps(&["docs"])).unwrap().unwrap();
    assert_eq!(name, "refs/project/backup/index-2");
    assert_eq!(repo.backups()[1].1[0].path, p("docs/guide.md"));
    assert!(
        !entries(&repo)
            .iter()
            .any(|(path, ..)| path == "docs/guide.md")
    );
}

#[test]
fn folders_with_conflicts_are_refused_on_stage() {
    let mut repo = MemoryBackend::sample();
    let ours = head_oid(&repo, "src/lib.rs");
    let mut list = repo.status().unwrap().entries;
    list.retain(|e| e.path != p("src/lib.rs"));
    list.push(StatusEntry {
        path: p("src/lib.rs"),
        index: Change::Unmerged,
        worktree: Change::Unmerged,
        kind: EntryKind::Unmerged(UnmergedInfo {
            conflict: Conflict::BothModified,
            submodule: None,
            mode_base: FILE_MODE,
            mode_ours: FILE_MODE,
            mode_theirs: FILE_MODE,
            mode_worktree: FILE_MODE,
            oid_base: ours,
            oid_ours: ours,
            oid_theirs: MemoryBackend::fake_oid(99),
        }),
    });
    repo.set_entries(list);
    let before = repo.status().unwrap();
    let err = repo.stage(&ps(&["src"])).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.status().unwrap(), before);
    repo.stage(&ps(&["src", "src/lib.rs"])).unwrap();
    assert!(entries(&repo).contains(&e("src/lib.rs", 'M', '.')));
}
