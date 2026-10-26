//! Helpers for integration tests: real git repos in temporary folders, in a deterministic
//! environment (`impl-docs/ARCHITECTURE-git.md` §9).
//!
//! Every git process (those of the tests and those of the backend opened with [`Repo::open`])
//! runs without global or system configuration, with a fixed author, committer and initial
//! branch. Dates start from [`START_TIME`] and every git command run by the repo moves them
//! forward by [`TICK`] seconds: the same scenario always gives the same commit ids.
//!
//! The ready-made scenarios are the `repo_*` functions at the bottom of the file.

// Every test file uses only part of the helper.
#![allow(dead_code)]

use project_domain::Oid;
use project_git::{CliBackend, CliOpener};
use std::cell::Cell;
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

/// Environment of every git process of the tests. `GIT_CONFIG_GLOBAL` exists from git 2.32:
/// with older git `HOME` and `XDG_CONFIG_HOME` keep the user's configuration out.
pub const GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("HOME", "/dev/null"),
    ("XDG_CONFIG_HOME", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Ada"),
    ("GIT_AUTHOR_EMAIL", "ada@example.com"),
    ("GIT_COMMITTER_NAME", "Bob"),
    ("GIT_COMMITTER_EMAIL", "bob@example.com"),
    ("LC_ALL", "C"),
];

/// Date of the first git command of every repo, in seconds since the Unix epoch.
pub const START_TIME: i64 = 1_700_000_000;
/// Seconds between one git command and the next.
pub const TICK: i64 = 60;
/// Time zone of the test dates.
pub const OFFSET: &str = "+0200";

/// A temporary repo, deleted when it goes out of scope.
pub struct Repo {
    dir: TempDir,
    clock: Cell<i64>,
}

impl Repo {
    /// Empty repo with a worktree, on `main`.
    pub fn new() -> Self {
        Self::init(&[])
    }

    /// Empty bare repo (`HEAD` on `main`): to use as a local remote, no network.
    pub fn bare() -> Self {
        Self::init(&["--bare"])
    }

    fn init(extra: &[&str]) -> Self {
        let repo = Self {
            dir: tempfile::tempdir().unwrap(),
            clock: Cell::new(START_TIME),
        };
        let path = repo.path_str().to_owned();
        let mut args = vec!["-c", "init.defaultBranch=main", "init", "--quiet"];
        args.extend_from_slice(extra);
        args.push(&path);
        let out = repo.command(&args).output().unwrap();
        assert!(out.status.success(), "git init: {}", stderr(&out));
        repo
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn path_str(&self) -> &str {
        self.path().to_str().unwrap()
    }

    /// Opener with the same environment as the tests.
    pub fn opener() -> CliOpener {
        GIT_ENV
            .iter()
            .fold(CliOpener::new(), |opener, (k, v)| opener.env(*k, *v))
    }

    /// The backend under test, opened on the repo root.
    pub fn open(&self) -> CliBackend {
        Self::opener().open_cli(self.path_str()).unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let date = format!("{} {OFFSET}", self.clock.get());
        self.clock.set(self.clock.get() + TICK);
        let mut cmd = Command::new("git");
        cmd.args(args)
            .envs(GIT_ENV.iter().copied())
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date);
        cmd
    }

    /// Runs git in the repo, whatever the result.
    pub fn git_output(&self, args: &[&str]) -> Output {
        let mut cmd = self.command(&[]);
        cmd.arg("-C").arg(self.path()).args(args);
        cmd.output().unwrap()
    }

    /// Runs git in the repo and returns stdout without trailing newlines; the test fails if
    /// git fails.
    pub fn git(&self, args: &[&str]) -> String {
        let out = self.git_output(args);
        assert!(out.status.success(), "git {args:?}: {}", stderr(&out));
        String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
    }

    /// Runs git in the repo and checks that it fails (e.g. a conflicting merge).
    pub fn git_fails(&self, args: &[&str]) {
        let out = self.git_output(args);
        assert!(!out.status.success(), "git {args:?} should have failed");
    }

    /// The id `rev` points to.
    pub fn rev(&self, rev: &str) -> Oid {
        self.git(&["rev-parse", "--verify", "--end-of-options", rev])
            .parse()
            .unwrap()
    }

    /// Writes `file` (with intermediate folders) without staging it.
    pub fn write(&self, file: &str, content: &str) {
        let path = self.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    /// Stages `files`.
    pub fn stage(&self, files: &[&str]) {
        self.git(&[&["add", "--"], files].concat());
    }

    /// Commits what is staged; returns the id.
    pub fn commit(&self, message: &str) -> Oid {
        self.git(&["commit", "--quiet", "-m", message]);
        self.rev("HEAD")
    }

    /// Writes `file`, stages it and creates a commit; returns the id.
    pub fn commit_file(&self, file: &str, content: &str, message: &str) -> Oid {
        self.write(file, content);
        self.stage(&[file]);
        self.commit(message)
    }

    /// Creates branch `name` on HEAD, without switching to it.
    pub fn branch(&self, name: &str) {
        self.git(&["branch", "--end-of-options", name]);
    }

    /// Switches to branch `name`. The trailing `--` makes `name` be read as a branch, not a file.
    pub fn checkout(&self, name: &str) {
        self.git(&["checkout", "--quiet", name, "--"]);
    }

    /// Detaches HEAD at commit `id`.
    pub fn detach(&self, id: Oid) {
        self.git(&["checkout", "--quiet", "--detach", &id.to_string()]);
    }

    /// Merges `branch` into the current branch with a merge commit (`--no-ff`) and git's
    /// default message; returns the merge id.
    pub fn merge(&self, branch: &str) -> Oid {
        self.git(&["merge", "--quiet", "--no-ff", "--no-edit", branch]);
        self.rev("HEAD")
    }

    /// Prepares a conflict on `file`: from the current commit, branch `other` writes it with
    /// `theirs` and the current branch with `ours`. Returns the id of the commit of `other`; HEAD
    /// stays on the current branch, which must exist.
    pub fn diverge(&self, other: &str, file: &str, ours: &str, theirs: &str) -> Oid {
        let current = self.git(&["symbolic-ref", "--short", "HEAD"]);
        self.branch(other);
        self.checkout(other);
        let theirs = self.commit_file(file, theirs, &format!("{file} on {other}"));
        self.checkout(&current);
        self.commit_file(file, ours, &format!("{file} on {current}"));
        theirs
    }

    /// Merge of `branch` that ends in a conflict: the repo stays mid-merge.
    pub fn conflict(&self, branch: &str) {
        self.git_fails(&["merge", "--no-edit", branch]);
    }

    /// Lightweight tag `name` on `rev`.
    pub fn tag(&self, name: &str, rev: &str) {
        self.git(&["tag", "--end-of-options", name, rev]);
    }

    /// Annotated tag `name` on `rev`; returns the id of the tag object.
    pub fn annotated_tag(&self, name: &str, rev: &str, message: &str) -> Oid {
        self.git(&["tag", "-a", "-m", message, "--end-of-options", name, rev]);
        self.rev(&format!("refs/tags/{name}"))
    }

    /// `git stash` of the changes to tracked files; returns the stash id.
    pub fn stash(&self) -> Oid {
        self.git(&["stash", "--quiet"]);
        self.rev("refs/stash")
    }

    /// Adds remote `name` pointing to repo `remote` (usually [`Repo::bare`]).
    pub fn add_remote(&self, name: &str, remote: &Repo) {
        self.git(&["remote", "add", "--", name, remote.path_str()]);
    }

    /// Pushes `rev` to branch `branch` of the remote, then fetches: the local remote-tracking
    /// branch (`refs/remotes/<remote>/<branch>`) is up to date.
    pub fn push(&self, remote: &str, rev: &str, branch: &str) {
        self.git(&[
            "push",
            "--quiet",
            remote,
            &format!("{rev}:refs/heads/{branch}"),
        ]);
        self.git(&["fetch", "--quiet", remote]);
    }

    /// The local branch `branch` tracks `<remote>/<branch>`.
    pub fn set_upstream(&self, branch: &str, remote: &str) {
        let upstream = format!("{remote}/{branch}");
        self.git(&["branch", "--quiet", "--set-upstream-to", &upstream, branch]);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

// --- Scenarios ---

/// The ids of [`repo_with_history`].
pub struct History {
    pub repo: Repo,
    pub c1: Oid,
    pub c2: Oid,
    pub f1: Oid,
    pub f2: Oid,
    pub m: Oid,
}

/// Two branches and a merge, HEAD on `main`:
///
/// ```text
/// m   Merge branch 'feature'        (main)
/// |\
/// | f2  feature 2                   (feature)
/// | f1  feature 1
/// c2 |  second on main, with a two-line body
/// |/
/// c1  first commit                  (root)
/// ```
pub fn repo_with_history() -> History {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "one\n", "first commit");
    repo.branch("feature");
    let c2 = repo.commit_file("a.txt", "two\n", "second on main\n\nBody\non two lines.");
    repo.checkout("feature");
    let f1 = repo.commit_file("b.txt", "b\n", "feature 1");
    let f2 = repo.commit_file("b.txt", "bb\n", "feature 2");
    repo.checkout("main");
    let m = repo.merge("feature");
    History {
        repo,
        c1,
        c2,
        f1,
        f2,
        m,
    }
}

/// [`repo_with_history`] with refs of every kind:
///
/// - remote `origin` (local bare repo, returned) with `main` at `c2`; `main` tracks it and is
///   ahead by 3 (`f1`, `f2`, `m`);
/// - lightweight tag `light` at `c1`, annotated tag `v1` at `c2`;
/// - a stash with a change to `a.txt`; the worktree stays clean.
pub fn repo_with_all_refs() -> (History, Repo) {
    let h = repo_with_history();
    let origin = Repo::bare();
    h.repo.add_remote("origin", &origin);
    h.repo.push("origin", &h.c2.to_string(), "main");
    h.repo.set_upstream("main", "origin");
    h.repo.tag("light", &h.c1.to_string());
    h.repo.annotated_tag("v1", &h.c2.to_string(), "version 1");
    h.repo.write("a.txt", "modified\n");
    h.repo.stash();
    (h, origin)
}

/// Worktree with a change of every kind, relative to two commits:
///
/// - `a.txt` modified and not staged;
/// - `new.txt` added to the index;
/// - `old.txt` renamed to `renamed.txt` (staged);
/// - `dir/untracked file.txt` untracked (with a space in the name).
pub fn repo_with_changes() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "one\n", "first");
    repo.commit_file(
        "old.txt",
        "content long enough to be recognized\n",
        "second",
    );
    repo.write("a.txt", "modified\n");
    repo.write("new.txt", "staged\n");
    repo.stage(&["new.txt"]);
    repo.git(&["mv", "old.txt", "renamed.txt"]);
    repo.write("dir/untracked file.txt", "?\n");
    repo
}

/// `main` and `other` change `a.txt` differently from a common commit; no operation
/// in progress. Returns the repo and the id of the commit of `other`.
pub fn repo_with_divergent_branches() -> (Repo, Oid) {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    let theirs = repo.diverge("other", "a.txt", "ours\n", "theirs\n");
    (repo, theirs)
}

/// [`repo_with_divergent_branches`] with the merge of `other` into `main` stopped on the
/// conflict in `a.txt`.
pub fn repo_with_merge_conflict() -> Repo {
    let (repo, _) = repo_with_divergent_branches();
    repo.conflict("other");
    repo
}

/// The ids of [`repo_with_file_changes`].
pub struct FileChanges {
    pub repo: Repo,
    /// Root commit: `old.txt`, `move.txt`, `deleted.txt`, `script.sh`.
    pub base: Oid,
    /// Relative to `base`: `old.txt` → `new.txt` (exact rename), `move.txt` →
    /// `moved.txt` (rename with changes), `deleted.txt` deleted, `script.sh` made
    /// executable, added `dir/with space.txt` and a file with a non-UTF-8 name (byte `0xff`,
    /// Unix only).
    pub changes: Oid,
    /// Empty commit (`--allow-empty`) after `changes`.
    pub empty: Oid,
}

/// The non-UTF-8 name of [`repo_with_file_changes`].
pub const NON_UTF8_NAME: &[u8] = b"cafe\xff.txt";

/// A commit with a change of every kind, and an empty commit after it.
pub fn repo_with_file_changes() -> FileChanges {
    let repo = Repo::new();
    repo.write("old.txt", "content long enough to recognize the rename\n");
    repo.write(
        "move.txt",
        "first line long enough\nsecond line\nthird line\nfourth line\n",
    );
    repo.write("deleted.txt", "to delete\n");
    repo.write("script.sh", "#!/bin/sh\n");
    repo.git(&["add", "-A"]);
    let base = repo.commit("base");

    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.git(&["mv", "move.txt", "moved.txt"]);
    let moved = std::fs::read_to_string(repo.path().join("moved.txt")).unwrap();
    repo.write("moved.txt", &format!("{moved}fifth line\n"));
    repo.git(&["rm", "--quiet", "deleted.txt"]);
    repo.write("dir/with space.txt", "space\n");
    #[cfg(unix)]
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        std::fs::write(
            repo.path().join(OsStr::from_bytes(NON_UTF8_NAME)),
            "latin\n",
        )
        .unwrap();
    }
    repo.git(&["add", "-A"]);
    // After `add -A`, which would take the mode again from the file system.
    repo.git(&["update-index", "--chmod=+x", "script.sh"]);
    let changes = repo.commit("every kind of change");
    repo.git(&["commit", "--quiet", "--allow-empty", "-m", "empty"]);
    let empty = repo.rev("HEAD");
    FileChanges {
        repo,
        base,
        changes,
        empty,
    }
}

/// HEAD detached at commit `f1` of [`repo_with_history`].
pub fn repo_detached_head() -> History {
    let h = repo_with_history();
    h.repo.detach(h.f1);
    h
}

/// Names that git would read as special pathspecs if they were not taken literally, in the
/// root commit and then all modified outside the index: `file*.txt` and `file1.txt` (the first
/// as a glob includes the second), `a[1].txt` and `a1.txt`, `:(top)magic.txt` (magic pathspec)
/// and `-dash.txt` (looks like an option).
pub fn repo_with_special_names() -> Repo {
    const NAMES: &[&str] = &[
        "file*.txt",
        "file1.txt",
        "a[1].txt",
        "a1.txt",
        ":(top)magic.txt",
        "-dash.txt",
    ];
    let repo = Repo::new();
    for name in NAMES {
        repo.write(name, "before\n");
    }
    repo.git(&["add", "-A"]);
    repo.commit("special names");
    for name in NAMES {
        repo.write(name, &format!("after: {name}\n"));
    }
    repo
}

/// The ids of [`repo_with_renamed_file`].
pub struct RenamedFile {
    pub repo: Repo,
    pub add: Oid,
    pub two: Oid,
    pub rename: Oid,
    pub four: Oid,
    pub five: Oid,
    pub one: Oid,
    pub merge: Oid,
}

/// The history of a file renamed and changed on two branches, HEAD on `main`:
///
/// ```text
/// merge  Merge branch 'side'      (main)  b.txt = ONE TWO three four five
/// |\
/// | five  b.txt: + five           (side)
/// one |   b.txt: one → ONE
/// |/
/// four    b.txt: + four
/// rename  a.txt → b.txt
/// two     a.txt: two → TWO
/// add     a.txt = one two three   (root)
/// ```
pub fn repo_with_renamed_file() -> RenamedFile {
    let repo = Repo::new();
    let add = repo.commit_file("a.txt", "one\ntwo\nthree\n", "add");
    let two = repo.commit_file("a.txt", "one\nTWO\nthree\n", "two");
    repo.git(&["mv", "a.txt", "b.txt"]);
    let rename = repo.commit("rename");
    let four = repo.commit_file("b.txt", "one\nTWO\nthree\nfour\n", "four");
    repo.branch("side");
    repo.checkout("side");
    let five = repo.commit_file("b.txt", "one\nTWO\nthree\nfour\nfive\n", "five");
    repo.checkout("main");
    let one = repo.commit_file("b.txt", "ONE\nTWO\nthree\nfour\n", "one");
    let merge = repo.merge("side");
    RenamedFile {
        repo,
        add,
        two,
        rename,
        four,
        five,
        one,
        merge,
    }
}
