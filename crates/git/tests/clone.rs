//! `repo.clone` and `repo.init` on real repos (`impl-docs/git/clone.md`): local remote by path and
//! by `file://`, a URL starting with `-`, destination not empty (or a file, a link, without
//! parent), cancellation (no folder left), `depth`, empty remote, branch, credentials; init with
//! and without `branch`.

mod common;

use common::Repo;
use project_engine::{CancelToken, EngineError, JobContext, Progress, Registry, Session};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn session() -> (Registry, Session) {
    (Registry::builtin(), Session::new(Arc::new(Repo::opener())))
}

/// A remote with two commits on `main` and a branch `feature` with one more.
fn remote() -> Repo {
    let remote = Repo::new();
    remote.commit_file("a.txt", "one\n", "first");
    remote.commit_file("a.txt", "two\n", "second");
    remote.branch("feature");
    remote.checkout("feature");
    remote.commit_file("b.txt", "b\n", "on feature");
    remote.checkout("main");
    remote
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn clone(r: &Registry, s: &mut Session, params: Value) -> Result<Value, EngineError> {
    r.run("repo.clone", s, params)
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(common::GIT_ENV.iter().copied())
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

fn head_branch(out: &Value) -> &str {
    out["head"]["Branch"]["name"].as_str().unwrap()
}

#[test]
fn clone_from_a_path_and_from_a_file_url() {
    let remote = remote();
    let tmp = tempfile::tempdir().unwrap();
    let (r, mut s) = session();
    let file_url = format!("file://{}", remote.path_str());
    for (url, name) in [
        (remote.path_str(), "by-path"),
        (file_url.as_str(), "by-url"),
    ] {
        let dest = tmp.path().join(name);
        let progress = Arc::new(Mutex::new(Vec::<Progress>::new()));
        let sink = Arc::clone(&progress);
        let out = r
            .run_with(
                "repo.clone",
                &mut s,
                json!({"url": url, "path": path_str(&dest)}),
                JobContext::new().with_progress(move |p| sink.lock().unwrap().push(p.clone())),
            )
            .unwrap();
        assert_eq!(out["path"], path_str(&dest), "{out}");
        assert_eq!(head_branch(&out), "main");
        assert_eq!(s.path(), Some(path_str(&dest)));
        assert_eq!(
            git(&dest, &["rev-parse", "HEAD"]),
            remote.rev("main").to_string()
        );
        assert_eq!(
            git(&dest, &["rev-parse", "origin/feature"]),
            remote.rev("feature").to_string()
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("a.txt")).unwrap(),
            "two\n"
        );
        let refs = r.run("repo.refs", &mut s, json!({})).unwrap().to_string();
        assert!(refs.contains("refs/remotes/origin/main"), "{refs}");
        if name == "by-url" {
            assert!(!progress.lock().unwrap().is_empty(), "no progress reported");
        }
        // Nothing to undo: the new repo starts with an empty journal.
        let undo = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(undo, EngineError::NotAllowed(_)), "{undo}");
    }
}

#[test]
fn clone_into_an_existing_empty_folder() {
    let remote = remote();
    let dest = tempfile::tempdir().unwrap();
    let (r, mut s) = session();
    let out = clone(
        &r,
        &mut s,
        json!({"url": remote.path_str(), "path": path_str(dest.path())}),
    )
    .unwrap();
    assert_eq!(head_branch(&out), "main");
    assert!(dest.path().join(".git").is_dir());
}

#[test]
fn a_url_starting_with_dash_is_not_an_option() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("pwned");
    let (r, mut s) = session();
    for url in [
        format!("--upload-pack=touch {}", marker.display()),
        "-x".to_owned(),
    ] {
        let dest = tmp.path().join("dest");
        let err = clone(&r, &mut s, json!({"url": url, "path": path_str(&dest)})).unwrap_err();
        assert!(
            matches!(err, EngineError::RemoteUnreachable { .. }),
            "{url}: {err}"
        );
        assert!(!marker.exists(), "{url} was taken as an option");
        assert!(!dest.exists(), "{url}: folder left behind");
    }
    assert!(s.path().is_none());
}

#[test]
fn destination_must_be_absent_or_empty() {
    let remote = remote();
    let tmp = tempfile::tempdir().unwrap();
    let full = tmp.path().join("full");
    std::fs::create_dir(&full).unwrap();
    std::fs::write(full.join("keep.txt"), "mine").unwrap();
    let file = tmp.path().join("file.txt");
    std::fs::write(&file, "mine").unwrap();
    let empty = tmp.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    let link = tmp.path().join("link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&empty, &link).unwrap();
    let no_parent = tmp.path().join("missing").join("dest");

    let (r, mut s) = session();
    r.run("repo.open", &mut s, json!({"path": remote.path_str()}))
        .unwrap();
    let mut targets = vec![&full, &file, &no_parent];
    if cfg!(unix) {
        targets.push(&link);
    }
    for dest in targets {
        let err = clone(
            &r,
            &mut s,
            json!({"url": remote.path_str(), "path": path_str(dest)}),
        )
        .unwrap_err();
        assert!(
            matches!(err, EngineError::NotAllowed(_)),
            "{}: {err}",
            dest.display()
        );
        let err = r
            .run("repo.init", &mut s, json!({"path": path_str(dest)}))
            .unwrap_err();
        assert!(
            matches!(err, EngineError::NotAllowed(_)),
            "{}: {err}",
            dest.display()
        );
    }
    assert_eq!(
        std::fs::read_to_string(full.join("keep.txt")).unwrap(),
        "mine"
    );
    assert_eq!(std::fs::read_dir(&full).unwrap().count(), 1);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine");
    assert!(!tmp.path().join("missing").exists());
    assert_eq!(std::fs::read_dir(&empty).unwrap().count(), 0);
    // The repo open before stays open.
    assert_eq!(s.path(), Some(remote.path_str()));
}

#[test]
fn depth_gives_a_shallow_clone() {
    let remote = remote();
    let tmp = tempfile::tempdir().unwrap();
    let (r, mut s) = session();
    let dest = tmp.path().join("shallow");
    let url = format!("file://{}", remote.path_str());
    let out = clone(
        &r,
        &mut s,
        json!({"url": url, "path": path_str(&dest), "depth": 1}),
    )
    .unwrap();
    assert_eq!(out["messages"], json!([]), "{out}");
    assert_eq!(git(&dest, &["rev-list", "--count", "HEAD"]), "1");
    assert_eq!(
        git(&dest, &["rev-parse", "--is-shallow-repository"]),
        "true"
    );
    // With a plain path git ignores it and says so.
    let dest = tmp.path().join("full");
    let out = clone(
        &r,
        &mut s,
        json!({"url": remote.path_str(), "path": path_str(&dest), "depth": 1}),
    )
    .unwrap();
    assert!(
        out["messages"].to_string().contains("--depth is ignored"),
        "{out}"
    );
    assert_eq!(git(&dest, &["rev-list", "--count", "HEAD"]), "2");
}

#[test]
fn clone_of_an_empty_remote() {
    let remote = Repo::bare();
    let tmp = tempfile::tempdir().unwrap();
    let (r, mut s) = session();
    let dest = tmp.path().join("empty");
    let out = clone(
        &r,
        &mut s,
        json!({"url": remote.path_str(), "path": path_str(&dest)}),
    )
    .unwrap();
    assert!(out["head"]["Branch"]["target"].is_null(), "{out}");
    assert!(
        out["messages"]
            .to_string()
            .contains("cloned an empty repository"),
        "{out}"
    );
    let log = r.run("repo.log", &mut s, json!({})).unwrap();
    assert_eq!(log["commits"], json!([]));
}

#[test]
fn clone_of_a_branch_and_of_a_missing_branch() {
    let remote = remote();
    let tmp = tempfile::tempdir().unwrap();
    let (r, mut s) = session();
    let dest = tmp.path().join("feature");
    let out = clone(
        &r,
        &mut s,
        json!({"url": remote.path_str(), "path": path_str(&dest), "branch": "feature"}),
    )
    .unwrap();
    assert_eq!(head_branch(&out), "feature");
    assert_eq!(
        git(&dest, &["rev-parse", "HEAD"]),
        remote.rev("feature").to_string()
    );

    let dest = tmp.path().join("missing");
    let err = clone(
        &r,
        &mut s,
        json!({"url": remote.path_str(), "path": path_str(&dest), "branch": "nope"}),
    )
    .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("nope"), "{err}");
    assert!(!dest.exists());
}

#[test]
fn invalid_parameters_write_nothing() {
    let remote = remote();
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("d");
    let (r, mut s) = session();
    for params in [
        json!({"url": remote.path_str(), "path": path_str(&dest), "depth": 0}),
        json!({"url": remote.path_str(), "path": path_str(&dest), "branch": "a..b"}),
        json!({"url": "", "path": path_str(&dest)}),
    ] {
        let err = clone(&r, &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    assert!(!dest.exists());
}

/// An "ssh" running `script`, for a remote `ssh://` URL that never touches the network.
#[cfg(unix)]
fn fake_ssh(script: &str) -> (tempfile::TempDir, PathBuf) {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fake-ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    (dir, path)
}

#[cfg(unix)]
#[test]
fn rejected_credentials_are_an_auth_error() {
    let (_dir, ssh) =
        fake_ssh("echo 'git@example.invalid: Permission denied (publickey).' >&2\nexit 255");
    let opener = Repo::opener().env("GIT_SSH_COMMAND", ssh.as_os_str());
    let (r, mut s) = (Registry::builtin(), Session::new(Arc::new(opener)));
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("d");
    let url = "ssh://git@example.invalid/r.git";
    let err = clone(&r, &mut s, json!({"url": url, "path": path_str(&dest)})).unwrap_err();
    assert!(
        matches!(&err, EngineError::Auth { remote, .. } if remote == url),
        "{err}"
    );
    assert!(!dest.exists());
}

#[cfg(unix)]
#[test]
fn cancel_midway_leaves_the_destination_as_it_was() {
    let (_dir, ssh) = fake_ssh("sleep 60");
    let opener = Repo::opener().env("GIT_SSH_COMMAND", ssh.as_os_str());
    let (r, mut s) = (Registry::builtin(), Session::new(Arc::new(opener)));
    let tmp = tempfile::tempdir().unwrap();
    let existing = tmp.path().join("existing");
    std::fs::create_dir(&existing).unwrap();
    for (dest, created) in [(tmp.path().join("new"), true), (existing, false)] {
        let token = CancelToken::new();
        let canceller = token.clone();
        let start = Instant::now();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            canceller.cancel();
        });
        let err = r
            .run_with(
                "repo.clone",
                &mut s,
                json!({"url": "ssh://git@example.invalid/r.git", "path": path_str(&dest)}),
                JobContext::new().with_cancel(token),
            )
            .unwrap_err();
        thread.join().unwrap();
        assert!(
            matches!(
                err,
                EngineError::Cancelled {
                    command: "repo.clone"
                }
            ),
            "{err}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{:?}",
            start.elapsed()
        );
        if created {
            assert!(!dest.exists(), "folder left behind");
        } else {
            assert!(dest.is_dir());
            assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);
        }
    }
    assert!(s.path().is_none());
}

#[test]
fn init_with_and_without_branch() {
    let tmp = tempfile::tempdir().unwrap();
    // Without `branch`: git's `init.defaultBranch`.
    // `GIT_CONFIG_PARAMETERS` and not `GIT_CONFIG_COUNT`, which exists from git 2.31.
    let opener = Repo::opener().env("GIT_CONFIG_PARAMETERS", "'init.defaultBranch=default-one'");
    let (r, mut s) = (Registry::builtin(), Session::new(Arc::new(opener)));
    let dest = tmp.path().join("plain");
    let out = r
        .run("repo.init", &mut s, json!({"path": path_str(&dest)}))
        .unwrap();
    assert_eq!(head_branch(&out), "default-one", "{out}");
    assert!(out["head"]["Branch"]["target"].is_null());
    assert!(dest.join(".git").is_dir());
    assert_eq!(s.path(), Some(path_str(&dest)));

    let dest = tmp.path().join("trunk");
    let out = r
        .run(
            "repo.init",
            &mut s,
            json!({"path": path_str(&dest), "branch": "trunk"}),
        )
        .unwrap();
    assert_eq!(head_branch(&out), "trunk");
    assert_eq!(git(&dest, &["symbolic-ref", "HEAD"]), "refs/heads/trunk");

    // Into an existing empty folder; then not again over the new repo.
    let empty = tmp.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    r.run("repo.init", &mut s, json!({"path": path_str(&empty)}))
        .unwrap();
    let err = r
        .run("repo.init", &mut s, json!({"path": path_str(&empty)}))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    let err = r
        .run(
            "repo.init",
            &mut s,
            json!({"path": path_str(&tmp.path().join("bad")), "branch": "-x"}),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert!(!tmp.path().join("bad").exists());
}
