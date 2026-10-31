//! Integration tests of `worktree.ignore` with the `CliBackend` on real repos: new file, append
//! without final newline, CRLF, duplicates, special and non-UTF-8 names (patterns from
//! `ignore_pattern` checked with `git check-ignore --no-index`), `.git/info/exclude` (also in a
//! linked worktree), links and folders refused, tracked files matched, undo/redo with the exact
//! bytes (also after `git gc`), file changed before undo/redo, permissions kept, write failure.

mod common;

use common::{GIT_ENV, Repo};
use project_domain::{IgnoreKind, IgnorePattern, RepoPath, ignore_pattern};
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::io::Write as _;
use std::process::{Command, Stdio};
use std::sync::Arc;

fn open_at(path: &str) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": path })).unwrap();
    (r, s)
}

fn open(repo: &Repo) -> (Registry, Session) {
    open_at(repo.path_str())
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn ignore(r: &Registry, s: &mut Session, params: Value) -> Result<Value, EngineError> {
    r.run("worktree.ignore", s, params)
}

fn bytes(repo: &Repo, file: &str) -> Option<Vec<u8>> {
    std::fs::read(repo.path().join(file)).ok()
}

/// A repo with `t.txt` committed.
fn base() -> Repo {
    let repo = Repo::new();
    repo.commit_file("t.txt", "t\n", "base");
    repo
}

/// The untracked, not ignored files, as git lists them.
fn untracked(repo: &Repo) -> Vec<String> {
    let out = repo.git(&["ls-files", "--others", "--exclude-standard"]);
    out.lines().map(str::to_owned).collect()
}

/// The paths among `paths` that `git check-ignore --no-index` says are ignored (by the
/// repo's ignore files), as bytes.
fn check_ignore(repo: &Repo, paths: &[&[u8]]) -> Vec<Vec<u8>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args(["check-ignore", "--no-index", "-z", "--stdin"])
        .envs(GIT_ENV.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = Vec::new();
    for p in paths {
        input.extend_from_slice(p);
        input.push(0);
    }
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let out = child.wait_with_output().unwrap();
    out.stdout
        .split(|&b| b == 0)
        .filter(|r| !r.is_empty())
        .map(<[u8]>::to_vec)
        .collect()
}

#[test]
fn new_gitignore_then_undo_and_redo_after_gc() {
    let repo = base();
    repo.write("a.log", "x");
    repo.write("d/b.log", "y");
    repo.write("keep.txt", "k");
    let (r, mut s) = open(&repo);
    let out = ignore(&r, &mut s, json!({ "patterns": ["*.log"] })).unwrap();
    assert_eq!(out["file"], "gitignore");
    assert_eq!(out["created"], true);
    assert_eq!(out["added"], json!(["*.log"]));
    assert_eq!(out["ignored"], json!(["a.log", "d/b.log"]));
    assert_eq!(out["tracked"], json!([]));
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["can_undo"], true);
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"*.log\n");
    assert_eq!(untracked(&repo), [".gitignore", "keep.txt"]);
    // The backup: the child commit has no file (it did not exist), the parent the bytes written.
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/ignore-"),
        "{backup}"
    );
    assert_eq!(repo.git(&["ls-tree", "-r", "--name-only", &backup]), "");
    assert_eq!(
        repo.git(&["show", &format!("{backup}^:.gitignore")]),
        "*.log"
    );
    // No temporary file left.
    let names: Vec<String> = std::fs::read_dir(repo.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.ends_with(".tmp")), "{names:?}");

    repo.git(&["gc", "--quiet", "--prune=now"]);
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "worktree.ignore");
    assert!(bytes(&repo, ".gitignore").is_none());
    assert_eq!(untracked(&repo), ["a.log", "d/b.log", "keep.txt"]);
    repo.git(&["gc", "--quiet", "--prune=now"]);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"*.log\n");
}

#[test]
fn append_without_final_newline_keeps_old_bytes_for_undo() {
    let repo = base();
    std::fs::write(repo.path().join(".gitignore"), b"/target").unwrap();
    repo.stage(&[".gitignore"]);
    repo.commit("ignore");
    repo.write("x.tmp", "x");
    let (r, mut s) = open(&repo);
    let out = ignore(&r, &mut s, json!({ "patterns": ["*.tmp"] })).unwrap();
    assert_eq!(out["created"], false);
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"/target\n*.tmp\n");
    assert_eq!(out["ignored"], json!(["x.tmp"]));
    // The change appears as an unstaged modification.
    let status = out["status"]["entries"].as_array().unwrap();
    assert!(status.iter().any(|e| e["path"] == ".gitignore"));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"/target");
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"/target\n*.tmp\n");
}

#[test]
fn crlf_file_keeps_crlf() {
    let repo = base();
    std::fs::write(repo.path().join(".gitignore"), b"a\r\nb").unwrap();
    let (r, mut s) = open(&repo);
    ignore(&r, &mut s, json!({ "patterns": ["b", "c", "d"] })).unwrap();
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"a\r\nb\r\nc\r\nd\r\n");
    std::fs::write(repo.path().join(".gitignore"), b"a\r\n").unwrap();
    let out = ignore(&r, &mut s, json!({ "patterns": ["a", "e"] })).unwrap();
    assert_eq!(out["already"], json!(["a"]));
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"a\r\ne\r\n");
}

#[test]
fn duplicates_are_not_added_and_nothing_is_written() {
    let repo = base();
    std::fs::write(repo.path().join(".gitignore"), b"*.log\n").unwrap();
    let (r, mut s) = open(&repo);
    let out = ignore(&r, &mut s, json!({ "patterns": ["*.log", "x", "x"] })).unwrap();
    assert_eq!(out["added"], json!(["x"]));
    assert_eq!(out["already"], json!(["*.log"]));
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"*.log\nx\n");
    let refs_before = repo.git(&["for-each-ref", "refs/project"]);
    let out = ignore(&r, &mut s, json!({ "patterns": ["x", "*.log"] })).unwrap();
    assert_eq!(out["added"], json!([]));
    assert_eq!(out["backup"], json!(null));
    assert_eq!(out["created"], false);
    assert_eq!(repo.git(&["for-each-ref", "refs/project"]), refs_before);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"].as_array().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn special_and_non_utf8_names() {
    use std::os::unix::ffi::OsStrExt;
    let repo = base();
    let names: [&[u8]; 8] = [
        b"a b.txt",
        b"#hash",
        b"!bang",
        b"st*r",
        b"q?",
        b"trail ",
        b"caf\xe9.txt",
        b"-rf",
    ];
    // Lookalikes the patterns must not match.
    let others: [&[u8]; 6] = [b"a  b.txt", b"stXr", b"qX", b"trail", b"caf.txt", b"hash"];
    for name in names.iter().chain(&others) {
        std::fs::write(repo.path().join(std::ffi::OsStr::from_bytes(name)), b"x").unwrap();
    }
    let patterns: Vec<IgnorePattern> = names
        .iter()
        .map(|n| ignore_pattern(&RepoPath::from_bytes(*n), IgnoreKind::Path).unwrap())
        .collect();
    let (r, mut s) = open(&repo);
    let out = ignore(&r, &mut s, json!({ "patterns": patterns })).unwrap();
    let ignored: Vec<RepoPath> = serde_json::from_value(out["ignored"].clone()).unwrap();
    let mut expected: Vec<RepoPath> = names.iter().map(|n| RepoPath::from_bytes(*n)).collect();
    expected.sort();
    assert_eq!(ignored, expected);
    let all: Vec<&[u8]> = names.iter().chain(&others).copied().collect();
    let mut matched = check_ignore(&repo, &all);
    matched.sort();
    let mut wanted: Vec<Vec<u8>> = names.iter().map(|n| n.to_vec()).collect();
    wanted.sort();
    assert_eq!(matched, wanted);
}

#[test]
fn folder_and_extension_patterns_match_what_they_say() {
    let repo = base();
    for f in [
        "build/a.o",
        "build/sub/b.o",
        "src/build/c.txt",
        "x.log",
        "d/y.log",
        "z.logs",
    ] {
        repo.write(f, "x");
    }
    let folder = ignore_pattern(&RepoPath::from("build"), IgnoreKind::Folder).unwrap();
    let ext = ignore_pattern(&RepoPath::from("d/y.log"), IgnoreKind::Extension).unwrap();
    let (r, mut s) = open(&repo);
    let out = ignore(&r, &mut s, json!({ "patterns": [folder, ext] })).unwrap();
    assert_eq!(
        out["ignored"],
        json!(["build/a.o", "build/sub/b.o", "d/y.log", "x.log"])
    );
    assert_eq!(
        untracked(&repo),
        [".gitignore", "src/build/c.txt", "z.logs"]
    );
}

#[test]
fn exclude_file_is_created_with_its_folder() {
    let repo = base();
    repo.write("secret.env", "x");
    std::fs::remove_dir_all(repo.path().join(".git/info")).ok();
    let (r, mut s) = open(&repo);
    let out = ignore(
        &r,
        &mut s,
        json!({ "patterns": ["/secret.env"], "file": "exclude" }),
    )
    .unwrap();
    assert_eq!(out["label"], ".git/info/exclude");
    assert_eq!(out["created"], true);
    assert_eq!(out["ignored"], json!(["secret.env"]));
    assert_eq!(bytes(&repo, ".git/info/exclude").unwrap(), b"/secret.env\n");
    assert!(bytes(&repo, ".gitignore").is_none());
    assert!(untracked(&repo).is_empty());
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(
        repo.git(&["show", &format!("{backup}^:info/exclude")]),
        "/secret.env"
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(bytes(&repo, ".git/info/exclude").is_none());
    assert_eq!(untracked(&repo), ["secret.env"]);
}

#[test]
fn exclude_of_a_linked_worktree_is_the_common_one() {
    let repo = base();
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("wt");
    repo.git(&["worktree", "add", "--quiet", wt_path.to_str().unwrap()]);
    std::fs::write(wt_path.join("w.tmp"), b"x").unwrap();
    let (r, mut s) = open_at(wt_path.to_str().unwrap());
    let out = ignore(
        &r,
        &mut s,
        json!({ "patterns": ["*.tmp"], "file": "exclude" }),
    )
    .unwrap();
    assert_eq!(out["ignored"], json!(["w.tmp"]));
    let common = bytes(&repo, ".git/info/exclude").unwrap();
    assert!(common.ends_with(b"*.tmp\n"), "{common:?}");
}

#[cfg(unix)]
#[test]
fn links_and_folders_are_refused() {
    let repo = base();
    std::fs::write(repo.path().join("target.txt"), b"keep\n").unwrap();
    std::os::unix::fs::symlink("target.txt", repo.path().join(".gitignore")).unwrap();
    let (r, mut s) = open(&repo);
    let err = ignore(&r, &mut s, json!({ "patterns": ["x"] })).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(bytes(&repo, "target.txt").unwrap(), b"keep\n");

    std::fs::remove_file(repo.path().join(".gitignore")).unwrap();
    std::fs::create_dir(repo.path().join(".gitignore")).unwrap();
    let err = ignore(&r, &mut s, json!({ "patterns": ["x"] })).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    // `.git/info` a link to another folder: refused, nothing written there.
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::remove_dir_all(repo.path().join(".git/info")).ok();
    std::os::unix::fs::symlink(elsewhere.path(), repo.path().join(".git/info")).unwrap();
    let err = ignore(&r, &mut s, json!({ "patterns": ["x"], "file": "exclude" })).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(
        std::fs::read_dir(elsewhere.path())
            .unwrap()
            .next()
            .is_none()
    );
    // `.git/info/exclude` a link: refused.
    std::fs::remove_file(repo.path().join(".git/info")).unwrap();
    std::fs::create_dir(repo.path().join(".git/info")).unwrap();
    std::os::unix::fs::symlink("../../target.txt", repo.path().join(".git/info/exclude")).unwrap();
    let err = ignore(&r, &mut s, json!({ "patterns": ["x"], "file": "exclude" })).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(bytes(&repo, "target.txt").unwrap(), b"keep\n");
    assert!(repo.git(&["for-each-ref", "refs/project"]).is_empty());
}

#[test]
fn tracked_files_matched_give_a_warning() {
    let repo = base();
    repo.commit_file("t.log", "l\n", "log");
    repo.write("u.log", "u");
    let (r, mut s) = open(&repo);
    let out = ignore(&r, &mut s, json!({ "patterns": ["*.log"] })).unwrap();
    assert_eq!(out["tracked"], json!(["t.log"]));
    assert_eq!(out["ignored"], json!(["u.log"]));
    assert_eq!(out["warnings"][0]["key"], "engine-ignore-tracked");
    // Still tracked.
    assert_eq!(repo.git(&["ls-files", "t.log"]), "t.log");
}

#[test]
fn file_changed_before_undo_or_redo_is_a_conflict() {
    let repo = base();
    let (r, mut s) = open(&repo);
    ignore(&r, &mut s, json!({ "patterns": ["*.log"] })).unwrap();
    std::fs::write(repo.path().join(".gitignore"), b"*.log\nmine\n").unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"*.log\nmine\n");
    std::fs::write(repo.path().join(".gitignore"), b"*.log\n").unwrap();
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(bytes(&repo, ".gitignore").is_none());
    // Created again by someone else: the redo does not overwrite it.
    std::fs::write(repo.path().join(".gitignore"), b"other\n").unwrap();
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"other\n");
}

#[test]
fn conflicted_gitignore_is_refused() {
    let repo = base();
    repo.diverge("other", ".gitignore", "ours\n", "theirs\n");
    repo.conflict("other");
    let before = bytes(&repo, ".gitignore");
    let (r, mut s) = open(&repo);
    let err = ignore(&r, &mut s, json!({ "patterns": ["x"] })).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(bytes(&repo, ".gitignore"), before);
}

/// A `.gitignore` tracked but marked `--skip-worktree` and missing from disk: git reads its rules
/// from the index, so a new file with only the new patterns is refused.
#[test]
fn tracked_gitignore_missing_from_disk_is_refused() {
    let repo = base();
    repo.commit_file(".gitignore", "*.log\n", "ignore");
    repo.git(&["update-index", "--skip-worktree", ".gitignore"]);
    std::fs::remove_file(repo.path().join(".gitignore")).unwrap();
    let (r, mut s) = open(&repo);
    let err = ignore(&r, &mut s, json!({ "patterns": ["x"] })).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(bytes(&repo, ".gitignore"), None);
}

#[cfg(unix)]
#[test]
fn permissions_are_kept() {
    use std::os::unix::fs::PermissionsExt;
    let repo = base();
    let path = repo.path().join(".gitignore");
    std::fs::write(&path, b"a\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    let (r, mut s) = open(&repo);
    ignore(&r, &mut s, json!({ "patterns": ["b"] })).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o640);
}

#[cfg(unix)]
#[test]
fn write_failure_leaves_the_file_and_names_the_backup() {
    use std::os::unix::fs::PermissionsExt;
    let repo = base();
    std::fs::write(repo.path().join(".gitignore"), b"a\n").unwrap();
    let (r, mut s) = open(&repo);
    let root = repo.path();
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o555)).unwrap();
    // As root the folder stays writable: nothing to test.
    let writable = std::fs::write(root.join("probe"), b"").is_ok();
    let result = ignore(&r, &mut s, json!({ "patterns": ["b"] }));
    std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        return;
    }
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("refs/project/backup/ignore-"),
        "{err}"
    );
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"a\n");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert!(history["done"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn backup_failure_writes_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let repo = base();
    std::fs::write(repo.path().join(".gitignore"), b"a\n").unwrap();
    let (r, mut s) = open(&repo);
    let refs = repo.path().join(".git/refs");
    std::fs::set_permissions(&refs, std::fs::Permissions::from_mode(0o555)).unwrap();
    let writable = std::fs::create_dir(refs.join("probe")).is_ok();
    let result = ignore(&r, &mut s, json!({ "patterns": ["b"] }));
    std::fs::set_permissions(&refs, std::fs::Permissions::from_mode(0o755)).unwrap();
    if writable {
        return;
    }
    assert!(result.is_err());
    assert_eq!(bytes(&repo, ".gitignore").unwrap(), b"a\n");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert!(history["done"].as_array().unwrap().is_empty());
}
