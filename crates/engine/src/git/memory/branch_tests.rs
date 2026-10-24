//! Branch tests on the `MemoryBackend`, directly on the backend (the commands are tested in
//! `builtin/branch.rs`).

use super::*;
use crate::GitBackend;
use crate::git::{ConfigEntry, HeadState};
use project_domain::{Change, EntryKind, Head, StatusEntry};

fn clean() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo
}

fn target(repo: &MemoryBackend, name: &str) -> Option<Oid> {
    repo.refs.get(&format!("{HEADS}{name}")).map(|r| r.target)
}

#[test]
fn create_never_overwrites_and_checks_nested_names() {
    let mut repo = clean();
    let main = target(&repo, "main").unwrap();
    let graph = target(&repo, "feature/graph").unwrap();
    repo.create_branch("brand-new", &graph).unwrap();
    assert_eq!(target(&repo, "brand-new"), Some(graph));
    for name in ["main", "brand-new", "feature", "main/x"] {
        let err = repo.create_branch(name, &main).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{name}: {err}");
    }
    let err = repo.create_branch("-x", &main).unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    let err = repo
        .create_branch("y", &MemoryBackend::fake_oid(999_999))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    assert_eq!(target(&repo, "main"), Some(main));
}

#[test]
fn checkout_updates_head_index_and_worktree_or_refuses() {
    let mut repo = clean();
    let graph = target(&repo, "feature/graph").unwrap();
    let tree = repo.tree_at(graph);
    repo.checkout(&HeadState::Branch("refs/heads/feature/graph".to_owned()))
        .unwrap();
    assert!(matches!(repo.head().unwrap(), Head::Branch { name, .. } if name == "feature/graph"));
    assert_eq!(repo.head_tree(), tree);
    assert_eq!(repo.index_tree(), tree);
    assert!(repo.status().unwrap().entries.is_empty());

    // Detached HEAD on a commit, then back on the branch.
    let main = target(&repo, "main").unwrap();
    repo.checkout(&HeadState::Detached(main)).unwrap();
    assert_eq!(repo.head().unwrap(), Head::Detached(main));
    repo.checkout(&HeadState::Branch("refs/heads/main".to_owned()))
        .unwrap();

    // An untracked file that the destination branch contains: rejected, nothing changes.
    let path = repo.tree_at(graph).keys().next().unwrap().clone();
    let mut repo = clean();
    let untracked = StatusEntry {
        path: path.clone(),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    };
    // It is enough that the destination contains the file and the worktree has it untracked.
    repo.set_entries(vec![untracked]);
    let before = repo.clone();
    let err = repo
        .checkout(&HeadState::Branch("refs/heads/feature/graph".to_owned()))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::CheckoutConflict { files, .. } if files == &[path.to_string()]),
        "{err}"
    );
    assert_eq!(repo, before);

    let err = repo
        .checkout(&HeadState::Branch("refs/heads/missing".to_owned()))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn delete_refuses_the_current_branch_and_a_moved_one() {
    let mut repo = clean();
    let main = target(&repo, "main").unwrap();
    let graph = target(&repo, "feature/graph").unwrap();
    let err = repo.delete_branch("main", &main).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = repo.delete_branch("feature/graph", &main).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(target(&repo, "feature/graph"), Some(graph));

    let out = repo.delete_branch("feature/graph", &graph).unwrap();
    assert_eq!(out.backup, "refs/project/backup/delete-1");
    assert_eq!(
        repo.commit_backups().last().unwrap(),
        &("refs/project/backup/delete-1".to_owned(), graph)
    );
    assert_eq!(target(&repo, "feature/graph"), None);
    assert!(!repo.upstreams.contains_key("refs/heads/feature/graph"));
}

#[test]
fn rename_moves_head_and_upstream_and_never_overwrites() {
    let mut repo = clean();
    let main = target(&repo, "main").unwrap();
    let err = repo
        .rename_branch("main", "feature/graph", &main)
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = repo.rename_branch("main", "feature", &main).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(target(&repo, "main"), Some(main));

    repo.rename_branch("main", "trunk", &main).unwrap();
    assert_eq!(target(&repo, "main"), None);
    assert_eq!(target(&repo, "trunk"), Some(main));
    assert!(matches!(repo.head().unwrap(), Head::Branch { name, .. } if name == "trunk"));
    assert_eq!(
        repo.upstreams.get("refs/heads/trunk").map(String::as_str),
        Some("refs/remotes/origin/main")
    );
}

#[test]
fn unmerged_counts_commits_not_reachable_from_the_others() {
    let repo = clean();
    let main = target(&repo, "main").unwrap();
    let graph = target(&repo, "feature/graph").unwrap();
    assert_eq!(repo.unmerged_count(&graph, &[main]).unwrap(), 0);
    assert!(repo.unmerged_count(&main, &[graph]).unwrap() > 0);
    assert_eq!(
        repo.unmerged_count(&main, &[]).unwrap(),
        repo.reachable(main).len()
    );
}

#[test]
fn checkout_refuses_an_untracked_folder_where_the_target_has_a_file() {
    let graph = target(&clean(), "feature/graph").unwrap();
    let tree = clean().tree_at(graph);
    let file = tree.keys().next().unwrap().to_string();
    let dir = tree
        .keys()
        .find_map(|p| p.to_string().split_once('/').map(|(d, _)| d.to_owned()))
        .unwrap();
    // `d/x` untracked where the destination has the file `d`; an untracked file `d` where
    // the destination has the folder `d/`.
    for path in [format!("{file}/x"), dir] {
        let mut repo = clean();
        repo.set_entries(vec![StatusEntry {
            path: RepoPath::from(path.as_str()),
            index: Change::Untracked,
            worktree: Change::Untracked,
            kind: EntryKind::Untracked,
        }]);
        let before = repo.clone();
        let err = repo
            .checkout(&HeadState::Branch("refs/heads/feature/graph".to_owned()))
            .unwrap_err();
        assert!(
            matches!(&err, EngineError::CheckoutConflict { files, .. } if files == std::slice::from_ref(&path)),
            "{path}: {err}"
        );
        assert_eq!(repo, before);
    }
}

#[test]
fn delete_saves_the_upstream_and_restore_puts_it_back() {
    let mut repo = clean();
    let graph = target(&repo, "feature/graph").unwrap();
    let out = repo.delete_branch("feature/graph", &graph).unwrap();
    let config = vec![
        ConfigEntry {
            key: "branch.feature/graph.remote".to_owned(),
            value: Some("origin".to_owned()),
        },
        ConfigEntry {
            key: "branch.feature/graph.merge".to_owned(),
            value: Some("refs/heads/feature/graph".to_owned()),
        },
    ];
    assert_eq!(out.config, config);
    assert!(!repo.upstreams.contains_key("refs/heads/feature/graph"));
    repo.set_branch("feature/graph", graph);
    assert!(
        repo.restore_branch_config("feature/graph", &config, false)
            .is_empty()
    );
    assert_eq!(
        repo.upstreams
            .get("refs/heads/feature/graph")
            .map(String::as_str),
        Some("refs/remotes/origin/feature/graph")
    );
    // Already present: not rewritten, warning.
    assert_eq!(
        repo.restore_branch_config("feature/graph", &config, false)
            .len(),
        1
    );
    // Different from the saved one: the redo does not remove it.
    assert_eq!(
        repo.restore_branch_config("feature/graph", &[], true).len(),
        1
    );
    assert!(
        repo.restore_branch_config("feature/graph", &config, true)
            .is_empty()
    );
    assert!(!repo.upstreams.contains_key("refs/heads/feature/graph"));
}
