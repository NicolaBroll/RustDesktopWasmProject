//! Integration tests of `conflict.show` and `conflict.resolve` (S30) with the `CliBackend`:
//! conflicting merge -> versions -> resolution -> `repo.operation` ready -> `merge.continue`;
//! stale digest, file not in conflict, locked index, link in the worktree (nothing written);
//! `delete` for `DeletedByThem`, missing file for `DeletedByUs`; backup readable with `git show`;
//! diff3 style and `conflict-marker-size`; non-UTF-8 files; names with spaces and a leading
//! `-`; executable mode and permissions; failing backup; write that fails after the backup.

mod common;

use common::{Repo, repo_with_merge_conflict};
use project_engine::{EngineError, Registry, Session, worktree_digest};
use serde_json::{Value, json};
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

fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/conflict-*",
    ]);
    out.lines().map(str::to_owned).collect()
}

fn unmerged(repo: &Repo) -> String {
    repo.git(&["ls-files", "-u"])
}

fn show(r: &Registry, s: &mut Session, path: &str) -> Value {
    run(r, s, "conflict.show", json!({ "path": path }))
}

fn resolve(r: &Registry, s: &mut Session, path: &str, digest: &Value, content: &str) -> Value {
    run(
        r,
        s,
        "conflict.resolve",
        json!({ "path": path, "digest": digest, "content": content }),
    )
}

#[test]
fn show_resolve_and_continue() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    assert_eq!(shown["kind"], "BothModified");
    assert_eq!(shown["base"]["text"], "base\n");
    assert_eq!(shown["ours"]["text"], "ours\n");
    assert_eq!(shown["theirs"]["text"], "theirs\n");
    let worktree = std::fs::read(repo.path().join("a.txt")).unwrap();
    assert_eq!(
        shown["worktree"]["text"],
        String::from_utf8(worktree.clone()).unwrap()
    );
    assert!(
        shown["worktree"]["text"]
            .as_str()
            .unwrap()
            .contains("<<<<<<< HEAD\n")
    );
    assert_eq!(shown["marker_size"], 7);
    assert_eq!(shown["digest"], worktree_digest(Some(&worktree)));

    let out = resolve(&r, &mut s, "a.txt", &shown["digest"], "merged\n");
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/conflict-"),
        "{backup}"
    );
    assert_eq!(out["index_backup"], Value::Null);
    assert_eq!(backups(&repo), std::slice::from_ref(&backup));
    // The backup: the file with the markers, without parents.
    assert_eq!(
        repo.git(&["show", &format!("{backup}:a.txt")]).into_bytes(),
        worktree.trim_ascii_end()
    );
    assert_eq!(repo.git(&["rev-list", "--count", &backup]), "1");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "merged\n"
    );
    assert_eq!(unmerged(&repo), "");
    assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt");

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);
    run(&r, &mut s, "merge.continue", json!({}));
    assert_eq!(repo.git(&["show", "HEAD:a.txt"]), "merged");
    assert_eq!(repo.git(&["rev-list", "--count", "--merges", "HEAD"]), "1");
}

#[test]
fn stale_digest_writes_nothing() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    repo.write("a.txt", "changed from outside\n");
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "a.txt", "digest": shown["digest"], "content": "x\n" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "changed from outside\n"
    );
    assert!(backups(&repo).is_empty());
    assert_ne!(unmerged(&repo), "");
}

#[test]
fn not_in_conflict_is_refused() {
    let repo = repo_with_merge_conflict();
    repo.write("new.txt", "n\n");
    let (r, mut s) = open(&repo);
    let digest = worktree_digest(Some(b"n\n"));
    for path in ["new.txt", "missing.txt"] {
        let err = r
            .run("conflict.show", &mut s, json!({ "path": path }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = r
            .run(
                "conflict.resolve",
                &mut s,
                json!({ "path": path, "digest": digest, "content": "x" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }
    assert_eq!(
        std::fs::read_to_string(repo.path().join("new.txt")).unwrap(),
        "n\n"
    );
    assert!(backups(&repo).is_empty());
}

#[test]
fn index_locked_writes_nothing() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "a.txt", "digest": shown["digest"], "content": "x\n" }),
        )
        .unwrap_err();
    std::fs::remove_file(&lock).unwrap();
    assert!(matches!(err, EngineError::IndexLocked { .. }), "{err}");
    assert!(backups(&repo).is_empty());
    assert_eq!(
        shown["worktree"]["text"],
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap()
    );
}

#[cfg(unix)]
#[test]
fn link_in_worktree_is_refused() {
    let repo = repo_with_merge_conflict();
    let file = repo.path().join("a.txt");
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink("b.txt", &file).unwrap();
    let (r, mut s) = open(&repo);
    let err = r
        .run("conflict.show", &mut s, json!({ "path": "a.txt" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "a.txt", "digest": worktree_digest(None), "content": "x" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(file.symlink_metadata().unwrap().is_symlink());
    assert!(backups(&repo).is_empty());
}

#[test]
fn delete_for_deleted_by_them() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("removed");
    repo.checkout("removed");
    repo.git(&["rm", "--quiet", "a.txt"]);
    repo.commit("remove a.txt");
    repo.checkout("main");
    repo.commit_file("a.txt", "ours\n", "ours");
    repo.conflict("removed");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    assert_eq!(shown["kind"], "DeletedByThem");
    assert_eq!(shown["theirs"], Value::Null);
    assert_eq!(shown["ours"]["text"], "ours\n");
    let out = run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "delete": true }),
    );
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.git(&["show", &format!("{backup}:a.txt")]), "ours");
    assert!(!repo.path().join("a.txt").exists());
    assert_eq!(repo.git(&["status", "--porcelain"]), "D  a.txt");
    assert_eq!(run(&r, &mut s, "repo.operation", json!({}))["ready"], true);
    run(&r, &mut s, "merge.continue", json!({}));
    repo.git_fails(&["cat-file", "-e", "HEAD:a.txt"]);
}

#[test]
fn deleted_by_us_with_missing_file() {
    let repo = Repo::new();
    repo.commit_file("dir/a.txt", "base\n", "base");
    repo.branch("other");
    repo.checkout("other");
    repo.commit_file("dir/a.txt", "theirs\n", "theirs");
    repo.checkout("main");
    repo.git(&["rm", "--quiet", "dir/a.txt"]);
    repo.commit("remove");
    repo.conflict("other");
    std::fs::remove_dir_all(repo.path().join("dir")).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "dir/a.txt");
    assert_eq!(shown["kind"], "DeletedByUs");
    assert_eq!(shown["ours"], Value::Null);
    assert_eq!(shown["worktree"], Value::Null);
    assert_eq!(shown["digest"], worktree_digest(None));
    let out = resolve(&r, &mut s, "dir/a.txt", &shown["digest"], "kept\n");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("dir/a.txt")).unwrap(),
        "kept\n"
    );
    assert_eq!(repo.git(&["status", "--porcelain"]), "A  dir/a.txt");
}

#[test]
fn diff3_style_and_marker_size() {
    let repo = Repo::new();
    repo.commit_file(".gitattributes", "*.txt conflict-marker-size=10\n", "attr");
    repo.commit_file("a.txt", "base\n", "base");
    repo.diverge("other", "a.txt", "ours\n", "theirs\n");
    repo.git_fails(&[
        "-c",
        "merge.conflictStyle=diff3",
        "merge",
        "--no-edit",
        "other",
    ]);
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    assert_eq!(shown["marker_size"], 10);
    let text = shown["worktree"]["text"].as_str().unwrap();
    assert!(text.contains("||||||||||"), "{text}");
    assert!(text.contains("\nbase\n"), "{text}");
    assert_eq!(shown["base"]["text"], "base\n");
}

#[test]
fn non_utf8_is_binary() {
    let repo = Repo::new();
    let write = |bytes: &[u8]| std::fs::write(repo.path().join("l.txt"), bytes).unwrap();
    write(b"caf\xe9\n");
    repo.stage(&["l.txt"]);
    repo.commit("base");
    repo.branch("other");
    repo.checkout("other");
    write(b"caf\xe8 theirs\n");
    repo.stage(&["l.txt"]);
    repo.commit("theirs");
    repo.checkout("main");
    write(b"caf\xe0 ours\n");
    repo.stage(&["l.txt"]);
    repo.commit("ours");
    repo.conflict("other");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "l.txt");
    for side in ["base", "ours", "theirs", "worktree"] {
        assert_eq!(shown[side]["text"], Value::Null, "{side}");
        assert_eq!(shown[side]["binary"], true, "{side}");
    }
    assert_eq!(shown["base"]["size"], 5);
    let before = std::fs::read(repo.path().join("l.txt")).unwrap();
    let out = resolve(&r, &mut s, "l.txt", &shown["digest"], "caf\u{e9}\n");
    let backup = out["backup"].as_str().unwrap();
    let saved = repo
        .git_output(&["show", &format!("{backup}:l.txt")])
        .stdout;
    assert_eq!(saved, before);
    assert_eq!(
        std::fs::read(repo.path().join("l.txt")).unwrap(),
        "caf\u{e9}\n".as_bytes()
    );
}

#[test]
fn special_names() {
    let repo = Repo::new();
    let name = "-a b.txt";
    repo.commit_file(name, "base\n", "base");
    repo.diverge("other", name, "ours\n", "theirs\n");
    repo.conflict("other");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, name);
    assert_eq!(shown["ours"]["text"], "ours\n");
    resolve(&r, &mut s, name, &shown["digest"], "merged\n");
    assert_eq!(unmerged(&repo), "");
    assert_eq!(
        std::fs::read_to_string(repo.path().join(name)).unwrap(),
        "merged\n"
    );
    let entries = repo.git(&["ls-files", "-s"]);
    assert_eq!(entries.lines().count(), 1, "{entries}");
}

#[cfg(unix)]
#[test]
fn executable_bit_is_kept() {
    use std::os::unix::fs::PermissionsExt;
    let repo = repo_with_merge_conflict();
    let file = repo.path().join("a.txt");
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&file, perms).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    resolve(&r, &mut s, "a.txt", &shown["digest"], "merged\n");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "{mode:o}");
}

/// The read-only folder: the backup is made, the write fails, the error names it and the file
/// stays as it was.
#[cfg(unix)]
#[test]
fn write_failure_after_backup_names_it() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("d/a.txt", "base\n", "base");
    repo.diverge("other", "d/a.txt", "ours\n", "theirs\n");
    repo.conflict("other");
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "d/a.txt");
    let dir = repo.path().join("d");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(dir.join("probe"), "").is_ok() {
        // Root: the folder stays writable.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let err = r
        .run(
            "conflict.resolve",
            &mut s,
            json!({ "path": "d/a.txt", "digest": shown["digest"], "content": "x\n" }),
        )
        .unwrap_err();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let backup = backups(&repo);
    assert_eq!(backup.len(), 1);
    assert!(err.to_string().contains(&backup[0]), "{err}");
    assert_eq!(
        shown["worktree"]["text"],
        std::fs::read_to_string(dir.join("a.txt")).unwrap()
    );
    assert_ne!(unmerged(&repo), "");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
}

/// The permissions of the replaced file stay the same (0600 does not become readable by others).
#[cfg(unix)]
#[test]
fn permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt;
    let repo = repo_with_merge_conflict();
    let file = repo.path().join("a.txt");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    resolve(&r, &mut s, "a.txt", &shown["digest"], "merged\n");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

/// `DeletedByUs` with the file missing and their side executable: the new file is executable.
#[cfg(unix)]
#[test]
fn missing_file_takes_mode_of_remaining_stage() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("a.sh", "base\n", "base");
    repo.branch("other");
    repo.checkout("other");
    repo.write("a.sh", "theirs\n");
    let file = repo.path().join("a.sh");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["add", "a.sh"]);
    repo.commit("theirs");
    repo.checkout("main");
    repo.git(&["rm", "--quiet", "a.sh"]);
    repo.commit("remove");
    repo.conflict("other");
    std::fs::remove_file(&file).unwrap();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.sh");
    resolve(&r, &mut s, "a.sh", &shown["digest"], "kept\n");
    assert_eq!(
        repo.git(&["ls-files", "-s", "a.sh"]).split(' ').next(),
        Some("100755")
    );
}

/// The backup ref cannot be created: nothing is written, the conflict stays, no temporary file.
#[test]
fn backup_failure_writes_nothing() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let shown = show(&r, &mut s, "a.txt");
    let refs = repo.path().join(".git/refs/project");
    std::fs::create_dir_all(&refs).unwrap();
    std::fs::write(refs.join("backup"), "").unwrap();
    r.run(
        "conflict.resolve",
        &mut s,
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "x\n" }),
    )
    .unwrap_err();
    assert_eq!(
        shown["worktree"]["text"],
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap()
    );
    assert_ne!(unmerged(&repo), "");
    let temps = std::fs::read_dir(repo.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".project-conflict-")
        })
        .count();
    assert_eq!(temps, 0);
}
