//! Git LFS on real repos (S63, `impl-docs/git/reading.md` "Git LFS"): the app runs git with the
//! user's filters and hooks, so with `git-lfs` installed `repo.clone`, `index.stage`,
//! `commit.create`, `remote.push`, `remote.pull`, `branch.checkout`, `worktree.discard`,
//! `stash.push`/`stash.pop` work as in a terminal; `worktree.discard_lines` refuses an LFS file;
//! `diff.worktree` shows the pointer text; `repo.lfs_paths` and `repo.lfs_status` tell the UI
//! which files are in LFS and whether git-lfs is missing.
//!
//! No LFS server: the remote is a local bare repo and git-lfs 3.x stores the objects in its
//! `lfs/objects` through its built-in standalone file transfer agent (a local path remote is a
//! `file://` URL for LFS). `git lfs install` is simulated as the user's global configuration
//! (`filter.lfs.*` through `GIT_CONFIG_COUNT`, like `protocol.file.allow` in `submodules.rs`);
//! git-lfs installs its hooks in each repo the first time a filter runs.
//!
//! Every test is skipped, with a message on stderr, when `git lfs version` fails (git-lfs not
//! installed).

mod common;

use common::{GIT_ENV, Repo};
use project_engine::{EngineError, Registry, Session};
use project_git::{CliOpener, GitVersion};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// `filter.lfs.*` as `git lfs install` writes them in the user's global configuration.
const LFS_CONFIG: &[(&str, &str)] = &[
    ("filter.lfs.clean", "git-lfs clean -- %f"),
    ("filter.lfs.smudge", "git-lfs smudge -- %f"),
    ("filter.lfs.process", "git-lfs filter-process"),
    ("filter.lfs.required", "true"),
];

/// Whether git-lfs is installed and git reads [`LFS_CONFIG`] (`GIT_CONFIG_COUNT`, from git 2.31);
/// prints why the test is skipped when not.
fn lfs_installed() -> bool {
    let version = std::process::Command::new("git")
        .arg("version")
        .output()
        .ok()
        .and_then(|o| GitVersion::parse(&String::from_utf8_lossy(&o.stdout)));
    if version.is_some_and(|v| v < GitVersion::new(2, 31, 0)) {
        eprintln!(
            "skipped: git older than 2.31 ignores `GIT_CONFIG_COUNT`, used for `filter.lfs.*`"
        );
        return false;
    }
    let ok = std::process::Command::new("git")
        .args(["lfs", "version"])
        .envs(GIT_ENV.iter().copied())
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("skipped: git-lfs is not installed (`git lfs version` fails)");
    }
    ok
}

/// The test opener plus [`LFS_CONFIG`] in the user's configuration.
fn opener() -> CliOpener {
    let mut opener = Repo::opener().env("GIT_CONFIG_COUNT", LFS_CONFIG.len().to_string());
    for (n, (key, value)) in LFS_CONFIG.iter().enumerate() {
        opener = opener
            .env(format!("GIT_CONFIG_KEY_{n}"), *key)
            .env(format!("GIT_CONFIG_VALUE_{n}"), *value);
    }
    opener
}

/// git in `dir` with the test environment and [`LFS_CONFIG`]; the test fails if git fails.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_output(dir, args);
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

fn git_output(dir: &Path, args: &[&str]) -> std::process::Output {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(dir);
    for (key, value) in LFS_CONFIG {
        cmd.arg("-c").arg(format!("{key}={value}"));
    }
    cmd.args(args)
        .envs(GIT_ENV.iter().copied())
        .output()
        .unwrap()
}

/// The first line of every LFS pointer.
const POINTER: &str = "version https://git-lfs.github.com/spec/v1";

/// A local bare remote `origin` and a repo `work` that tracks `*.bin` with LFS
/// (`.gitattributes`) and has pushed `big.bin` ("big 1") and `notes.txt` on `main`: the LFS
/// object is in the remote's `lfs/objects`.
struct LfsUpstream {
    origin: Repo,
    work: Repo,
}

fn repo_with_lfs_remote() -> LfsUpstream {
    let origin = Repo::bare();
    let work = Repo::new();
    work.write(
        ".gitattributes",
        "*.bin filter=lfs diff=lfs merge=lfs -text\n",
    );
    work.write("big.bin", "big 1\n");
    work.write("notes.txt", "notes\n");
    git(work.path(), &["add", "."]);
    git(work.path(), &["commit", "--quiet", "-m", "lfs"]);
    work.add_remote("origin", &origin);
    git(work.path(), &["push", "--quiet", "origin", "main"]);
    LfsUpstream { origin, work }
}

/// A clone of `upstream` made with `repo.clone`, open in its session.
struct Clone {
    _tmp: tempfile::TempDir,
    path: PathBuf,
    r: Registry,
    s: Session,
}

impl Clone {
    fn new(upstream: &Repo) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("clone");
        let mut clone = Clone {
            path: path.clone(),
            _tmp: tmp,
            r: Registry::builtin(),
            s: Session::new(Arc::new(opener())),
        };
        clone.ok(
            "repo.clone",
            json!({"url": upstream.path_str(), "path": path.to_str().unwrap()}),
        );
        clone
    }

    fn run(&mut self, id: &str, params: Value) -> Result<Value, EngineError> {
        self.r.run(id, &mut self.s, params)
    }

    fn ok(&mut self, id: &str, params: Value) -> Value {
        self.run(id, params).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.path.join(file)).unwrap()
    }

    fn write(&self, file: &str, content: &str) {
        std::fs::write(self.path.join(file), content).unwrap();
    }

    fn git(&self, args: &[&str]) -> String {
        git(&self.path, args)
    }
}

/// A clone of a repo with an LFS file, made with `repo.clone`: the clone's `origin` is the
/// bare remote of [`repo_with_lfs_remote`].
fn clone_with_lfs_file() -> (LfsUpstream, Clone) {
    let up = repo_with_lfs_remote();
    let clone = Clone::new(&up.origin);
    (up, clone)
}

#[test]
fn clone_smudges_lfs_files() {
    if !lfs_installed() {
        return;
    }
    let (_up, clone) = clone_with_lfs_file();
    assert_eq!(clone.read("big.bin"), "big 1\n");
    assert!(
        clone
            .git(&["cat-file", "-p", "HEAD:big.bin"])
            .starts_with(POINTER)
    );
    assert_eq!(clone.git(&["lfs", "ls-files", "--name-only"]), "big.bin");
}

#[test]
fn stage_commit_and_push_store_a_pointer_and_upload_the_object() {
    if !lfs_installed() {
        return;
    }
    let (up, mut clone) = clone_with_lfs_file();
    clone.write("new.bin", "new content\n");
    clone.write("big.bin", "big 2\n");
    clone.ok("index.stage", json!({"paths": ["new.bin", "big.bin"]}));
    let staged = clone.git(&["cat-file", "-p", ":new.bin"]);
    assert!(staged.starts_with(POINTER), "{staged}");
    clone.ok("commit.create", json!({"message": "more lfs"}));
    assert!(
        clone
            .git(&["cat-file", "-p", "HEAD:new.bin"])
            .starts_with(POINTER)
    );
    assert_eq!(clone.git(&["status", "--porcelain"]), "");

    clone.ok("remote.push", json!({}));
    // A fresh clone gets the new contents from the remote's LFS store.
    let other = Clone::new(&up.origin);
    assert_eq!(other.read("new.bin"), "new content\n");
    assert_eq!(other.read("big.bin"), "big 2\n");
}

#[test]
fn pull_downloads_the_new_lfs_objects() {
    if !lfs_installed() {
        return;
    }
    let (up, mut clone) = clone_with_lfs_file();
    up.work.write("big.bin", "big 2\n");
    git(up.work.path(), &["commit", "--quiet", "-am", "big 2"]);
    git(up.work.path(), &["push", "--quiet", "origin", "main"]);

    clone.ok("remote.pull", json!({}));
    assert_eq!(clone.read("big.bin"), "big 2\n");
    assert_eq!(clone.git(&["status", "--porcelain"]), "");
}

#[test]
fn checkout_smudges_the_other_branch() {
    if !lfs_installed() {
        return;
    }
    let (_up, mut clone) = clone_with_lfs_file();
    clone.ok("branch.create", json!({"name": "other"}));
    clone.ok("branch.checkout", json!({"name": "other"}));
    clone.write("big.bin", "big other\n");
    clone.ok("index.stage", json!({"paths": ["big.bin"]}));
    clone.ok("commit.create", json!({"message": "other"}));

    clone.ok("branch.checkout", json!({"name": "main"}));
    assert_eq!(clone.read("big.bin"), "big 1\n");
    clone.ok("branch.checkout", json!({"name": "other"}));
    assert_eq!(clone.read("big.bin"), "big other\n");
    assert_eq!(clone.git(&["status", "--porcelain"]), "");
}

#[test]
fn discard_restores_the_real_content() {
    if !lfs_installed() {
        return;
    }
    let (_up, mut clone) = clone_with_lfs_file();
    clone.write("big.bin", "changed\n");
    let out = clone.ok(
        "worktree.discard",
        json!({"paths": ["big.bin"], "confirm": true}),
    );
    assert_eq!(clone.read("big.bin"), "big 1\n");
    assert_eq!(clone.git(&["status", "--porcelain"]), "");
    assert!(out["backup"].is_string());
    // The undo brings the discarded bytes back, not a pointer.
    clone.ok("edit.undo", json!({}));
    assert_eq!(clone.read("big.bin"), "changed\n");
}

/// The `digest` of the `diff.worktree` (`index` false) or `diff.index` diff of `path`.
fn digest(clone: &mut Clone, index: bool, path: &str) -> String {
    let id = if index { "diff.index" } else { "diff.worktree" };
    let diff = clone.ok(id, json!({"path": path}));
    diff["files"][0]["digest"].as_str().unwrap().to_owned()
}

fn assert_lfs_refusal(err: EngineError) {
    match err {
        EngineError::NotAllowed(m) => assert_eq!(m.key(), "engine-lines-lfs", "{m:?}"),
        other => panic!("expected NotAllowed, got {other:?}"),
    }
}

/// Without the refusal, discarding the pointer's `oid` line wrote a pointer with two `oid`
/// lines to disk (git-lfs smudges an invalid pointer as is).
#[test]
fn discard_lines_refuses_an_lfs_file() {
    if !lfs_installed() {
        return;
    }
    let (_up, mut clone) = clone_with_lfs_file();
    clone.write("big.bin", "changed\n");
    let digest = digest(&mut clone, false, "big.bin");
    let err = clone
        .run(
            "worktree.discard_lines",
            json!({"path": "big.bin", "lines": [{"old": 2}], "digest": digest, "confirm": true}),
        )
        .unwrap_err();
    assert_lfs_refusal(err);
    assert_eq!(clone.read("big.bin"), "changed\n");
    assert_eq!(clone.git(&["for-each-ref", "refs/project/backup/"]), "");
}

/// Without the refusal, staging the new `oid` line put a pointer with two `oid` lines in the
/// index (a corrupt LFS file in the next commit).
#[test]
fn stage_and_unstage_lines_refuse_an_lfs_file() {
    if !lfs_installed() {
        return;
    }
    let (_up, mut clone) = clone_with_lfs_file();
    clone.write("big.bin", "changed\n");
    let index_before = clone.git(&["ls-files", "-s", "big.bin"]);
    let d = digest(&mut clone, false, "big.bin");
    let err = clone
        .run(
            "index.stage_lines",
            json!({"path": "big.bin", "lines": [{"new": 2}], "digest": d}),
        )
        .unwrap_err();
    assert_lfs_refusal(err);
    assert_eq!(clone.git(&["ls-files", "-s", "big.bin"]), index_before);

    clone.ok("index.stage", json!({"paths": ["big.bin"]}));
    let staged = clone.git(&["ls-files", "-s", "big.bin"]);
    let d = digest(&mut clone, true, "big.bin");
    let err = clone
        .run(
            "index.unstage_lines",
            json!({"path": "big.bin", "lines": [{"new": 2}], "digest": d}),
        )
        .unwrap_err();
    assert_lfs_refusal(err);
    assert_eq!(clone.git(&["ls-files", "-s", "big.bin"]), staged);
}

/// `diff.worktree` of an LFS file compares the pointers (the clean content): the `oid` and
/// `size` lines change, the `version` line is context. Not binary.
#[test]
fn diff_worktree_shows_the_pointer_text() {
    if !lfs_installed() {
        return;
    }
    let (_up, mut clone) = clone_with_lfs_file();
    clone.write("big.bin", "changed\n");
    let diff = clone.ok("diff.worktree", json!({"path": "big.bin"}));
    let file = &diff["files"][0];
    assert_eq!(file["binary"], false);
    let lines: Vec<(String, String)> = file["hunks"][0]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            let content = l["content"].as_str().unwrap();
            let start = content.split(' ').next().unwrap();
            (l["kind"].as_str().unwrap().to_owned(), start.to_owned())
        })
        .collect();
    let expected = [
        ("Context", "version"),
        ("Removed", "oid"),
        ("Removed", "size"),
        ("Added", "oid"),
        ("Added", "size"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(k, c)| ((*k).to_owned(), (*c).to_owned()))
        .collect();
    assert_eq!(lines, expected);
    let added_size = file["hunks"][0]["lines"][4]["content"].as_str().unwrap();
    assert_eq!(added_size, "size 8");
}

#[test]
fn stash_push_and_pop_keep_the_real_content() {
    if !lfs_installed() {
        return;
    }
    let (_up, mut clone) = clone_with_lfs_file();
    clone.write("big.bin", "work in progress\n");
    clone.ok("stash.push", json!({}));
    assert_eq!(clone.read("big.bin"), "big 1\n");
    assert!(
        clone
            .git(&["cat-file", "-p", "stash@{0}:big.bin"])
            .starts_with(POINTER)
    );
    clone.ok("stash.pop", json!({}));
    assert_eq!(clone.read("big.bin"), "work in progress\n");
    assert_eq!(clone.git(&["stash", "list"]), "");
}

/// `repo.lfs_paths` answers from the attributes, also for names with spaces, a leading `-`,
/// non-UTF-8 bytes and files that do not exist; each path once, in the order given.
#[test]
fn lfs_paths_reports_the_lfs_files() {
    if !lfs_installed() {
        return;
    }
    let (_up, mut clone) = clone_with_lfs_file();
    std::fs::write(
        clone.path.join(".gitattributes"),
        "*.bin filter=lfs diff=lfs merge=lfs -text\n*.dat filter=other\n",
    )
    .unwrap();
    let non_utf8: Vec<u8> = b"caf\xe9.bin".to_vec();
    let out = clone.ok(
        "repo.lfs_paths",
        json!({"paths": [
            "notes.txt", "-dash x.bin", "big.bin", "x.dat", non_utf8, "missing/a.bin", "big.bin"
        ]}),
    );
    assert_eq!(
        out,
        json!({"paths": ["-dash x.bin", "big.bin", non_utf8, "missing/a.bin"]})
    );
    assert_eq!(
        clone.ok("repo.lfs_paths", json!({"paths": []})),
        json!({"paths": []})
    );
    assert_eq!(
        clone.ok("repo.lfs_status", json!({})),
        json!({"used": true, "installed": true})
    );
}

/// A repo without LFS attributes: not used.
#[test]
fn lfs_status_of_a_repo_without_lfs() {
    if !lfs_installed() {
        return;
    }
    let repo = Repo::new();
    repo.commit_file("a.bin", "plain\n", "plain");
    let mut s = Session::new(Arc::new(opener()));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({"path": repo.path_str()}))
        .unwrap();
    let out = r.run("repo.lfs_status", &mut s, json!({})).unwrap();
    assert_eq!(out, json!({"used": false, "installed": true}));
    let out = r
        .run("repo.lfs_paths", &mut s, json!({"paths": ["a.bin"]}))
        .unwrap();
    assert_eq!(out, json!({"paths": []}));
}

/// A folder whose only program is a link to the real `git`: git-lfs is not on its `PATH`.
fn path_without_git_lfs() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|d| d.join("git"))
        .find(|p| p.is_file())
        .expect("git on PATH");
    std::os::unix::fs::symlink(git, dir.path().join("git")).unwrap();
    dir
}

/// A clone made by a user without git-lfs (no `filter.lfs.*`, no `git-lfs` program): the LFS
/// file stays a pointer in the worktree and `repo.lfs_status` says `used` but not `installed`
/// (the notice of the UI); `repo.lfs_paths` still reports the file (attributes only).
#[test]
fn repo_using_lfs_without_git_lfs() {
    if !lfs_installed() {
        return;
    }
    let up = repo_with_lfs_remote();
    let bin = path_without_git_lfs();
    let opener = Repo::opener().env("PATH", bin.path());
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("clone");
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(opener));
    r.run(
        "repo.clone",
        &mut s,
        json!({"url": up.origin.path_str(), "path": path.to_str().unwrap()}),
    )
    .unwrap();
    let content = std::fs::read_to_string(path.join("big.bin")).unwrap();
    assert!(content.starts_with(POINTER), "{content}");
    let out = r.run("repo.lfs_status", &mut s, json!({})).unwrap();
    assert_eq!(out, json!({"used": true, "installed": false}));
    let out = r
        .run(
            "repo.lfs_paths",
            &mut s,
            json!({"paths": ["big.bin", "notes.txt"]}),
        )
        .unwrap();
    assert_eq!(out, json!({"paths": ["big.bin"]}));
}
