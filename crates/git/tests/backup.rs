//! Index backup before stage and unstage (§1 and §7): the staged versions that are neither in
//! HEAD nor in the worktree are saved in a `refs/project/backup/...` ref before writing.

mod common;

use common::{Repo, repo_with_merge_conflict};
use project_domain::RepoPath;
use project_engine::{EngineError, GitBackend};

fn paths(list: &[&str]) -> Vec<RepoPath> {
    list.iter().map(|p| RepoPath::from(*p)).collect()
}

/// The backup refs, in order.
fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ]);
    out.lines().map(str::to_owned).collect()
}

/// `git show <rev>:<path>`.
fn show(repo: &Repo, rev: &str, path: &str) -> String {
    repo.git(&["show", &format!("{rev}:{path}")])
}

fn index_bytes(repo: &Repo) -> Vec<u8> {
    std::fs::read(repo.path().join(".git/index")).unwrap()
}

/// A repo with `a.txt` committed.
fn base() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "in HEAD\n", "first");
    repo
}

/// No temporary index left in the git folder.
fn assert_no_temp_index(repo: &Repo) {
    let left: Vec<_> = std::fs::read_dir(repo.path().join(".git"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("project-backup-index")
        })
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn unstage_added_then_deleted_keeps_a_backup() {
    let repo = base();
    repo.write("new.txt", "exists only in the index\n");
    repo.stage(&["new.txt"]);
    std::fs::remove_file(repo.path().join("new.txt")).unwrap();
    let mut backend = repo.open();
    assert_eq!(backend.status().unwrap().entries[0].index.letter(), 'A');

    let backup = backend.unstage(&paths(&["new.txt"])).unwrap().unwrap();
    assert!(backup.starts_with("refs/project/backup/index-"), "{backup}");
    assert!(
        backend.status().unwrap().entries.is_empty(),
        "the file is gone"
    );
    // The content can be recovered from the ref; the commit has HEAD as parent.
    assert_eq!(show(&repo, &backup, "new.txt"), "exists only in the index");
    assert_eq!(repo.rev(&format!("{backup}^")), repo.rev("HEAD"));
    let message = repo.git(&["log", "-1", "--format=%B", &backup]);
    assert!(message.contains("index.unstage"), "{message}");
    assert!(message.contains("new.txt"), "{message}");
    assert_no_temp_index(&repo);
}

#[test]
fn stage_added_then_deleted_keeps_a_backup() {
    let repo = base();
    repo.write("new.txt", "exists only in the index\n");
    repo.stage(&["new.txt"]);
    std::fs::remove_file(repo.path().join("new.txt")).unwrap();
    let mut backend = repo.open();
    let backup = backend.stage(&paths(&["new.txt"])).unwrap().unwrap();
    assert!(backend.status().unwrap().entries.is_empty());
    assert_eq!(show(&repo, &backup, "new.txt"), "exists only in the index");
}

#[test]
fn unstage_modified_then_deleted_keeps_a_backup() {
    let repo = base();
    repo.write("a.txt", "staged version\n");
    repo.stage(&["a.txt"]);
    std::fs::remove_file(repo.path().join("a.txt")).unwrap();
    let mut backend = repo.open();
    let backup = backend.unstage(&paths(&["a.txt"])).unwrap().unwrap();
    let status = backend.status().unwrap();
    assert_eq!(
        (
            status.entries[0].index.letter(),
            status.entries[0].worktree.letter()
        ),
        ('.', 'D')
    );
    assert_eq!(show(&repo, &backup, "a.txt"), "staged version");
}

#[test]
fn staged_version_unique_to_the_index_is_saved_on_stage_and_unstage() {
    let repo = base();
    repo.write("a.txt", "staged version\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "worktree version\n");
    let mut backend = repo.open();

    // Stage of `MM`: the staged version would disappear.
    let first = backend.stage(&paths(&["a.txt"])).unwrap().unwrap();
    assert_eq!(show(&repo, &first, "a.txt"), "staged version");
    assert_eq!(repo.git(&["show", ":a.txt"]), "worktree version");

    repo.write("a.txt", "third version\n");
    let second = backend.unstage(&paths(&["a.txt"])).unwrap().unwrap();
    assert_ne!(first, second, "a backup never overwrites another");
    assert_eq!(show(&repo, &second, "a.txt"), "worktree version");
    assert_eq!(show(&repo, &first, "a.txt"), "staged version");
    assert_eq!(backups(&repo).len(), 2);
}

#[test]
fn files_inside_a_requested_folder_count_too() {
    let repo = base();
    repo.write("dir/x.txt", "x staged\n");
    repo.stage(&["dir/x.txt"]);
    repo.write("dir/x.txt", "x in worktree\n");
    let mut backend = repo.open();
    let backup = backend.stage(&paths(&["dir"])).unwrap().unwrap();
    assert_eq!(show(&repo, &backup, "dir/x.txt"), "x staged");
}

#[test]
fn nothing_at_risk_means_no_backup() {
    let repo = base();
    repo.write("a.txt", "modified\n");
    repo.write("new.txt", "new\n");
    repo.write("staged.txt", "staged and equal in the worktree\n");
    repo.stage(&["staged.txt"]);
    let mut backend = repo.open();
    assert_eq!(backend.stage(&paths(&["a.txt", "new.txt"])).unwrap(), None);
    // `A.`: the staged version is also in the worktree.
    assert_eq!(
        backend.unstage(&paths(&["staged.txt", "a.txt"])).unwrap(),
        None
    );
    assert!(backups(&repo).is_empty());
}

#[test]
fn failed_backup_leaves_the_index_untouched() {
    let repo = base();
    repo.write("a.txt", "staged version\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "worktree version\n");
    // A file where the backup refs folder would go: `update-ref` cannot create it.
    std::fs::write(repo.path().join(".git/refs/project"), "").unwrap();
    let mut backend = repo.open();
    let before = index_bytes(&repo);
    let status = backend.status().unwrap();

    for result in [
        backend.stage(&paths(&["a.txt"])),
        backend.unstage(&paths(&["a.txt"])),
    ] {
        let err = result.unwrap_err();
        assert!(
            matches!(&err, EngineError::Git { command, .. } if command == "update-ref"),
            "{err}"
        );
        assert_eq!(index_bytes(&repo), before, "the index is not touched");
        assert_eq!(backend.status().unwrap(), status);
    }
    assert_eq!(repo.git(&["show", ":a.txt"]), "staged version");
    assert_no_temp_index(&repo);
}

#[test]
fn backup_works_with_conflicts_elsewhere_in_the_index() {
    let repo = repo_with_merge_conflict();
    repo.write("b.txt", "b staged\n");
    repo.stage(&["b.txt"]);
    repo.write("b.txt", "b in worktree\n");
    let mut backend = repo.open();
    // `git write-tree` on the real index would fail because of the conflict in `a.txt`.
    let backup = backend.stage(&paths(&["b.txt"])).unwrap().unwrap();
    assert_eq!(show(&repo, &backup, "b.txt"), "b staged");
    assert_eq!(repo.git(&["show", ":b.txt"]), "b in worktree");
}

#[test]
fn backup_on_a_branch_without_commits_has_no_parent() {
    let repo = Repo::new();
    repo.write("a.txt", "staged\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "in worktree\n");
    let mut backend = repo.open();
    let backup = backend.unstage(&paths(&["a.txt"])).unwrap().unwrap();
    assert_eq!(show(&repo, &backup, "a.txt"), "staged");
    let parents = repo.git(&["log", "-1", "--format=%P", &backup]);
    assert_eq!(parents, "");
}

/// `a.txt` with one version in the index and another in the worktree, hidden by `flag`
/// (`--assume-unchanged` or `--skip-worktree`): `git status` gives `M.`.
fn hidden_worktree_change(flag: &str) -> Repo {
    let repo = base();
    repo.write("a.txt", "staged version\n");
    repo.stage(&["a.txt"]);
    repo.git(&["update-index", flag, "a.txt"]);
    repo.write("a.txt", "worktree version\n");
    repo
}

#[test]
fn assume_unchanged_and_skip_worktree_files_are_at_risk() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let repo = hidden_worktree_change(flag);
        let mut backend = repo.open();
        let status = backend.status().unwrap();
        assert_eq!(
            (
                status.entries[0].index.letter(),
                status.entries[0].worktree.letter()
            ),
            ('M', '.'),
            "{flag}: git does not see the worktree"
        );
        let backup = backend
            .unstage(&paths(&["a.txt"]))
            .unwrap()
            .unwrap_or_else(|| panic!("{flag}: no backup"));
        assert_eq!(show(&repo, &backup, "a.txt"), "staged version", "{flag}");
        assert_eq!(repo.git(&["show", ":a.txt"]), "in HEAD", "{flag}");
    }
}

#[test]
fn split_index_leaves_no_shared_index_behind() {
    let repo = base();
    repo.git(&["config", "core.splitIndex", "true"]);
    repo.git(&["update-index", "--split-index"]);
    let shared = || -> Vec<String> {
        std::fs::read_dir(repo.path().join(".git"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("sharedindex."))
            .collect()
    };
    repo.write("a.txt", "staged version\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "worktree version\n");
    let before = shared();
    assert!(!before.is_empty(), "split index is active");
    let mut backend = repo.open();
    assert!(backend.unstage(&paths(&["a.txt"])).unwrap().is_some());
    let after = shared();
    // The temporary index did not leave a `sharedindex.*` of its own.
    let new: Vec<_> = after.iter().filter(|n| !before.contains(n)).collect();
    assert!(new.is_empty(), "{new:?}");
    assert_no_temp_index(&repo);
}
