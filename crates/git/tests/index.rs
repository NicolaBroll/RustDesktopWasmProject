//! Integration tests of `CliBackend::stage`/`unstage` and of the `index.stage` and
//! `index.unstage` commands on real repos.

mod common;

use common::{Repo, repo_with_changes, repo_with_merge_conflict, repo_with_special_names};
use project_domain::{EntryKind, Operation, RepoPath, Status};
use project_engine::{DiffSpec, EngineError, GitIndex, GitRead, Registry, Session};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// `(path, X, Y)` of every status entry, like the letters of `git status`.
fn entries(status: &Status) -> Vec<(String, char, char)> {
    status
        .entries
        .iter()
        .map(|e| (e.path.to_string(), e.index.letter(), e.worktree.letter()))
        .collect()
}

fn entry(path: &str, x: char, y: char) -> (String, char, char) {
    (path.to_owned(), x, y)
}

fn paths(list: &[&str]) -> Vec<RepoPath> {
    list.iter().map(|p| RepoPath::from(*p)).collect()
}

fn read(repo: &Repo, file: &str) -> Vec<u8> {
    std::fs::read(repo.path().join(file)).unwrap()
}

fn exists(repo: &Repo, file: &str) -> bool {
    repo.path().join(file).symlink_metadata().is_ok()
}

fn is_not_allowed(err: &EngineError) -> bool {
    matches!(err, EngineError::NotAllowed(_))
}

#[test]
fn stage_modification_new_file_and_folder() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", '.', 'M'),
            entry("new.txt", 'A', '.'),
            entry("renamed.txt", 'R', '.'),
            entry("dir/untracked file.txt", '?', '?'),
        ]
    );

    backend.stage(&paths(&["a.txt"])).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries[0].path, RepoPath::from("a.txt"));
    assert_eq!(
        (
            status.entries[0].index.letter(),
            status.entries[0].worktree.letter()
        ),
        ('M', '.')
    );
    // Reads after the write see the new index.
    assert_eq!(repo.git(&["show", ":a.txt"]), "modified");
    let unstaged = backend.diff(&DiffSpec::Worktree { path: None }).unwrap();
    assert!(unstaged.files.is_empty(), "{unstaged:?}");

    // A directory includes the untracked files it contains.
    backend.stage(&paths(&["dir"])).unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", 'M', '.'),
            entry("dir/untracked file.txt", 'A', '.'),
            entry("new.txt", 'A', '.'),
            entry("renamed.txt", 'R', '.'),
        ]
    );
    // The worktree does not change.
    assert_eq!(read(&repo, "a.txt"), b"modified\n");
}

#[test]
fn stage_deletion_and_worktree_rename() {
    let repo = Repo::new();
    repo.commit_file("gone.txt", "to delete\n", "first");
    repo.commit_file(
        "old.txt",
        "content long enough to recognize the rename\n",
        "second",
    );
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
    std::fs::rename(repo.path().join("old.txt"), repo.path().join("new.txt")).unwrap();
    let mut backend = repo.open();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("gone.txt", '.', 'D'),
            entry("old.txt", '.', 'D'),
            entry("new.txt", '?', '?'),
        ]
    );

    backend.stage(&paths(&["gone.txt"])).unwrap();
    backend.stage(&paths(&["old.txt", "new.txt"])).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(
        entries(&status),
        [entry("gone.txt", 'D', '.'), entry("new.txt", 'R', '.')]
    );
    assert!(matches!(
        &status.entries[1].kind,
        EntryKind::Renamed { original, .. } if *original == RepoPath::from("old.txt")
    ));

    // The same paths again: the deletions are already staged, nothing to do
    // (`git add` alone would say "did not match any files").
    backend
        .stage(&paths(&["old.txt", "new.txt", "gone.txt"]))
        .unwrap();
    assert_eq!(backend.status().unwrap(), status);
    assert!(!exists(&repo, "gone.txt"));
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_staged_and_unstaged_byte_for_byte() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    let name = common::NON_UTF8_NAME;
    std::fs::write(repo.path().join(OsStr::from_bytes(name)), "latin\n").unwrap();
    let mut backend = repo.open();

    let path = RepoPath::from_bytes(name);
    backend.stage(std::slice::from_ref(&path)).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries.len(), 1);
    assert_eq!(status.entries[0].path, path);
    assert_eq!(status.entries[0].index.letter(), 'A');

    backend.unstage(std::slice::from_ref(&path)).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries[0].path, path);
    assert!(matches!(status.entries[0].kind, EntryKind::Untracked));
}

#[test]
fn glob_magic_and_dash_names_are_literal() {
    let repo = repo_with_special_names();
    let mut backend = repo.open();
    for name in ["file*.txt", "a[1].txt", ":(top)magic.txt", "-dash.txt"] {
        backend.stage(&paths(&[name])).unwrap();
    }
    let mut staged: Vec<_> = entries(&backend.status().unwrap())
        .into_iter()
        .filter(|(_, x, _)| *x == 'M')
        .map(|(p, _, _)| p)
        .collect();
    staged.sort();
    // `file1.txt` and `a1.txt` stay unstaged: `file*.txt` and `a[1].txt` are not globs.
    assert_eq!(
        staged,
        ["-dash.txt", ":(top)magic.txt", "a[1].txt", "file*.txt"]
    );

    backend
        .unstage(&paths(&["file*.txt", ":(top)magic.txt"]))
        .unwrap();
    let status = entries(&backend.status().unwrap());
    assert!(status.contains(&entry("file*.txt", '.', 'M')), "{status:?}");
    assert!(
        status.contains(&entry(":(top)magic.txt", '.', 'M')),
        "{status:?}"
    );
    assert!(status.contains(&entry("-dash.txt", 'M', '.')), "{status:?}");
    assert!(status.contains(&entry("file1.txt", '.', 'M')), "{status:?}");
}

#[test]
fn ignored_files_are_refused_and_nothing_is_staged() {
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignore the logs");
    repo.write("ok.txt", "ok\n");
    repo.write("debug.log", "noise\n");
    repo.write("dir/inside.txt", "inside\n");
    repo.write("dir/inside.log", "noise\n");
    let mut backend = repo.open();
    let before = backend.status().unwrap();

    let err = backend.stage(&paths(&["ok.txt", "debug.log"])).unwrap_err();
    assert!(is_not_allowed(&err), "{err}");
    assert!(err.to_string().contains("debug.log"), "{err}");
    // Not even `ok.txt` is staged: all or nothing.
    assert_eq!(backend.status().unwrap(), before);

    // In a directory ignored files are skipped, like in git.
    backend.stage(&paths(&["dir"])).unwrap();
    let after = entries(&backend.status().unwrap());
    assert!(
        after.contains(&entry("dir/inside.txt", 'A', '.')),
        "{after:?}"
    );
    assert!(
        repo.git(&["ls-files", "--", "dir"])
            .lines()
            .eq(["dir/inside.txt"]),
        "the ignored file does not enter the index"
    );
}

#[test]
fn unstage_never_touches_the_worktree() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\n", "first");
    repo.commit_file("gone.txt", "to delete\n", "second");
    // `a.txt`: one change staged and another unstaged; `new.txt` added; `gone.txt`
    // deleted in the index.
    repo.write("a.txt", "staged\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "unstaged\n");
    repo.write("new.txt", "new\n");
    repo.stage(&["new.txt"]);
    repo.git(&["rm", "--quiet", "gone.txt"]);
    let mut backend = repo.open();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", 'M', 'M'),
            entry("gone.txt", 'D', '.'),
            entry("new.txt", 'A', '.'),
        ]
    );

    backend
        .unstage(&paths(&["a.txt", "new.txt", "gone.txt"]))
        .unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", '.', 'M'),
            entry("gone.txt", '.', 'D'),
            entry("new.txt", '?', '?'),
        ]
    );
    // The worktree is as it was: nothing restored, nothing lost.
    assert_eq!(read(&repo, "a.txt"), b"unstaged\n");
    assert_eq!(read(&repo, "new.txt"), b"new\n");
    assert!(!exists(&repo, "gone.txt"), "unstage does not restore files");
    // The index is back as in HEAD.
    assert_eq!(repo.git(&["show", ":a.txt"]), "one");
    assert!(
        backend
            .diff(&DiffSpec::Index { path: None })
            .unwrap()
            .files
            .is_empty()
    );
}

#[test]
fn unstage_one_side_of_a_staged_rename() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    backend.unstage(&paths(&["renamed.txt"])).unwrap();
    let status = entries(&backend.status().unwrap());
    assert!(status.contains(&entry("old.txt", 'D', '.')), "{status:?}");
    assert!(
        status.contains(&entry("renamed.txt", '?', '?')),
        "{status:?}"
    );
    assert!(exists(&repo, "renamed.txt"));
}

#[test]
fn unstage_without_commits_leaves_files_untracked() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let mut backend = repo.open();
    backend.stage(&paths(&["a.txt", "b.txt"])).unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [entry("a.txt", 'A', '.'), entry("b.txt", 'A', '.')]
    );

    backend.unstage(&paths(&["a.txt"])).unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [entry("b.txt", 'A', '.'), entry("a.txt", '?', '?')]
    );
    assert_eq!(read(&repo, "a.txt"), b"a\n");
    assert_eq!(read(&repo, "b.txt"), b"b\n");
}

#[test]
fn conflicts_resolve_with_stage_and_refuse_unstage() {
    let repo = repo_with_merge_conflict();
    let mut backend = repo.open();
    let before = backend.status().unwrap();
    assert!(matches!(before.entries[0].kind, EntryKind::Unmerged(_)));
    let content = read(&repo, "a.txt");

    // Unstage would drop the conflict stages: refused, nothing changes.
    let err = backend.unstage(&paths(&["a.txt"])).unwrap_err();
    assert!(is_not_allowed(&err), "{err}");
    assert!(err.to_string().contains("a.txt"), "{err}");
    assert_eq!(backend.status().unwrap(), before);

    // Stage = resolution with the worktree content (here with the conflict markers).
    backend.stage(&paths(&["a.txt"])).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(entries(&status), [entry("a.txt", 'M', '.')]);
    assert_eq!(
        status.operation,
        Some(Operation::Merge),
        "the merge stays in progress"
    );
    assert_eq!(read(&repo, "a.txt"), content);
}

#[test]
fn locked_index_is_an_error_and_the_lock_stays() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    let before = backend.status().unwrap();
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();

    for result in [
        backend.stage(&paths(&["a.txt"])),
        backend.unstage(&paths(&["new.txt"])),
    ] {
        let err = result.unwrap_err();
        let EngineError::IndexLocked { lock: reported } = &err else {
            panic!("expected IndexLocked: {err}");
        };
        assert_eq!(Path::new(reported), lock);
        assert!(lock.exists(), "the lock is never deleted");
    }
    std::fs::remove_file(&lock).unwrap();
    assert_eq!(backend.status().unwrap(), before);
}

#[test]
fn missing_paths_fail_and_change_nothing() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    let before = backend.status().unwrap();

    let err = backend
        .stage(&paths(&["a.txt", "missing.txt"]))
        .unwrap_err();
    let EngineError::Git { command, stderr } = &err else {
        panic!("expected a git error: {err}");
    };
    assert_eq!(command, "add");
    assert!(stderr.contains("missing.txt"), "{stderr}");
    assert_eq!(backend.status().unwrap(), before, "`a.txt` is not staged");

    let err = backend
        .unstage(&paths(&["new.txt", "missing.txt"]))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { command, .. } if command == "restore"),
        "{err}"
    );
    assert_eq!(backend.status().unwrap(), before);

    // An untracked file is not staged: to git it is unknown.
    let err = backend
        .unstage(&paths(&["dir/untracked file.txt"]))
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");

    // No paths: nothing to do, not "everything".
    backend.stage(&[]).unwrap();
    backend.unstage(&[]).unwrap();
    assert_eq!(backend.status().unwrap(), before);
}

#[test]
fn many_paths_go_through_stdin() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    // Long names: together they far exceed the limit of a single argument (128 KiB).
    let names: Vec<String> = (0..1500)
        .map(|i| format!("dir/{}-{i:04}.txt", "x".repeat(150)))
        .collect();
    for name in &names {
        repo.write(name, "?\n");
    }
    let list: Vec<RepoPath> = names.iter().map(|n| RepoPath::from(n.as_str())).collect();
    let mut backend = repo.open();
    backend.stage(&list).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries.len(), names.len());
    assert!(status.entries.iter().all(|e| e.index.letter() == 'A'));
    backend.unstage(&list).unwrap();
    let status = backend.status().unwrap();
    assert!(
        status
            .entries
            .iter()
            .all(|e| matches!(e.kind, EntryKind::Untracked))
    );
}

fn run(registry: &Registry, session: &mut Session, id: &str, params: Value) -> Value {
    registry
        .run(id, session, params)
        .unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn index_commands_on_a_real_repo() {
    let repo = repo_with_changes();
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    let err = registry
        .run("index.stage", &mut session, json!({ "paths": ["a.txt"] }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NoRepo), "{err}");
    run(
        &registry,
        &mut session,
        "repo.open",
        json!({ "path": repo.path_str() }),
    );

    // The result is the new status, like `repo.status`.
    let out = run(
        &registry,
        &mut session,
        "index.stage",
        json!({ "paths": ["a.txt", "dir/untracked file.txt"] }),
    );
    assert_eq!(
        out["status"],
        run(&registry, &mut session, "repo.status", json!({}))
    );
    assert_eq!(out["backup"], Value::Null);
    let status: Status = serde_json::from_value(out["status"].clone()).unwrap();
    assert!(entries(&status).contains(&entry("a.txt", 'M', '.')));

    let out = run(
        &registry,
        &mut session,
        "index.unstage",
        json!({ "paths": ["a.txt"] }),
    );
    assert_eq!(out["status"]["entries"][0]["path"], "a.txt");
    assert_eq!(out["status"]["entries"][0]["index"], "Unmodified");
    assert_eq!(out["status"]["entries"][0]["worktree"], "Modified");
    let diff = run(
        &registry,
        &mut session,
        "diff.worktree",
        json!({ "path": "a.txt" }),
    );
    assert_eq!(diff["files"][0]["change"], "Modified");

    for params in [
        json!({}),
        json!({ "paths": [] }),
        json!({ "paths": "a.txt" }),
        json!({ "path": ["a.txt"] }),
        json!({ "paths": ["a.txt"], "confirm": true }),
        json!({ "paths": [""] }),
        json!({ "paths": ["/etc/passwd"] }),
        json!({ "paths": ["../outside"] }),
        json!({ "paths": ["a/../b"] }),
        json!({ "paths": ["./a.txt"] }),
        json!({ "paths": ["."] }),
        json!({ "paths": ["dir/"] }),
        json!({ "paths": ["a.txt", [97, 0, 98]] }),
    ] {
        for id in ["index.stage", "index.unstage"] {
            let err = registry.run(id, &mut session, params.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{id} {params}: {err}"
            );
        }
    }
}

#[test]
fn folders_with_conflicts_are_refused() {
    let repo = Repo::new();
    repo.commit_file("dir/a.txt", "base\n", "base");
    repo.diverge("other", "dir/a.txt", "ours\n", "theirs\n");
    repo.conflict("other");
    repo.write("dir/new.txt", "new\n");
    let mut backend = repo.open();
    let before = backend.status().unwrap();
    // By directory: the conflict is not resolved silently, and nothing is staged.
    let err = backend.stage(&paths(&["dir"])).unwrap_err();
    assert!(is_not_allowed(&err), "{err}");
    assert!(err.to_string().contains("dir/a.txt"), "{err}");
    assert_eq!(backend.status().unwrap(), before);
    // By name yes (it is the resolution), also together with the directory.
    backend.stage(&paths(&["dir", "dir/a.txt"])).unwrap();
    let after = entries(&backend.status().unwrap());
    assert!(after.contains(&entry("dir/a.txt", 'M', '.')), "{after:?}");
    assert!(after.contains(&entry("dir/new.txt", 'A', '.')), "{after:?}");
}

#[test]
fn folder_with_only_staged_deletions_is_a_no_op() {
    let repo = Repo::new();
    repo.write("dir/a.txt", "a\n");
    repo.write("dir/b.txt", "b\n");
    repo.git(&["add", "-A"]);
    repo.commit("first");
    repo.git(&["rm", "--quiet", "-r", "dir"]);
    assert!(!exists(&repo, "dir"));
    let mut backend = repo.open();
    let before = backend.status().unwrap();
    assert_eq!(backend.stage(&paths(&["dir"])).unwrap(), None);
    assert_eq!(backend.status().unwrap(), before);
}

#[test]
fn broken_head_is_not_an_empty_branch() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let mut backend = repo.open();
    // `main` points to a commit that does not exist.
    std::fs::write(
        repo.path().join(".git/refs/heads/main"),
        "1111111111111111111111111111111111111111\n",
    )
    .unwrap();
    let index = std::fs::read(repo.path().join(".git/index")).unwrap();
    let err = backend.unstage(&paths(&["a.txt", "b.txt"])).unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert_eq!(
        std::fs::read(repo.path().join(".git/index")).unwrap(),
        index,
        "the index is not emptied"
    );
}
