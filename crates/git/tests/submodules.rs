//! Submodules on real repos (`impl-docs/git/submodules.md`): `repo.clone` with and without
//! `submodules`, `submodule.list` states, `submodule.update` (init, a superproject pull that moves
//! the submodule, the per-submodule refusals, the backup of a detached HEAD, nested submodules
//! only with `recursive`, names with spaces and not UTF-8, a URL starting with `-`, failures after
//! the backup, the user's `submodule.<name>.update`, local paths refused unless the user's
//! configuration allows them).
//!
//! The submodules are served from local paths: the backend under test gets
//! `protocol.file.allow=always` as the user's configuration (`GIT_CONFIG_COUNT`), the product
//! never adds it.

mod common;

use common::{GIT_ENV, Repo};
use project_engine::{EngineError, Registry, Session};
use project_git::CliOpener;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// The test opener plus `protocol.file.allow=always` in the user's configuration.
fn opener() -> CliOpener {
    Repo::opener()
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
        .env("GIT_CONFIG_VALUE_0", "always")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "protocol.file.allow=always"])
        .args(args)
        .envs(GIT_ENV.iter().copied())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

/// A library repo with one commit.
fn library(name: &str) -> Repo {
    let lib = Repo::new();
    lib.commit_file("lib.txt", &format!("{name} 1\n"), &format!("{name} 1"));
    lib
}

/// A superproject with the submodule `lib` (from `library`) at `path`.
fn superproject(library: &Repo, path: &str) -> Repo {
    let upstream = Repo::new();
    upstream.commit_file("README", "top\n", "top");
    git(
        upstream.path(),
        &["submodule", "--quiet", "add", library.path_str(), path],
    );
    upstream.commit("add the submodule");
    upstream
}

struct Clone {
    _tmp: tempfile::TempDir,
    path: std::path::PathBuf,
    r: Registry,
    s: Session,
}

impl Clone {
    fn run(&mut self, id: &str, params: Value) -> Result<Value, EngineError> {
        self.r.run(id, &mut self.s, params)
    }

    fn ok(&mut self, id: &str, params: Value) -> Value {
        self.run(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    fn sub(&self, path: &str) -> std::path::PathBuf {
        self.path.join(path)
    }

    /// `(path, state)` of every submodule.
    fn states(&mut self) -> Vec<(String, String)> {
        let out = self.ok("submodule.list", json!({}));
        out["submodules"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["path"].as_str().unwrap_or("<bytes>").to_owned(),
                    s["state"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }
}

fn clone_with(upstream: &Repo, submodules: bool, opener: CliOpener) -> Clone {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clone");
    let mut clone = Clone {
        path: path.clone(),
        _tmp: tmp,
        r: Registry::builtin(),
        s: Session::new(Arc::new(opener)),
    };
    clone.ok(
        "repo.clone",
        json!({"url": upstream.path_str(), "path": path.to_str().unwrap(), "submodules": submodules}),
    );
    clone
}

fn clone_of(upstream: &Repo, submodules: bool) -> Clone {
    clone_with(upstream, submodules, opener())
}

fn reasons(out: &Value) -> Vec<(String, String)> {
    out["refused"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["path"].as_str().unwrap().to_owned(),
                r["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.to_owned(), b.to_owned())
}

#[test]
fn clone_without_then_init_and_with_submodules() {
    let lib = library("lib");
    let upstream = superproject(&lib, "lib");
    let mut c = clone_of(&upstream, false);
    assert_eq!(c.states(), [pair("lib", "uninitialized")]);
    let list = c.ok("submodule.list", json!({}));
    let info = &list["submodules"][0];
    assert_eq!(info["name"], "lib");
    assert_eq!(info["url"], lib.path_str());
    assert_eq!(info["recorded"], lib.rev("HEAD").to_string());
    assert_eq!(info["head"], Value::Null);

    // Without `init`: refused, nothing cloned.
    let out = c.ok("submodule.update", json!({}));
    assert_eq!(reasons(&out), [pair("lib", "not_initialized")]);
    assert!(!c.sub("lib/lib.txt").exists());

    let out = c.ok("submodule.update", json!({"init": true}));
    assert_eq!(out["updated"][0]["path"], "lib");
    assert_eq!(out["updated"][0]["from"], Value::Null);
    assert_eq!(out["updated"][0]["to"], lib.rev("HEAD").to_string());
    assert_eq!(out["updated"][0]["backup"], Value::Null);
    assert_eq!(c.states(), [pair("lib", "in_sync")]);
    // No journal entry: the superproject's refs did not move.
    assert!(c.run("edit.undo", json!({})).is_err());

    let mut c = clone_of(&upstream, true);
    assert_eq!(c.states(), [pair("lib", "in_sync")]);
    assert!(c.sub("lib/lib.txt").exists());
}

#[test]
fn local_paths_need_the_users_configuration() {
    let lib = library("lib");
    let upstream = superproject(&lib, "lib");
    // Plain test opener: `protocol.file.allow` not configured, the product passes `user`.
    let mut c = clone_with(&upstream, false, Repo::opener());
    let err = c
        .run("submodule.update", json!({"init": true}))
        .unwrap_err();
    assert!(
        err.to_string().contains("transport 'file' not allowed"),
        "{err}"
    );
    let tmp = tempfile::tempdir().unwrap();
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    let dest = tmp.path().join("c");
    let err = r
        .run(
            "repo.clone",
            &mut s,
            json!({"url": upstream.path_str(), "path": dest.to_str().unwrap(), "submodules": true}),
        )
        .unwrap_err();
    assert!(err.to_string().contains("not allowed"), "{err}");
    assert!(!dest.exists(), "the failed clone is removed");
}

#[test]
fn pull_that_moves_the_submodule_then_update_saves_the_detached_head() {
    let lib = library("lib");
    let upstream = superproject(&lib, "lib");
    let mut c = clone_of(&upstream, true);
    let old = lib.rev("HEAD");
    let new = lib.commit_file("lib.txt", "lib 2\n", "lib 2");
    git(
        &upstream.path().join("lib"),
        &["pull", "--quiet", "--ff-only"],
    );
    git(upstream.path(), &["commit", "--quiet", "-am", "move lib"]);
    git(&c.path, &["pull", "--quiet", "--no-recurse-submodules"]);
    assert_eq!(c.states(), [pair("lib", "different_commit")]);

    let out = c.ok("submodule.update", json!({"paths": ["lib"]}));
    assert_eq!(out["updated"][0]["from"], old.to_string());
    assert_eq!(out["updated"][0]["to"], new.to_string());
    let backup = out["updated"][0]["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/submodule-"),
        "{backup}"
    );
    assert_eq!(git(&c.sub("lib"), &["rev-parse", &backup]), old.to_string());
    // The backup is inside the submodule, not in the superproject.
    assert!(
        git(&c.path, &["for-each-ref", "refs/project/"]).is_empty(),
        "no backup in the superproject"
    );
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(c.states(), [pair("lib", "in_sync")]);
}

/// A clone whose `lib` is one commit behind the recorded one: ready to update.
fn behind() -> (Repo, Repo, Clone, project_domain::Oid) {
    let lib = library("lib");
    let upstream = superproject(&lib, "lib");
    let c = clone_of(&upstream, true);
    let old = lib.rev("HEAD");
    lib.commit_file("lib.txt", "lib 2\n", "lib 2");
    git(
        &upstream.path().join("lib"),
        &["pull", "--quiet", "--ff-only"],
    );
    git(upstream.path(), &["commit", "--quiet", "-am", "move lib"]);
    git(&c.path, &["pull", "--quiet", "--no-recurse-submodules"]);
    (lib, upstream, c, old)
}

#[test]
fn local_changes_untracked_files_and_operations_are_refused() {
    let (_lib, _up, mut c, old) = behind();
    let lib = c.sub("lib");

    std::fs::write(lib.join("lib.txt"), "mine\n").unwrap();
    assert_eq!(c.states(), [pair("lib", "local_changes")]);
    let out = c.ok("submodule.update", json!({}));
    assert_eq!(reasons(&out), [pair("lib", "local_changes")]);
    assert_eq!(out["updated"], json!([]));
    assert_eq!(
        std::fs::read_to_string(lib.join("lib.txt")).unwrap(),
        "mine\n"
    );
    git(&lib, &["checkout", "--quiet", "--", "lib.txt"]);

    std::fs::write(lib.join("new.txt"), "untracked\n").unwrap();
    let out = c.ok("submodule.update", json!({}));
    assert_eq!(reasons(&out), [pair("lib", "untracked_files")]);
    std::fs::remove_file(lib.join("new.txt")).unwrap();

    let git_dir = git(&lib, &["rev-parse", "--absolute-git-dir"]);
    std::fs::create_dir(Path::new(&git_dir).join("rebase-merge")).unwrap();
    let out = c.ok("submodule.update", json!({}));
    assert_eq!(reasons(&out), [pair("lib", "operation_in_progress")]);
    std::fs::remove_dir(Path::new(&git_dir).join("rebase-merge")).unwrap();

    assert_eq!(git(&lib, &["rev-parse", "HEAD"]), old.to_string());
    let out = c.ok("submodule.update", json!({}));
    assert_eq!(out["refused"], json!([]));
    assert_eq!(out["updated"].as_array().unwrap().len(), 1);
}

#[test]
fn commits_on_no_ref_are_refused_but_a_branch_is_fine() {
    let (_lib, _up, mut c, _old) = behind();
    let lib = c.sub("lib");
    git(&lib, &["commit", "--quiet", "--allow-empty", "-m", "lost"]);
    let lost = git(&lib, &["rev-parse", "HEAD"]);
    let list = c.ok("submodule.list", json!({}));
    assert_eq!(list["submodules"][0]["unreferenced"], true);
    let out = c.ok("submodule.update", json!({}));
    assert_eq!(reasons(&out), [pair("lib", "unreferenced_commits")]);
    assert_eq!(git(&lib, &["rev-parse", "HEAD"]), lost);

    // On a branch the commit is kept by the branch: the update runs, no backup (not detached).
    git(&lib, &["switch", "--quiet", "-c", "work"]);
    let out = c.ok("submodule.update", json!({}));
    assert_eq!(out["refused"], json!([]));
    assert_eq!(out["updated"][0]["backup"], Value::Null);
    assert_eq!(git(&lib, &["rev-parse", "work"]), lost);
}

#[test]
fn nested_submodules_only_with_recursive() {
    let inner = library("inner");
    let middle = superproject(&inner, "inner");
    let upstream = superproject(&middle, "middle");
    let mut c = clone_of(&upstream, false);
    let out = c.ok("submodule.update", json!({"init": true}));
    assert_eq!(out["updated"].as_array().unwrap().len(), 1);
    assert!(c.sub("middle/README").exists());
    assert!(!c.sub("middle/inner/lib.txt").exists(), "not recursive");

    let out = c.ok("submodule.update", json!({"init": true, "recursive": true}));
    assert_eq!(out["unchanged"], json!(["middle"]));
    assert_eq!(out["updated"][0]["path"], "middle/inner");
    assert!(c.sub("middle/inner/lib.txt").exists());

    // A nested refusal is listed with its full path.
    git(
        &c.sub("middle/inner"),
        &["commit", "--quiet", "--allow-empty", "-m", "lost"],
    );
    let out = c.ok("submodule.update", json!({"recursive": true}));
    assert_eq!(out["unchanged"], json!(["middle"]));
    assert_eq!(
        reasons(&out),
        [pair("middle/inner", "unreferenced_commits")]
    );
}

#[test]
fn paths_with_spaces_and_a_folder_selection() {
    let lib = library("lib");
    let upstream = superproject(&lib, "libs/my lib");
    let mut c = clone_of(&upstream, false);
    assert_eq!(c.states(), [pair("libs/my lib", "uninitialized")]);
    let out = c.ok("submodule.update", json!({"paths": ["libs"], "init": true}));
    assert_eq!(out["updated"][0]["path"], "libs/my lib");
    assert!(c.sub("libs/my lib/lib.txt").exists());
    let err = c
        .run("submodule.update", json!({"paths": ["lib"]}))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_utf8() {
    use std::os::unix::ffi::OsStrExt;
    let lib = library("lib");
    let upstream = Repo::new();
    upstream.commit_file("README", "top\n", "top");
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(upstream.path())
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "--quiet",
            "add",
        ])
        .arg(lib.path())
        .arg(name)
        .envs(GIT_ENV.iter().copied())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    upstream.commit("add");
    let mut c = clone_of(&upstream, false);
    let list = c.ok("submodule.list", json!({}));
    assert_eq!(list["submodules"][0]["path"], json!([99, 97, 102, 233]));
    let out = c.ok(
        "submodule.update",
        json!({"paths": [[99, 97, 102, 233]], "init": true}),
    );
    assert_eq!(out["updated"][0]["path"], json!([99, 97, 102, 233]));
    assert!(c.path.join(name).join("lib.txt").exists());
}

#[test]
fn a_url_starting_with_a_dash_is_refused() {
    let lib = library("lib");
    let upstream = superproject(&lib, "lib");
    let mut c = clone_of(&upstream, false);
    git(
        &c.path,
        &[
            "config",
            "-f",
            ".gitmodules",
            "submodule.lib.url",
            "--upload-pack=touch x",
        ],
    );
    let out = c.ok("submodule.update", json!({"init": true}));
    assert_eq!(reasons(&out), [pair("lib", "suspicious_url")]);
    assert!(!c.sub("lib/lib.txt").exists());
}

#[test]
fn the_users_update_mode_does_not_change_the_checkout() {
    let (_lib, _up, mut c, _old) = behind();
    git(&c.path, &["config", "submodule.lib.update", "rebase"]);
    git(&c.sub("lib"), &["switch", "--quiet", "-c", "work"]);
    let out = c.ok("submodule.update", json!({}));
    let to = out["updated"][0]["to"].as_str().unwrap().to_owned();
    // Checked out (detached on the recorded commit), not rebased onto it.
    assert_eq!(git(&c.sub("lib"), &["rev-parse", "HEAD"]), to);
    assert_eq!(git(&c.sub("lib"), &["branch", "--show-current"]), "");
    assert_ne!(git(&c.sub("lib"), &["rev-parse", "work"]), to);
}

#[test]
fn a_failed_backup_runs_nothing() {
    let (_lib, _up, mut c, old) = behind();
    let lib = c.sub("lib");
    let git_dir = git(&lib, &["rev-parse", "--absolute-git-dir"]);
    // A file where the backup folder should be: `update-ref` cannot create the ref.
    std::fs::create_dir_all(Path::new(&git_dir).join("refs/project")).unwrap();
    std::fs::write(Path::new(&git_dir).join("refs/project/backup"), "").unwrap();
    assert!(c.run("submodule.update", json!({})).is_err());
    assert_eq!(git(&lib, &["rev-parse", "HEAD"]), old.to_string());
}

#[test]
fn a_failure_after_the_backup_names_it() {
    let (_lib, _up, mut c, old) = behind();
    let lib = c.sub("lib");
    // The recorded commit is not in the submodule and its remote is gone: the fetch fails
    // after the backup of the detached HEAD.
    git(&lib, &["remote", "set-url", "origin", "/nonexistent/lib"]);
    let err = c.run("submodule.update", json!({})).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("refs/project/backup/submodule-"), "{text}");
    let backups = git(
        &lib,
        &[
            "for-each-ref",
            "--format=%(objectname)",
            "refs/project/backup/",
        ],
    );
    assert_eq!(backups, old.to_string());
    assert_eq!(git(&lib, &["rev-parse", "HEAD"]), old.to_string());
}

#[cfg(unix)]
#[test]
fn a_failing_hook_inside_the_submodule_is_reported_with_the_backup() {
    use std::os::unix::fs::PermissionsExt;
    let (lib_repo, _up, mut c, _old) = behind();
    let lib = c.sub("lib");
    // The recorded commit must be there already: fetch it first so only the checkout runs.
    git(&lib, &["fetch", "--quiet", "origin"]);
    let hooks = Path::new(&git(&lib, &["rev-parse", "--absolute-git-dir"])).join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let hook = hooks.join("post-checkout");
    std::fs::write(&hook, "#!/bin/sh\necho 'hook says no' >&2\nexit 1\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = c.run("submodule.update", json!({})).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("hook says no"), "{text}");
    assert!(text.contains("refs/project/backup/submodule-"), "{text}");
    // git checked out before the hook ran: the state is reported as git left it.
    assert_eq!(
        git(&lib, &["rev-parse", "HEAD"]),
        lib_repo.rev("HEAD").to_string()
    );
    let list = c.ok("submodule.list", json!({}));
    assert_eq!(list["submodules"][0]["state"], "in_sync");
}
