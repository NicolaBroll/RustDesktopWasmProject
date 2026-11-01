//! Linked worktrees on real repos (`impl-docs/git/worktrees.md`): `worktree.list` (main, linked,
//! locked, prunable), `worktree.add` (existing branch, new branch and its journal entry, detached,
//! branch checked out elsewhere, occupied path, relative path, a failing `post-checkout` hook,
//! names with spaces and a leading `-`) and `worktree.remove` (confirm, clean, changes, untracked,
//! ignored, assume-unchanged, locked, main, current, operation in progress, missing folder, the
//! backup of a detached HEAD, a failure after the backup).

mod common;

use common::Repo;
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Fixture {
    repo: Repo,
    /// Where the linked worktrees go (outside the repo).
    dir: tempfile::TempDir,
    r: Registry,
    s: Session,
}

impl Fixture {
    /// A repo with one commit on `main` and a branch `feat`, open in a session.
    fn new() -> Self {
        let repo = Repo::new();
        repo.commit_file("a.txt", "a\n", "one");
        repo.branch("feat");
        let mut s = Session::new(Arc::new(Repo::opener()));
        s.open(repo.path_str()).unwrap();
        Self {
            repo,
            dir: tempfile::tempdir().unwrap(),
            r: Registry::builtin(),
            s,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn at(&self, name: &str) -> String {
        self.path(name).to_str().unwrap().to_owned()
    }

    fn run(&mut self, id: &str, params: Value) -> Result<Value, EngineError> {
        self.r.run(id, &mut self.s, params)
    }

    fn add(&mut self, name: &str, extra: Value) -> Value {
        let mut params = json!({ "path": self.at(name) });
        params
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.run("worktree.add", params)
            .unwrap_or_else(|e| panic!("worktree.add {name}: {e}"))
    }

    fn remove(&mut self, name: &str, confirm: bool) -> Result<Value, EngineError> {
        let path = self.at(name);
        self.run("worktree.remove", json!({"path": path, "confirm": confirm}))
    }

    fn list(&mut self) -> Vec<Value> {
        let out = self.run("worktree.list", json!({})).unwrap();
        out["worktrees"].as_array().unwrap().clone()
    }

    /// git run inside the linked worktree `name`.
    fn git_in(&self, name: &str, args: &[&str]) -> String {
        let path = self.at(name);
        let mut all = vec!["-C", path.as_str()];
        all.extend_from_slice(args);
        // `Repo::git` adds `-C <repo>` first; the second `-C` is relative to it (absolute here).
        self.repo.git(&all)
    }
}

fn key(err: &EngineError) -> String {
    match err {
        EngineError::NotAllowed(m) => m.key().to_owned(),
        EngineError::ConfirmRequired { reason, .. } => reason.key().to_owned(),
        EngineError::GitWithReason { reason, .. } => reason.key().to_owned(),
        other => format!("{other:?}"),
    }
}

fn same(a: &str, b: &Path) -> bool {
    std::fs::canonicalize(a).ok() == std::fs::canonicalize(b).ok()
}

#[test]
fn add_on_an_existing_branch_then_list_and_remove() {
    let mut f = Fixture::new();
    let out = f.add("w", json!({"branch": "feat"}));
    assert!(same(out["path"].as_str().unwrap(), &f.path("w")), "{out}");
    assert_eq!(out["branch"], "refs/heads/feat");
    assert_eq!(out["created_branch"], false);
    assert_eq!(out["head"], f.repo.rev("feat").to_string());
    assert_eq!(out["can_undo"], false);
    assert!(f.path("w/a.txt").exists());
    assert_eq!(f.git_in("w", &["symbolic-ref", "HEAD"]), "refs/heads/feat");

    let list = f.list();
    assert_eq!(list.len(), 2, "{list:?}");
    assert_eq!(list[0]["main"], true);
    assert_eq!(list[0]["current"], true);
    assert_eq!(list[0]["branch"], "refs/heads/main");
    assert_eq!(list[1]["main"], false);
    assert_eq!(list[1]["current"], false);
    assert_eq!(list[1]["branch"], "refs/heads/feat");
    assert_eq!(list[1]["locked"], false);
    assert_eq!(list[1]["prunable"], false);

    // The new worktree opens as a repo of its own.
    let mut other = Session::new(Arc::new(Repo::opener()));
    other.open(&f.at("w")).unwrap();

    // Without confirm nothing happens.
    let err = f.remove("w", false).unwrap_err();
    assert_eq!(key(&err), "engine-worktree-remove-confirm");
    assert!(f.path("w/a.txt").exists());
    let out = f.remove("w", true).unwrap();
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["worktrees"].as_array().unwrap().len(), 1, "{out}");
    assert!(!f.path("w").exists());
    // The branch stays.
    assert!(f.repo.git(&["branch", "--list", "feat"]).contains("feat"));
}

#[test]
fn add_with_a_new_branch_goes_into_the_journal() {
    let mut f = Fixture::new();
    let main = f.repo.rev("main");
    let out = f.add("w", json!({"new_branch": "topic", "start": "main"}));
    assert_eq!(out["created_branch"], true);
    assert_eq!(out["branch"], "refs/heads/topic");
    assert_eq!(out["next_undo"]["key"], "engine-journal-branch-create");
    assert_eq!(f.repo.rev("topic"), main);
    assert_eq!(f.git_in("w", &["symbolic-ref", "HEAD"]), "refs/heads/topic");
    // No upstream, whatever start was.
    assert!(
        !f.repo
            .git_output(&["config", "branch.topic.merge"])
            .status
            .success()
    );
    // Undo would delete the branch of the worktree: refused.
    let err = f.run("edit.undo", json!({})).unwrap_err();
    assert_eq!(key(&err), "engine-undo-delete-worktree-branch");
    f.remove("w", true).unwrap();
    f.run("edit.undo", json!({})).unwrap();
    assert!(f.repo.git(&["branch", "--list", "topic"]).is_empty());
}

#[test]
fn add_detached_and_with_a_relative_path() {
    let mut f = Fixture::new();
    let main = f.repo.rev("main");
    // Relative to the root of the open worktree.
    let rel = format!(
        "../{}/d",
        f.dir.path().file_name().unwrap().to_str().unwrap()
    );
    // Only if the temp folders are siblings (they are, under the same temp dir).
    if f.repo.path().parent() == f.dir.path().parent() {
        let out = f.run("worktree.add", json!({"path": rel})).unwrap();
        assert!(same(out["path"].as_str().unwrap(), &f.path("d")), "{out}");
        assert_eq!(out["branch"], Value::Null);
        assert_eq!(f.git_in("d", &["rev-parse", "HEAD"]), main.to_string());
        assert!(
            !f.repo
                .git_output(&["-C", &f.at("d"), "symbolic-ref", "-q", "HEAD"])
                .status
                .success()
        );
        // No branch named after the folder (git's default without a commit-ish).
        assert!(f.repo.git(&["branch", "--list", "d"]).is_empty());
    }
}

#[test]
fn add_refuses_a_branch_checked_out_elsewhere_and_an_occupied_path() {
    let mut f = Fixture::new();
    let path = f.at("x");
    let err = f
        .run("worktree.add", json!({"path": path, "branch": "main"}))
        .unwrap_err();
    assert_eq!(key(&err), "engine-worktree-branch-in-use");
    f.add("w", json!({"branch": "feat"}));
    let err = f
        .run("worktree.add", json!({"path": path, "branch": "feat"}))
        .unwrap_err();
    assert_eq!(key(&err), "engine-worktree-branch-in-use");
    assert!(!f.path("x").exists());

    std::fs::create_dir(f.path("full")).unwrap();
    std::fs::write(f.path("full/keep.txt"), "mine").unwrap();
    let path = f.at("full");
    let err = f
        .run("worktree.add", json!({"path": path, "new_branch": "n"}))
        .unwrap_err();
    assert_eq!(key(&err), "engine-clone-not-empty");
    assert_eq!(
        std::fs::read_to_string(f.path("full/keep.txt")).unwrap(),
        "mine"
    );
    assert!(f.repo.git(&["branch", "--list", "n"]).is_empty());
    // An existing empty folder is used.
    std::fs::create_dir(f.path("empty")).unwrap();
    f.add("empty", json!({}));
    assert!(f.path("empty/a.txt").exists());
}

#[test]
fn a_failing_git_leaves_no_branch_and_no_folder() {
    let mut f = Fixture::new();
    // The parent folder does not exist: refused before git.
    let path = f.at("no/such");
    let err = f
        .run("worktree.add", json!({"path": path, "new_branch": "n"}))
        .unwrap_err();
    assert_eq!(key(&err), "engine-clone-no-parent");
    assert!(f.repo.git(&["branch", "--list", "n"]).is_empty());
    // A worktree administrative folder git cannot create: git fails after the branch exists.
    let admin = f.repo.path().join(".git/worktrees");
    std::fs::write(&admin, "not a folder").unwrap();
    let path = f.at("w");
    let err = f
        .run("worktree.add", json!({"path": path, "new_branch": "n"}))
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err:?}");
    assert!(f.repo.git(&["branch", "--list", "n"]).is_empty());
    assert!(!f.path("w").exists());
    std::fs::remove_file(&admin).unwrap();
}

#[cfg(unix)]
#[test]
fn a_failing_post_checkout_hook_is_reported() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    let hooks = f.repo.path().join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    std::fs::write(&hook, "#!/bin/sh\necho hook says no >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = f.add("w", json!({"branch": "feat"}));
    let warnings = out["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{out}");
    assert_eq!(warnings[0]["key"], "engine-worktree-add-hook");
    assert!(
        warnings[0]["text"]
            .as_str()
            .unwrap()
            .contains("hook says no")
    );
    assert!(f.path("w/a.txt").exists());
}

#[test]
fn names_with_spaces_and_a_leading_dash() {
    let mut f = Fixture::new();
    let out = f.add("-w x", json!({"new_branch": "n"}));
    assert!(
        same(out["path"].as_str().unwrap(), &f.path("-w x")),
        "{out}"
    );
    let list = f.list();
    assert!(
        list.iter()
            .any(|w| same(w["path"].as_str().unwrap(), &f.path("-w x")))
    );
    f.remove("-w x", true).unwrap();
    assert!(!f.path("-w x").exists());
}

#[test]
fn remove_refuses_changes_untracked_ignored_and_hidden_files() {
    let mut f = Fixture::new();
    f.add("w", json!({"branch": "feat"}));
    let w = f.path("w");
    let refused = |f: &mut Fixture, expected: &str| {
        let err = f.remove("w", true).unwrap_err();
        assert_eq!(key(&err), expected, "{err}");
        assert!(f.path("w/a.txt").exists());
    };

    std::fs::write(w.join("a.txt"), "changed\n").unwrap();
    refused(&mut f, "engine-worktree-changes");
    f.git_in("w", &["add", "a.txt"]);
    refused(&mut f, "engine-worktree-changes");
    f.git_in("w", &["reset", "-q", "--hard"]);

    std::fs::write(w.join("new.txt"), "new\n").unwrap();
    refused(&mut f, "engine-worktree-untracked");
    std::fs::remove_file(w.join("new.txt")).unwrap();

    std::fs::write(f.repo.path().join(".git/info/exclude"), "*.o\n").unwrap();
    std::fs::write(w.join("build.o"), "obj").unwrap();
    refused(&mut f, "engine-worktree-ignored");
    assert!(w.join("build.o").exists());
    std::fs::remove_file(w.join("build.o")).unwrap();

    f.git_in("w", &["update-index", "--assume-unchanged", "a.txt"]);
    std::fs::write(w.join("a.txt"), "hidden change\n").unwrap();
    refused(&mut f, "engine-worktree-hidden");
    assert_eq!(
        std::fs::read_to_string(w.join("a.txt")).unwrap(),
        "hidden change\n"
    );
    f.git_in("w", &["update-index", "--no-assume-unchanged", "a.txt"]);
    f.git_in("w", &["checkout", "--", "a.txt"]);

    f.remove("w", true).unwrap();
}

#[test]
fn remove_refuses_locked_main_current_and_an_operation_in_progress() {
    let mut f = Fixture::new();
    f.add("w", json!({"branch": "feat"}));
    f.repo
        .git(&["worktree", "lock", "--reason", "on a usb disk", &f.at("w")]);
    let list = f.list();
    assert_eq!(list[1]["locked"], true, "{list:?}");
    assert_eq!(list[1]["lock_reason"], "on a usb disk");
    assert_eq!(list[1]["prunable"], false);
    let err = f.remove("w", true).unwrap_err();
    assert_eq!(key(&err), "engine-worktree-locked-reason");
    f.repo.git(&["worktree", "unlock", &f.at("w")]);

    let main = f.repo.path_str().to_owned();
    let err = f
        .run("worktree.remove", json!({"path": main, "confirm": true}))
        .unwrap_err();
    assert_eq!(key(&err), "engine-worktree-main");
    let err = f
        .run(
            "worktree.remove",
            json!({"path": "/no/such/worktree", "confirm": true}),
        )
        .unwrap_err();
    assert_eq!(key(&err), "engine-worktree-unknown");

    // From the linked worktree: itself is the current one, the main one is still main.
    let mut other = Session::new(Arc::new(Repo::opener()));
    other.open(&f.at("w")).unwrap();
    let r = Registry::builtin();
    let here = f.at("w");
    let err = r
        .run(
            "worktree.remove",
            &mut other,
            json!({"path": here, "confirm": true}),
        )
        .unwrap_err();
    assert_eq!(key(&err), "engine-worktree-current");
    let err = r
        .run(
            "worktree.remove",
            &mut other,
            json!({"path": main, "confirm": true}),
        )
        .unwrap_err();
    assert_eq!(key(&err), "engine-worktree-main");
    let list = r.run("worktree.list", &mut other, json!({})).unwrap();
    assert_eq!(list["worktrees"][1]["current"], true, "{list}");
    assert_eq!(list["worktrees"][0]["current"], false);

    // A merge stopped on a conflict in the worktree.
    f.git_in("w", &["checkout", "-q", "-b", "side"]);
    std::fs::write(f.path("w/a.txt"), "side\n").unwrap();
    f.git_in("w", &["commit", "-q", "-am", "side"]);
    f.repo.commit_file("a.txt", "main\n", "main change");
    let out = f
        .repo
        .git_output(&["-C", &f.at("w"), "merge", "-q", "main"]);
    assert!(!out.status.success());
    let err = f.remove("w", true).unwrap_err();
    assert_eq!(key(&err), "engine-worktree-operation");
    assert!(f.path("w/a.txt").exists());
}

#[test]
fn a_missing_folder_is_prunable_and_removed_with_its_detached_head_saved() {
    let mut f = Fixture::new();
    f.add("w", json!({}));
    std::fs::write(f.path("w/b.txt"), "b\n").unwrap();
    f.git_in("w", &["add", "b.txt"]);
    f.git_in("w", &["commit", "-q", "-m", "only here"]);
    let lost = f.git_in("w", &["rev-parse", "HEAD"]);
    std::fs::remove_dir_all(f.path("w")).unwrap();
    let list = f.list();
    assert_eq!(list[1]["prunable"], true, "{list:?}");
    assert_eq!(list[1]["detached"], true);
    let err = f.remove("w", false).unwrap_err();
    assert_eq!(key(&err), "engine-worktree-remove-confirm");
    let out = f.remove("w", true).unwrap();
    let backup = out["backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/worktree-"), "{out}");
    assert_eq!(f.repo.rev(backup).to_string(), lost);
    assert_eq!(f.list().len(), 1);
}

#[test]
fn a_detached_head_on_a_branch_needs_no_backup() {
    let mut f = Fixture::new();
    f.add("w", json!({"start": "feat"}));
    let out = f.remove("w", true).unwrap();
    assert_eq!(out["backup"], Value::Null);
}

#[cfg(unix)]
#[test]
fn a_failure_after_the_backup_keeps_it_and_names_it() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    f.add("w", json!({}));
    std::fs::create_dir(f.path("w/d")).unwrap();
    std::fs::write(f.path("w/d/b.txt"), "b\n").unwrap();
    f.git_in("w", &["add", "d/b.txt"]);
    f.git_in("w", &["commit", "-q", "-m", "only here"]);
    let lost = f.git_in("w", &["rev-parse", "HEAD"]);
    // A folder git cannot empty: the removal fails midway.
    let mode = |m| std::fs::Permissions::from_mode(m);
    std::fs::set_permissions(f.path("w/d"), mode(0o555)).unwrap();
    if std::fs::write(f.path("w/d/probe"), "").is_ok() {
        eprintln!("skipped: the folder stays writable (root)");
        std::fs::set_permissions(f.path("w/d"), mode(0o755)).unwrap();
        return;
    }
    let err = f.remove("w", true).unwrap_err();
    std::fs::set_permissions(f.path("w/d"), mode(0o755)).unwrap();
    assert_eq!(key(&err), "engine-worktree-remove-failed", "{err}");
    let text = err.to_string();
    let backup = text
        .split('`')
        .find(|t| t.starts_with("refs/project/backup/worktree-"))
        .unwrap_or_else(|| panic!("{text}"));
    assert_eq!(f.repo.rev(backup).to_string(), lost);
    assert!(f.path("w/d/b.txt").exists());
}

#[test]
fn a_folder_that_no_longer_holds_the_worktree_is_refused() {
    let mut f = Fixture::new();
    f.add("w", json!({"branch": "feat"}));
    std::fs::remove_file(f.path("w/.git")).unwrap();
    let list = f.list();
    assert_eq!(list[1]["prunable"], true, "{list:?}");
    let err = f.remove("w", true).unwrap_err();
    assert_eq!(key(&err), "engine-worktree-not-worktree");
    assert!(f.path("w/a.txt").exists());
}

#[test]
fn add_refuses_the_path_of_a_stale_entry() {
    let mut f = Fixture::new();
    f.add("w", json!({}));
    std::fs::remove_dir_all(f.path("w")).unwrap();
    let path = f.at("w");
    let err = f
        .run("worktree.add", json!({"path": path, "new_branch": "n"}))
        .unwrap_err();
    assert_eq!(key(&err), "engine-worktree-path-registered", "{err}");
    assert!(f.repo.git(&["branch", "--list", "n"]).is_empty());
    assert!(!f.path("w").exists());
    let err = f.run("edit.undo", json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_neither_prunable_nor_removed() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    std::fs::create_dir(f.path("p")).unwrap();
    f.add("p/w", json!({}));
    let mode = |m| std::fs::Permissions::from_mode(m);
    std::fs::set_permissions(f.path("p"), mode(0o000)).unwrap();
    if std::fs::read_dir(f.path("p")).is_ok() {
        std::fs::set_permissions(f.path("p"), mode(0o755)).unwrap();
        eprintln!("skipped: the folder stays readable (root)");
        return;
    }
    let list = f.list();
    let err = f.remove("p/w", true).unwrap_err();
    std::fs::set_permissions(f.path("p"), mode(0o755)).unwrap();
    assert_eq!(list[1]["prunable"], false, "{list:?}");
    assert_eq!(key(&err), "engine-worktree-unreachable", "{err}");
    assert!(f.path("p/w/a.txt").exists());
}
