//! Integration tests of `worktree.clean` (§7) with the `CliBackend` on real repos: mandatory
//! confirmation, backup commit with the exact bytes and mode (CRLF with `core.autocrlf`, binary,
//! executable bit, links) that survives `git gc`, the user's index untouched, empty folders,
//! undo/redo and their refusals, refusals (tracked, ignored, nested repositories), names that
//! look like options or pathspecs, non-UTF-8 names, backup failure.

mod common;

use common::Repo;
use project_domain::RepoPath;
use project_engine::{CleanRestore, EngineError, GitIndex, Registry, Session};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn clean(r: &Registry, s: &mut Session, paths: Value) -> Result<Value, EngineError> {
    r.run(
        "worktree.clean",
        s,
        json!({ "paths": paths, "confirm": true }),
    )
}

/// What a refused clean must not touch: status, index file bytes, refs, and the listed files.
fn snapshot(repo: &Repo, files: &[&str]) -> (String, Vec<u8>, String, Vec<Option<Vec<u8>>>) {
    (
        repo.git(&[
            "status",
            "--porcelain=v2",
            "--untracked-files=all",
            "--ignored",
        ]),
        std::fs::read(repo.path().join(".git/index")).unwrap(),
        repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        files.iter().map(|f| bytes(repo, f)).collect(),
    )
}

fn bytes(repo: &Repo, file: &str) -> Option<Vec<u8>> {
    std::fs::read(repo.path().join(file)).ok()
}

fn exists(repo: &Repo, file: &str) -> bool {
    std::fs::symlink_metadata(repo.path().join(file)).is_ok()
}

/// A repo with `t.txt` and `d/tracked.txt` committed.
fn base() -> Repo {
    let repo = Repo::new();
    repo.write("t.txt", "tracked\n");
    repo.write("d/tracked.txt", "tracked in d\n");
    repo.git(&["add", "-A"]);
    repo.commit("first");
    repo
}

#[test]
fn files_and_folders_are_deleted_with_backup_and_undo_restores_them() {
    let repo = base();
    repo.write("a.txt", "a\n");
    repo.write("d/new.txt", "new in d\n");
    repo.write("d/e/f/deep.txt", "deep\n");
    repo.write("u/only/x.txt", "x\n");
    std::fs::create_dir_all(repo.path().join("d/empty")).unwrap();
    std::fs::create_dir_all(repo.path().join("empty-elsewhere")).unwrap();
    let index = std::fs::read(repo.path().join(".git/index")).unwrap();
    let (r, mut s) = open(&repo);

    let out = clean(&r, &mut s, json!(["a.txt", "d", "u"])).unwrap();
    assert_eq!(
        out["deleted"],
        json!(["a.txt", "d/e/f/deep.txt", "d/new.txt", "u/only/x.txt"])
    );
    assert_eq!(out["bytes"], 2 + 5 + 9 + 2);
    assert_eq!(out["dirs"], json!(["d/e/f", "d/e", "u/only", "u"]));
    assert_eq!(out["can_undo"], true);
    assert_eq!(out["warnings"], json!([]));
    for gone in ["a.txt", "d/new.txt", "d/e", "u"] {
        assert!(!exists(&repo, gone), "{gone}");
    }
    // Tracked files, the folder that still has them, and empty folders not left empty by the
    // deletion stay.
    for kept in ["t.txt", "d/tracked.txt", "d/empty", "empty-elsewhere"] {
        assert!(exists(&repo, kept), "{kept}");
    }
    // The user's index is not touched.
    assert_eq!(
        std::fs::read(repo.path().join(".git/index")).unwrap(),
        index
    );

    // The backup: a ref that survives gc, a commit without parent with exactly those files.
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/clean-"), "{backup}");
    repo.git(&["gc", "--quiet", "--prune=now"]);
    assert_eq!(repo.git(&["rev-parse", &backup]), out["commit"]);
    assert_eq!(
        repo.git(&["rev-list", "--parents", "-n", "1", &backup]),
        out["commit"]
    );
    assert_eq!(
        repo.git(&["ls-tree", "-r", "--name-only", &backup]),
        "a.txt\nd/e/f/deep.txt\nd/new.txt\nu/only/x.txt"
    );
    assert_eq!(
        repo.git(&["show", &format!("{backup}:d/e/f/deep.txt")]),
        "deep"
    );

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "worktree.clean");
    assert_eq!(undo["warnings"], json!([]));
    assert_eq!(bytes(&repo, "d/e/f/deep.txt").unwrap(), b"deep\n");
    assert_eq!(bytes(&repo, "u/only/x.txt").unwrap(), b"x\n");
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a\n");
    assert_eq!(
        std::fs::read(repo.path().join(".git/index")).unwrap(),
        index
    );

    run(&r, &mut s, "edit.redo", json!({}));
    assert!(!exists(&repo, "a.txt") && !exists(&repo, "u"));
    assert!(exists(&repo, "d/tracked.txt"));
}

#[test]
fn without_confirm_nothing_changes() {
    let repo = base();
    repo.write("a.txt", "abc\n");
    repo.write("n/b.txt", "b\n");
    let before = snapshot(&repo, &["a.txt", "n/b.txt"]);
    let (r, mut s) = open(&repo);
    let err = r
        .run("worktree.clean", &mut s, json!({ "paths": ["a.txt", "n"] }))
        .unwrap_err();
    match &err {
        EngineError::ConfirmRequired {
            command, reason, ..
        } => {
            assert_eq!(*command, "worktree.clean");
            let text = reason.to_string();
            assert!(text.contains("2 untracked files"), "{text}");
            assert!(text.contains("`a.txt`, `n/b.txt`"), "{text}");
            assert!(text.contains("6 bytes"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(snapshot(&repo, &["a.txt", "n/b.txt"]), before);
}

#[test]
fn exact_bytes_mode_and_links_come_back() {
    let repo = base();
    repo.git(&["config", "core.autocrlf", "true"]);
    std::fs::write(repo.path().join("crlf.txt"), b"one\r\ntwo\r\n").unwrap();
    std::fs::write(repo.path().join("lf.txt"), b"one\ntwo\n").unwrap();
    std::fs::write(repo.path().join("bin.dat"), b"\0\x01\xff\xfe\r\n\0").unwrap();
    repo.write("script.sh", "#!/bin/sh\necho hi\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = repo.path().join("script.sh");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("t.txt", repo.path().join("link")).unwrap();
        // A link to a folder: deleted as a link, the folder and its files stay.
        std::os::unix::fs::symlink("d", repo.path().join("dlink")).unwrap();
        std::os::unix::fs::symlink("missing", repo.path().join("dangling")).unwrap();
    }
    let files = [
        "bin.dat",
        "crlf.txt",
        "dangling",
        "dlink",
        "lf.txt",
        "link",
        "script.sh",
    ];
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!(files)).unwrap();
    assert_eq!(out["deleted"], json!(files));
    for f in files {
        assert!(!exists(&repo, f), "{f}");
    }
    assert_eq!(bytes(&repo, "t.txt").unwrap(), b"tracked\n");
    assert_eq!(bytes(&repo, "d/tracked.txt").unwrap(), b"tracked in d\n");

    let backup = out["backup"].as_str().unwrap();
    let tree = repo.git(&["ls-tree", "-r", backup]);
    #[cfg(unix)]
    {
        assert!(tree.contains("100755 blob"), "{tree}");
        assert!(tree.contains("120000 blob"), "{tree}");
    }
    // The blob has the CRLF bytes, not the normalized ones.
    let crlf = std::process::Command::new("git")
        .args(["-C", repo.path_str(), "cat-file", "blob"])
        .arg(format!("{backup}:crlf.txt"))
        .output()
        .unwrap();
    assert_eq!(crlf.stdout, b"one\r\ntwo\r\n");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "crlf.txt").unwrap(), b"one\r\ntwo\r\n");
    assert_eq!(bytes(&repo, "lf.txt").unwrap(), b"one\ntwo\n");
    assert_eq!(bytes(&repo, "bin.dat").unwrap(), b"\0\x01\xff\xfe\r\n\0");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(repo.path().join("script.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o100, 0, "{mode:o}");
        let lf = std::fs::metadata(repo.path().join("lf.txt"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(lf & 0o111, 0, "{lf:o}");
        for (link, target) in [("link", "t.txt"), ("dlink", "d"), ("dangling", "missing")] {
            let read = std::fs::read_link(repo.path().join(link)).unwrap();
            assert_eq!(read, Path::new(target), "{link}");
        }
    }
    // Redo: the files match the backup again, they go.
    run(&r, &mut s, "edit.redo", json!({}));
    assert!(!exists(&repo, "crlf.txt") && !exists(&repo, "dlink"));
    assert!(exists(&repo, "d/tracked.txt"));
}

#[test]
fn tracked_ignored_and_missing_paths_are_refused() {
    let repo = base();
    repo.write(".gitignore", "*.log\n");
    repo.stage(&[".gitignore"]);
    repo.commit("ignore");
    repo.write("a.txt", "a\n");
    repo.write("x.log", "ignored\n");
    repo.write("t.txt", "tracked, modified\n");
    let files = ["a.txt", "x.log", "t.txt"];
    let before = snapshot(&repo, &files);
    let (r, mut s) = open(&repo);
    for (paths, needle) in [
        (json!(["x.log"]), "`x.log`"),
        (json!(["a.txt", "t.txt"]), "`t.txt`"),
        (json!(["a.txt", "d/tracked.txt"]), "`d/tracked.txt`"),
        (json!(["a.txt", "nope.txt"]), "`nope.txt`"),
        (json!(["d"]), "`d`"),
    ] {
        let err = clean(&r, &mut s, paths.clone()).unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.contains(needle)),
            "{paths}: {err}"
        );
        assert_eq!(snapshot(&repo, &files), before, "{paths}");
    }
}

#[test]
fn ignored_files_inside_a_folder_stay_with_their_folder() {
    let repo = base();
    repo.write(".gitignore", "*.log\n");
    repo.stage(&[".gitignore"]);
    repo.commit("ignore");
    repo.write("logs/a.txt", "a\n");
    repo.write("logs/b.log", "ignored\n");
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!(["logs"])).unwrap();
    assert_eq!(out["deleted"], json!(["logs/a.txt"]));
    assert_eq!(out["dirs"], json!([]));
    assert_eq!(bytes(&repo, "logs/b.log").unwrap(), b"ignored\n");
}

#[test]
fn nested_repositories_are_refused() {
    let repo = base();
    repo.write("outer/f.txt", "f\n");
    let nested = repo.path().join("outer/inner");
    std::fs::create_dir_all(&nested).unwrap();
    repo.git(&["init", "--quiet", nested.to_str().unwrap()]);
    std::fs::write(nested.join("own.txt"), "own\n").unwrap();
    let files = ["outer/f.txt", "outer/inner/own.txt"];
    let before = snapshot(&repo, &files);
    let (r, mut s) = open(&repo);
    for paths in [
        json!(["outer"]),
        json!(["outer/inner"]),
        json!(["outer/inner/own.txt"]),
    ] {
        let err = clean(&r, &mut s, paths.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{paths}: {err}");
        assert_eq!(snapshot(&repo, &files), before, "{paths}");
        assert!(nested.join(".git").exists());
    }
    // The file next to it, by name, is fine.
    let out = clean(&r, &mut s, json!(["outer/f.txt"])).unwrap();
    assert_eq!(out["deleted"], json!(["outer/f.txt"]));
    assert!(nested.join("own.txt").exists());
}

#[test]
fn names_are_taken_literally() {
    let repo = base();
    for name in ["-rf", "--help", "*", "a[1].txt", "a1.txt", ":(top)magic"] {
        repo.write(name, &format!("{name}\n"));
    }
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!(["-rf", "--help", "*", "a[1].txt"])).unwrap();
    assert_eq!(out["deleted"], json!(["*", "--help", "-rf", "a[1].txt"]));
    for kept in ["a1.txt", ":(top)magic", "t.txt"] {
        assert!(exists(&repo, kept), "{kept}");
    }
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "*").unwrap(), b"*\n");
    assert_eq!(bytes(&repo, "-rf").unwrap(), b"-rf\n");
}

#[cfg(unix)]
#[test]
fn non_utf8_names() {
    use std::os::unix::ffi::OsStrExt;
    let repo = base();
    let name: &[u8] = b"caf\xe9.txt";
    std::fs::write(repo.path().join(std::ffi::OsStr::from_bytes(name)), b"x\n").unwrap();
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!([name])).unwrap();
    let deleted: Vec<RepoPath> = serde_json::from_value(out["deleted"].clone()).unwrap();
    assert_eq!(deleted, [RepoPath::from_bytes(name)]);
    assert!(
        std::fs::symlink_metadata(repo.path().join(std::ffi::OsStr::from_bytes(name))).is_err()
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(
        std::fs::read(repo.path().join(std::ffi::OsStr::from_bytes(name))).unwrap(),
        b"x\n"
    );
}

#[test]
fn show_untracked_normal_still_lists_every_file() {
    let repo = base();
    repo.git(&["config", "status.showUntrackedFiles", "normal"]);
    repo.write("u/a.txt", "a\n");
    repo.write("u/v/b.txt", "b\n");
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!(["u"])).unwrap();
    assert_eq!(out["deleted"], json!(["u/a.txt", "u/v/b.txt"]));
    assert!(!exists(&repo, "u"));
}

#[test]
fn backup_failure_deletes_nothing() {
    let repo = base();
    repo.write("a.txt", "a\n");
    repo.write("n/b.txt", "b\n");
    // A file where the backup refs folder should be: `update-ref` cannot create the ref.
    std::fs::create_dir_all(repo.path().join(".git/refs/project")).unwrap();
    std::fs::write(repo.path().join(".git/refs/project/backup"), "x").unwrap();
    let files = ["a.txt", "n/b.txt"];
    let before = snapshot(&repo, &files);
    let (r, mut s) = open(&repo);
    let err = clean(&r, &mut s, json!(["a.txt", "n"])).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Git { .. } | EngineError::GitWithReason { .. }
        ),
        "{err}"
    );
    assert_eq!(snapshot(&repo, &files), before);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"], json!([]));
}

#[test]
fn undo_refused_if_a_file_was_recreated() {
    let repo = base();
    repo.write("a.txt", "a\n");
    repo.write("n/b.txt", "b\n");
    let (r, mut s) = open(&repo);
    clean(&r, &mut s, json!(["a.txt", "n"])).unwrap();
    repo.write("n/b.txt", "new content\n");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    match &err {
        EngineError::UndoConflict { refs, .. } => {
            assert_eq!(refs.len(), 1);
            assert!(refs[0].to_string().contains("`n/b.txt`"), "{err}");
        }
        other => panic!("{other:?}"),
    }
    // Nothing restored, nothing overwritten.
    assert!(!exists(&repo, "a.txt"));
    assert_eq!(bytes(&repo, "n/b.txt").unwrap(), b"new content\n");

    // A file in place of the folder: refused too.
    std::fs::remove_dir_all(repo.path().join("n")).unwrap();
    repo.write("n", "a file where the folder was\n");
    assert!(matches!(
        r.run("edit.undo", &mut s, json!({})),
        Err(EngineError::UndoConflict { .. })
    ));
    assert_eq!(bytes(&repo, "n").unwrap(), b"a file where the folder was\n");

    std::fs::remove_file(repo.path().join("n")).unwrap();
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "n/b.txt").unwrap(), b"b\n");
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a\n");
}

#[test]
fn redo_refused_if_a_file_changed_or_was_staged() {
    let repo = base();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let (r, mut s) = open(&repo);
    clean(&r, &mut s, json!(["a.txt", "b.txt"])).unwrap();
    run(&r, &mut s, "edit.undo", json!({}));
    repo.write("a.txt", "a, changed\n");
    assert!(matches!(
        r.run("edit.redo", &mut s, json!({})),
        Err(EngineError::UndoConflict { .. })
    ));
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a, changed\n");
    assert!(exists(&repo, "b.txt"));
    // Same bytes but staged: no longer untracked.
    repo.write("a.txt", "a\n");
    repo.stage(&["a.txt"]);
    assert!(matches!(
        r.run("edit.redo", &mut s, json!({})),
        Err(EngineError::UndoConflict { .. })
    ));
    assert!(exists(&repo, "a.txt") && exists(&repo, "b.txt"));
    repo.git(&["rm", "--cached", "--quiet", "a.txt"]);
    run(&r, &mut s, "edit.redo", json!({}));
    assert!(!exists(&repo, "a.txt") && !exists(&repo, "b.txt"));
}

#[test]
fn plan_changed_since_the_confirmation_is_refused() {
    let repo = base();
    repo.write("a.txt", "a\n");
    let mut backend = repo.open();
    let plan = backend.clean_plan(&[RepoPath::from("a.txt")]).unwrap();
    repo.write("a.txt", "a, longer now\n");
    let err = backend
        .clean(&[RepoPath::from("a.txt")], &plan)
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a, longer now\n");
    // And restoring a clean whose backup lacks a path changes nothing.
    let request = CleanRestore {
        command: "edit.undo",
        redo: false,
        commit: repo.rev("HEAD"),
        files: vec![project_engine::CleanedFile {
            path: RepoPath::from("zzz.txt"),
            mode: 0o100644,
            size: 1,
        }],
    };
    assert!(backend.restore_cleaned(&request).is_err());
    assert!(!exists(&repo, "zzz.txt"));
}

#[test]
fn works_on_a_branch_without_commits() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!(["a.txt"])).unwrap();
    assert_eq!(out["deleted"], json!(["a.txt"]));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a\n");
}

#[test]
fn signing_configuration_and_detached_head_do_not_matter() {
    let repo = base();
    repo.git(&["config", "commit.gpgsign", "true"]);
    repo.git(&["config", "gpg.program", "false"]);
    repo.detach(repo.rev("HEAD"));
    repo.write("a.txt", "a\n");
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!(["a.txt"])).unwrap();
    assert_eq!(out["deleted"], json!(["a.txt"]));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a\n");
}

/// A file that cannot be deleted after the backup (read-only folder) stays, with a warning
/// that names the backup; the others are deleted and only they go into the journal.
#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_deleted_is_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    let repo = base();
    repo.write("a.txt", "a\n");
    repo.write("ro/x.txt", "x\n");
    let ro = repo.path().join("ro");
    let set = |mode| std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(mode)).unwrap();
    set(0o555);
    if std::fs::write(ro.join("probe"), "").is_ok() {
        // root: the folder stays writable, the case cannot be triggered.
        std::fs::remove_file(ro.join("probe")).unwrap();
        set(0o755);
        return;
    }
    let (r, mut s) = open(&repo);
    let out = clean(&r, &mut s, json!(["a.txt", "ro"]));
    set(0o755);
    let out = out.unwrap();
    assert_eq!(out["deleted"], json!(["a.txt"]));
    let warning = out["warnings"][0]["text"].as_str().unwrap();
    assert!(warning.contains("`ro/x.txt`"), "{warning}");
    assert!(
        warning.contains(out["backup"].as_str().unwrap()),
        "{warning}"
    );
    assert_eq!(bytes(&repo, "ro/x.txt").unwrap(), b"x\n");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(
        history["done"][0]["clean"]["files"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Only `ro`: nothing deleted, the error names the backup.
    set(0o555);
    let err = clean(&r, &mut s, json!(["ro"]));
    set(0o755);
    let err = err.unwrap_err();
    assert!(
        err.to_string().contains("refs/project/backup/clean-"),
        "{err}"
    );
    assert_eq!(bytes(&repo, "ro/x.txt").unwrap(), b"x\n");
}

/// A folder of the path replaced by a link to a folder outside the worktree: the undo refuses
/// and writes nothing through the link.
#[cfg(unix)]
#[test]
fn undo_never_writes_through_a_linked_folder() {
    let repo = base();
    repo.write("n/m/b.txt", "b\n");
    let (r, mut s) = open(&repo);
    clean(&r, &mut s, json!(["n"])).unwrap();
    assert!(!exists(&repo, "n"));
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), repo.path().join("n")).unwrap();
    assert!(matches!(
        r.run("edit.undo", &mut s, json!({})),
        Err(EngineError::UndoConflict { .. })
    ));
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    // A link one level down, inside a real folder.
    std::fs::remove_file(repo.path().join("n")).unwrap();
    std::fs::create_dir(repo.path().join("n")).unwrap();
    std::os::unix::fs::symlink(outside.path(), repo.path().join("n/m")).unwrap();
    assert!(matches!(
        r.run("edit.undo", &mut s, json!({})),
        Err(EngineError::UndoConflict { .. })
    ));
    assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    std::fs::remove_file(repo.path().join("n/m")).unwrap();
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "n/m/b.txt").unwrap(), b"b\n");
}

/// An undo that stops midway (a read-only folder) can be retried: the files it already wrote
/// are equal to the backup and count as restored.
#[cfg(unix)]
#[test]
fn an_interrupted_undo_can_be_retried() {
    use std::os::unix::fs::PermissionsExt;
    let repo = base();
    repo.write("a.txt", "a\n");
    repo.write("z/x.txt", "x\n");
    let (r, mut s) = open(&repo);
    clean(&r, &mut s, json!(["a.txt", "z"])).unwrap();
    let z = repo.path().join("z");
    std::fs::create_dir(&z).unwrap();
    let set = |mode| std::fs::set_permissions(&z, std::fs::Permissions::from_mode(mode)).unwrap();
    set(0o555);
    if std::fs::write(z.join("probe"), "").is_ok() {
        // root: the folder stays writable, the case cannot be triggered.
        std::fs::remove_file(z.join("probe")).unwrap();
        set(0o755);
        return;
    }
    let err = r.run("edit.undo", &mut s, json!({}));
    set(0o755);
    assert!(err.is_err());
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a\n");
    assert!(!exists(&repo, "z/x.txt"));
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["warnings"], json!([]));
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a\n");
    assert_eq!(bytes(&repo, "z/x.txt").unwrap(), b"x\n");
}
