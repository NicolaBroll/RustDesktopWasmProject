//! Configuration inherited from the environment (`GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_<n>`,
//! `GIT_CONFIG_VALUE_<n>`, `GIT_CONFIG_PARAMETERS`): the backend ignores it. A separate file
//! because it changes the process environment, which the tests of a file share.

mod common;

use common::Repo;
use project_domain::RepoPath;
use project_engine::GitBackend;

#[test]
fn inherited_config_cannot_redirect_writes() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "first");
    repo.write("b.txt", "b\n");
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::write(elsewhere.path().join("b.txt"), "elsewhere\n").unwrap();
    let mut backend = repo.open();

    let target = elsewhere.path().to_str().unwrap();
    let injected = format!("'core.worktree={target}'");
    // SAFETY: it is the only test of this executable, no other thread reads the environment.
    unsafe {
        std::env::set_var("GIT_CONFIG_COUNT", "1");
        std::env::set_var("GIT_CONFIG_KEY_0", "core.worktree");
        std::env::set_var("GIT_CONFIG_VALUE_0", target);
        std::env::set_var("GIT_CONFIG_PARAMETERS", &injected);
    }
    backend.stage(&[RepoPath::from("b.txt")]).unwrap();
    let status = backend.status().unwrap();
    unsafe {
        for key in [
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_PARAMETERS",
        ] {
            std::env::remove_var(key);
        }
    }
    // The staged file is the repo's, not the one in the other folder.
    assert_eq!(repo.git(&["show", ":b.txt"]), "b");
    assert_eq!(status.entries[0].path, RepoPath::from("b.txt"));
    assert_eq!(status.entries[0].index.letter(), 'A');
}
