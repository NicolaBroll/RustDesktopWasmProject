//! `commit.create` with a hostile inherited environment (e.g. the app started from a git hook or
//! from a terminal with git variables set): the commit goes into the open repo, with its index,
//! without the injected configuration, without an editor and with the current date (not the one
//! from the inherited `GIT_*_DATE` variables). A separate file because it changes the
//! process environment, which the tests of one file share.

mod common;

use common::Repo;
use project_engine::{CommitRequest, GitBackend};

#[test]
fn inherited_environment_does_not_change_the_commit() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let other = Repo::new();
    other.commit_file("x.txt", "x\n", "other repo");
    let other_head = other.rev("HEAD");
    let mut backend = repo.open();

    let other_git = other.path().join(".git");
    let other_index = other_git.join("index");
    let vars: [(&str, &std::ffi::OsStr); 8] = [
        // Inherited dates (e.g. from a hook): they must not end up in the commit.
        ("GIT_AUTHOR_DATE", "1000000000 +0000".as_ref()),
        ("GIT_COMMITTER_DATE", "1000000000 +0000".as_ref()),
        ("GIT_DIR", other_git.as_os_str()),
        ("GIT_INDEX_FILE", other_index.as_os_str()),
        ("GIT_EDITOR", "false".as_ref()),
        (
            "GIT_CONFIG_PARAMETERS",
            "'commit.gpgsign=true' 'gpg.program=false'".as_ref(),
        ),
        ("GIT_CONFIG_COUNT", "1".as_ref()),
        ("GIT_CONFIG_KEY_0", "core.hooksPath".as_ref()),
    ];
    // SAFETY: this is the only test of this executable, no other thread reads the environment.
    unsafe {
        for (key, value) in &vars {
            std::env::set_var(key, value);
        }
        std::env::set_var("GIT_CONFIG_VALUE_0", "/nonexistent");
    }
    let result = backend.commit(&CommitRequest::new("in the right repo"));
    unsafe {
        for (key, _) in &vars {
            std::env::remove_var(key);
        }
        std::env::remove_var("GIT_CONFIG_VALUE_0");
    }
    let out = result.unwrap();
    assert_eq!(out.commit.id, repo.rev("HEAD"));
    assert_eq!(
        repo.git(&["show", "--format=", "--name-only", "HEAD"]),
        "b.txt"
    );
    assert_eq!(repo.git(&["log", "-1", "--format=%B"]), "in the right repo");
    assert_eq!(other.rev("HEAD"), other_head);
    assert_ne!(out.commit.author.time, 1_000_000_000);
    assert_ne!(out.commit.committer.time, 1_000_000_000);
}
