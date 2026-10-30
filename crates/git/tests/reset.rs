//! Integration tests of `branch.reset` (S45) with the `CliBackend` on real repos: the three
//! modes, `hard` with modified, staged, CRLF and conflicted files (confirmation, backup with
//! `git stash create` and byte for byte, untracked files untouched), detached HEAD, a target
//! that is not an ancestor, undo/redo, refs changed between the confirmation and the write,
//! refusals (operation in progress, branch without commits, bad target, untracked file in the
//! way).

mod common;

use common::{Repo, repo_with_merge_conflict};
use project_domain::RepoPath;
use project_engine::{
    EngineError, GitRead, GitRefs, HeadState, RefMove, RefRestore, Registry, ResetMode,
    ResetRequest, Session, reset_files,
};
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

/// Status, index, refs and the bytes of `files`: what a refused reset must not touch.
fn snapshot(repo: &Repo, files: &[&str]) -> (String, String, String, Vec<Option<Vec<u8>>>) {
    (
        repo.git(&["status", "--porcelain=v2", "--untracked-files=all"]),
        repo.git(&["ls-files", "-s"]),
        repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        files
            .iter()
            .map(|f| std::fs::read(repo.path().join(f)).ok())
            .collect(),
    )
}

fn bytes(repo: &Repo, file: &str) -> Option<Vec<u8>> {
    std::fs::read(repo.path().join(file)).ok()
}

/// Three commits on `main`: `c1` (`a.txt` = 1, `u.txt`), `c2` (`a.txt` = 2, `u.txt` deleted),
/// `c3` (`a.txt` = 3, `b.txt`).
fn three_commits() -> (Repo, [project_domain::Oid; 3]) {
    let repo = Repo::new();
    repo.write("u.txt", "u\n");
    repo.stage(&["u.txt"]);
    let c1 = repo.commit_file("a.txt", "1\n", "c1");
    repo.git(&["rm", "--quiet", "u.txt"]);
    let c2 = repo.commit_file("a.txt", "2\n", "c2");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let c3 = repo.commit_file("a.txt", "3\n", "c3");
    (repo, [c1, c2, c3])
}

#[test]
fn soft_moves_only_the_branch_and_undo_redo_follow() {
    let (repo, [c1, _, c3]) = three_commits();
    repo.write("a.txt", "local\n");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c1.to_string(), "mode": "soft" }),
    );
    assert_eq!(out["before"], c3.to_string());
    assert_eq!(out["after"], c1.to_string());
    assert_eq!(repo.rev("main"), c1);
    // The index still has c3's tree, the worktree its change.
    assert_eq!(
        repo.git(&["write-tree"]),
        repo.git(&["rev-parse", &format!("{c3}^{{tree}}")])
    );
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"local\n");
    // The reflog says what happened.
    assert!(
        repo.git(&["reflog", "-1", "main"])
            .contains("project: reset (soft)")
    );

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c3);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), c1);
}

#[test]
fn mixed_rewrites_the_index_and_saves_the_staged_versions() {
    let (repo, [_, c2, c3]) = three_commits();
    repo.write("a.txt", "staged\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "worktree\n");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c2.to_string(), "mode": "mixed" }),
    );
    let backup = out["index_backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/index-"), "{out}");
    assert_eq!(repo.git(&["show", &format!("{backup}:a.txt")]), "staged");
    assert_eq!(repo.rev("main"), c2);
    // Index = c2's tree, worktree untouched (b.txt now untracked).
    repo.git(&["diff", "--cached", "--quiet"]);
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"worktree\n");
    assert!(repo.path().join("b.txt").exists());

    // Undo: ref and index back to c3; the worktree stays.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c3);
    repo.git(&["diff", "--cached", "--quiet"]);
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"worktree\n");
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), c2);
}

/// `main` at `c2` with: `m.txt` modified, `s.txt` new and staged, `crlf.txt` with CRLF
/// (`core.autocrlf=input`), `f.txt` conflicted (by `git stash apply`: no operation in
/// progress), `note.txt` untracked and `junk.log` ignored.
fn messy() -> (Repo, project_domain::Oid, project_domain::Oid) {
    let repo = Repo::new();
    repo.write(".gitignore", "*.log\n");
    repo.write("m.txt", "m\n");
    repo.write("crlf.txt", "x\n");
    repo.stage(&[".gitignore", "m.txt", "crlf.txt"]);
    let c1 = repo.commit_file("f.txt", "base\n", "c1");
    repo.write("f.txt", "stashed\n");
    repo.stash();
    let c2 = repo.commit_file("f.txt", "ours\n", "c2");
    repo.git_fails(&["stash", "apply"]);
    repo.git(&["config", "core.autocrlf", "input"]);
    std::fs::write(repo.path().join("crlf.txt"), b"y\r\nz\r\n").unwrap();
    repo.write("m.txt", "changed\n");
    repo.write("s.txt", "staged\n");
    repo.stage(&["s.txt"]);
    repo.write("note.txt", "keep me\n");
    repo.write("junk.log", "ignored\n");
    (repo, c1, c2)
}

#[test]
fn hard_with_every_kind_of_change_needs_confirm_and_saves_everything() {
    let (repo, c1, c2) = messy();
    assert!(repo.git(&["status", "--porcelain"]).contains("UU f.txt"));
    let files = [
        "m.txt", "s.txt", "crlf.txt", "f.txt", "note.txt", "junk.log",
    ];
    let before = snapshot(&repo, &files);
    let crlf = bytes(&repo, "crlf.txt").unwrap();
    let conflicted = bytes(&repo, "f.txt").unwrap();
    let (r, mut s) = open(&repo);

    let preview = run(
        &r,
        &mut s,
        "branch.reset_preview",
        json!({ "target": c1.to_string(), "mode": "hard" }),
    );
    assert_eq!(preview["needs_confirm"], true);
    assert_eq!(
        preview["files"],
        json!(["crlf.txt", "f.txt", "m.txt", "s.txt"])
    );
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c1.to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    let EngineError::ConfirmRequired { expected, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(expected.as_deref(), Some(c2.to_string().as_str()));
    assert_eq!(snapshot(&repo, &files), before, "nothing changed");

    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c1.to_string(), "mode": "hard", "confirm": true, "expected": preview["expected"] }),
    );
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/reset-"), "{out}");
    let stash = out["stash"].as_str().unwrap().to_owned();
    assert_eq!(repo.rev("main"), c1);
    // Clean against c1, untracked and ignored files untouched.
    assert_eq!(
        repo.git(&["status", "--porcelain", "--ignored"]),
        "?? note.txt\n!! junk.log"
    );
    assert_eq!(bytes(&repo, "note.txt").unwrap(), b"keep me\n");
    assert_eq!(bytes(&repo, "junk.log").unwrap(), b"ignored\n");
    assert_eq!(bytes(&repo, "f.txt").unwrap(), b"base\n");

    // Everything is in the backup: the stash commit (worktree, index as second parent) and the
    // CRLF bytes in the raw child commit the ref points to.
    assert_eq!(repo.git(&["show", &format!("{stash}:m.txt")]), "changed");
    assert_eq!(repo.git(&["show", &format!("{stash}^2:s.txt")]), "staged");
    assert_eq!(
        repo.git(&["show", &format!("{stash}:f.txt")]).into_bytes(),
        conflicted.trim_ascii_end()
    );
    let raw = repo.git_output(&["cat-file", "blob", &format!("{backup}:crlf.txt")]);
    assert_eq!(raw.stdout, crlf);
    // The ref survives gc.
    repo.git(&["gc", "--quiet", "--prune=now"]);
    assert_eq!(repo.git(&["show", &format!("{stash}:m.txt")]), "changed");

    // Undo: back to c2's files (not the local changes: the warning says where they are).
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c2);
    assert_eq!(bytes(&repo, "f.txt").unwrap(), b"ours\n");
    assert!(
        undo["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["key"] == "engine-reset-undo-backup"),
        "{undo}"
    );
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), c1);
    assert_eq!(bytes(&repo, "note.txt").unwrap(), b"keep me\n");
}

#[test]
fn detached_head_moves_head_only() {
    let (repo, [c1, c2, c3]) = three_commits();
    repo.detach(c2);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c1.to_string(), "mode": "hard" }),
    );
    assert_eq!(out["branch"], Value::Null);
    assert_eq!(repo.rev("HEAD"), c1);
    assert_eq!(repo.rev("main"), c3, "the branch does not move");
    assert!(repo.path().join("u.txt").exists());
    repo.git_fails(&["symbolic-ref", "-q", "HEAD"]);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("HEAD"), c2);
    assert!(!repo.path().join("u.txt").exists());
}

#[test]
fn target_not_an_ancestor_saves_the_abandoned_commits() {
    let (repo, [c1, _, c3]) = three_commits();
    repo.git(&["branch", "other", &c1.to_string()]);
    repo.checkout("other");
    let x = repo.commit_file("x.txt", "x\n", "x");
    repo.checkout("main");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": "other", "mode": "hard" }),
    );
    assert_eq!(repo.rev("main"), x);
    let backups = out["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 1, "{out}");
    let saved = backups[0].as_str().unwrap();
    assert!(saved.starts_with("refs/project/backup/reset-"));
    assert_eq!(repo.rev(saved), c3);
    assert_eq!(bytes(&repo, "x.txt").unwrap(), b"x\n");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c3);
    assert!(!repo.path().join("x.txt").exists());
}

#[test]
fn commits_already_on_the_upstream_give_a_warning() {
    let (repo, [c1, _, _]) = three_commits();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.push("origin", "main", "main");
    repo.set_upstream("main", "origin");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c1.to_string(), "mode": "soft" }),
    );
    assert_eq!(out["published"], 2);
    assert_eq!(out["warnings"][0]["key"], "engine-reset-published", "{out}");
}

#[test]
fn refs_changed_between_the_confirmation_and_the_write() {
    let (repo, [c1, c2, c3]) = three_commits();
    repo.write("a.txt", "local\n");
    let (r, mut s) = open(&repo);
    let preview = run(
        &r,
        &mut s,
        "branch.reset_preview",
        json!({ "target": c1.to_string(), "mode": "hard" }),
    );
    // A commit from a terminal after the dialog was shown.
    let c4 = repo.commit_file("b.txt", "terminal\n", "c4");
    repo.write("a.txt", "local\n");
    let before = snapshot(&repo, &["a.txt"]);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c1.to_string(), "mode": "hard", "confirm": true, "expected": preview["expected"] }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-head-moved"),
        "{err}"
    );
    assert_eq!(snapshot(&repo, &["a.txt"]), before);
    assert_eq!(repo.rev("main"), c4);

    // The backend with the values read earlier (the branch moved between the command's read and
    // the write): refused, nothing written, no backup.
    let mut backend = repo.open();
    let files = reset_files(&backend.status().unwrap()).unwrap();
    let main = "refs/heads/main".to_owned();
    let request = ResetRequest {
        mode: ResetMode::Hard,
        refs: RefRestore {
            command: "branch.reset",
            head_expected: HeadState::Branch(main.clone()),
            head_target: HeadState::Branch(main.clone()),
            refs: vec![RefMove {
                name: main,
                expected: Some(c3),
                target: Some(c2),
            }],
            reflog: "project: reset".to_owned(),
        },
        files,
        strict: false,
    };
    let err = backend.reset(&request).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-moved"),
        "{err}"
    );
    assert_eq!(snapshot(&repo, &["a.txt"]), before);
    assert_eq!(
        repo.git(&["for-each-ref", "refs/project/backup/"]),
        "",
        "no backup"
    );
    // The confirmed plan is not the current one.
    let mut stale = request.clone();
    stale.refs.refs[0].expected = Some(c4);
    stale.files = vec![RepoPath::from("b.txt")];
    let err = backend.reset(&stale).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-files-changed"),
        "{err}"
    );
    assert_eq!(snapshot(&repo, &["a.txt"]), before);
}

#[test]
fn undo_refused_when_the_worktree_changed_after_a_hard() {
    let (repo, [c1, _, c3]) = three_commits();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c1.to_string(), "mode": "hard" }),
    );
    repo.write("a.txt", "new work\n");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"new work\n");
    assert_eq!(repo.rev("main"), c1);
    // The branch moved from a terminal: refused too.
    repo.write("a.txt", "1\n");
    repo.git(&["update-ref", "refs/heads/main", &c3.to_string()]);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
}

#[test]
fn untracked_file_in_the_way_is_never_overwritten() {
    let (repo, [c1, _, _]) = three_commits();
    // `u.txt` is in c1 but not in HEAD: an untracked file with that name would be overwritten.
    repo.write("u.txt", "mine\n");
    let (r, mut s) = open(&repo);
    let before = snapshot(&repo, &["u.txt"]);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c1.to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-in-the-way"),
        "{err}"
    );
    assert_eq!(snapshot(&repo, &["u.txt"]), before);
    // An untracked folder in its place too.
    std::fs::remove_file(repo.path().join("u.txt")).unwrap();
    repo.write("u.txt/inside", "mine\n");
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c1.to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(repo.path().join("u.txt/inside").exists());
    // `soft` and `mixed` do not touch the worktree: allowed.
    run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c1.to_string(), "mode": "mixed" }),
    );
    assert!(repo.path().join("u.txt/inside").exists());
}

#[test]
fn refusals_change_nothing() {
    // Merge in progress.
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let before = snapshot(&repo, &["a.txt"]);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": "HEAD~1", "mode": "hard", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(snapshot(&repo, &["a.txt"]), before);

    // Branch without commits.
    let repo = Repo::new();
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": "HEAD", "mode": "soft" }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-unborn"),
        "{err}"
    );

    // Bad targets: unknown, a tree, something that looks like an option.
    let (repo, [_, _, c3]) = three_commits();
    let (r, mut s) = open(&repo);
    let before = snapshot(&repo, &["a.txt"]);
    let tree = repo.git(&["rev-parse", "HEAD^{tree}"]);
    for target in ["nope", tree.as_str(), "--all", "-h"] {
        let err = r
            .run(
                "branch.reset",
                &mut s,
                json!({ "target": target, "mode": "hard", "confirm": true }),
            )
            .unwrap_err();
        assert!(
            !matches!(err, EngineError::ConfirmRequired { .. }),
            "{target}: {err}"
        );
        assert_eq!(snapshot(&repo, &["a.txt"]), before, "{target}");
    }
    assert_eq!(repo.rev("main"), c3);
}

#[test]
fn the_backup_restores_the_changes_and_odd_names_are_kept() {
    let repo = Repo::new();
    repo.commit_file("-dash file.txt", "1\n", "c1");
    repo.write("--all", "2\n");
    repo.stage(&["--all"]);
    repo.commit("c2");
    #[cfg(unix)]
    let odd = {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
        std::fs::write(repo.path().join(name), b"x\n").unwrap();
        repo.git(&["add", "--all"]);
        repo.commit("c3");
        std::fs::write(repo.path().join(name), b"changed\n").unwrap();
        name.to_owned()
    };
    repo.write("-dash file.txt", "local\n");
    repo.write("--all", "staged\n");
    repo.stage(&["--all"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": "HEAD", "mode": "hard", "confirm": true }),
    );
    let files: Vec<&str> = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f.as_str())
        .collect();
    #[cfg(unix)]
    assert_eq!(out["files"].as_array().unwrap().len(), 3, "{out}");
    assert!(
        files.contains(&"-dash file.txt") && files.contains(&"--all"),
        "{out}"
    );
    // Same commit: nothing in the journal, the recovery is the backup.
    assert_eq!(out["can_undo"], false);
    assert_eq!(repo.git(&["status", "--porcelain"]), "");
    let stash = out["stash"].as_str().unwrap();
    repo.git(&["stash", "apply", "--index", "--quiet", stash]);
    assert_eq!(bytes(&repo, "-dash file.txt").unwrap(), b"local\n");
    assert_eq!(repo.git(&["show", ":--all"]), "staged");
    #[cfg(unix)]
    assert_eq!(std::fs::read(repo.path().join(odd)).unwrap(), b"changed\n");
}

#[test]
fn assume_unchanged_files_that_the_reset_changes_are_refused() {
    let (repo, [_, c2, _]) = three_commits();
    repo.git(&["update-index", "--assume-unchanged", "a.txt"]);
    repo.write("a.txt", "hidden work\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c2.to_string(), "mode": "hard", "confirm": true }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-hidden"),
        "{err}"
    );
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"hidden work\n");
    // The same with `--skip-worktree`.
    repo.git(&["update-index", "--no-assume-unchanged", "a.txt"]);
    repo.git(&["update-index", "--skip-worktree", "a.txt"]);
    assert!(repo.git(&["ls-files", "-v", "a.txt"]).starts_with("S "));
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c2.to_string(), "mode": "hard", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"hidden work\n");
}

#[test]
fn locked_index_and_refused_ref_move_write_nothing() {
    let (repo, [c1, _, c3]) = three_commits();
    repo.write("a.txt", "local\n");
    let (r, mut s) = open(&repo);
    let before = snapshot(&repo, &["a.txt"]);
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();
    for mode in ["mixed", "hard"] {
        let err = r
            .run(
                "branch.reset",
                &mut s,
                json!({ "target": c1.to_string(), "mode": mode, "confirm": true }),
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::IndexLocked { .. }),
            "{mode}: {err}"
        );
    }
    std::fs::remove_file(&lock).unwrap();
    assert_eq!(snapshot(&repo, &["a.txt"]), before);

    // A `reference-transaction` hook that refuses to move `main`: the backup is made, then
    // nothing else; the error names it.
    let hooks = repo.path().join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("reference-transaction");
    std::fs::write(
        &hook,
        "#!/bin/sh\nif [ \"$1\" = prepared ] && grep -q ' refs/heads/main$'; then exit 1; fi\nexit 0\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c1.to_string(), "mode": "hard", "confirm": true }),
        )
        .unwrap_err();
    let backup = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ]);
    assert!(backup.starts_with("refs/project/backup/reset-"), "{backup}");
    assert!(err.to_string().contains(&backup), "{err}");
    assert_eq!(repo.rev("main"), c3);
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"local\n");
}

#[cfg(unix)]
#[test]
fn a_worktree_write_that_fails_after_the_move_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    let c1 = repo.commit_file("d/x.txt", "1\n", "c1");
    let c2 = repo.commit_file("d/x.txt", "2\n", "c2");
    let dir = repo.path().join("d");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(dir.join("probe"), "").is_ok() {
        // Root: the folder stays writable.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c1.to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let text = err.to_string();
    assert!(
        text.contains(&c2.to_string()),
        "names the previous commit: {text}"
    );
    assert_eq!(
        repo.rev("main"),
        c1,
        "the branch has moved, as the error says"
    );
}

#[test]
fn hidden_files_whose_index_entry_the_reset_rewrites_are_refused() {
    // Hard: `f` staged (index != HEAD), then hidden and edited; same in HEAD and HEAD~1.
    let repo = Repo::new();
    repo.commit_file("f", "1\n", "c1");
    let c1 = repo.rev("HEAD");
    let c2 = repo.commit_file("g", "g\n", "c2");
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        repo.write("f", "staged\n");
        repo.stage(&["f"]);
        repo.git(&["update-index", flag, "f"]);
        repo.write("f", "hidden\n");
        let (r, mut s) = open(&repo);
        let err = r
            .run(
                "branch.reset",
                &mut s,
                json!({ "target": c1.to_string(), "mode": "hard", "confirm": true }),
            )
            .unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-hidden"),
            "{flag}: {err}"
        );
        assert_eq!(repo.rev("main"), c2, "{flag}");
        assert_eq!(bytes(&repo, "f").unwrap(), b"hidden\n");
        let unflag = flag.replace("--", "--no-");
        repo.git(&["update-index", &unflag, "f"]);
        repo.git(&["reset", "--quiet", "--hard"]);
    }

    // Mixed: `f` hidden with a local edit, different between HEAD and the target.
    let (repo, [_, c2, c3]) = three_commits();
    repo.git(&["update-index", "--assume-unchanged", "a.txt"]);
    repo.write("a.txt", "hidden work\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c2.to_string(), "mode": "mixed" }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-hidden"),
        "{err}"
    );
    assert_eq!(repo.rev("main"), c3);
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"hidden work\n");
}

#[test]
fn a_tracked_folder_replaced_by_a_file_is_not_in_the_way() {
    let repo = Repo::new();
    let c1 = repo.commit_file("d", "file\n", "c1");
    repo.git(&["rm", "--quiet", "d"]);
    let c2 = repo.commit_file("d/x", "x\n", "c2");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "branch.reset",
        json!({ "target": c1.to_string(), "mode": "hard" }),
    );
    assert_eq!(bytes(&repo, "d").unwrap(), b"file\n");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c2);
    // With an untracked file inside the folder: refused, the file stays.
    repo.write("d/mine", "mine\n");
    let err = r
        .run(
            "branch.reset",
            &mut s,
            json!({ "target": c1.to_string(), "mode": "hard" }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.key() == "engine-reset-in-the-way"),
        "{err}"
    );
    assert_eq!(bytes(&repo, "d/mine").unwrap(), b"mine\n");
    assert_eq!(repo.rev("main"), c2);
}
