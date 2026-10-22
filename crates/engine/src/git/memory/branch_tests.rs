//! Test dei branch sul `MemoryBackend`, direttamente sul backend (i comandi sono provati in
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
    let grafo = target(&repo, "feature/grafo").unwrap();
    repo.create_branch("nuovo", &grafo).unwrap();
    assert_eq!(target(&repo, "nuovo"), Some(grafo));
    for name in ["main", "nuovo", "feature", "main/x"] {
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
    let grafo = target(&repo, "feature/grafo").unwrap();
    let tree = repo.tree_at(grafo);
    repo.checkout(&HeadState::Branch("refs/heads/feature/grafo".to_owned()))
        .unwrap();
    assert!(matches!(repo.head().unwrap(), Head::Branch { name, .. } if name == "feature/grafo"));
    assert_eq!(repo.head_tree(), tree);
    assert_eq!(repo.index_tree(), tree);
    assert!(repo.status().unwrap().entries.is_empty());

    // HEAD staccata su un commit, poi di nuovo sul branch.
    let main = target(&repo, "main").unwrap();
    repo.checkout(&HeadState::Detached(main)).unwrap();
    assert_eq!(repo.head().unwrap(), Head::Detached(main));
    repo.checkout(&HeadState::Branch("refs/heads/main".to_owned()))
        .unwrap();

    // Un non tracciato che il branch di destinazione contiene: rifiutato, niente cambia.
    let path = repo.tree_at(grafo).keys().next().unwrap().clone();
    let mut repo = clean();
    let untracked = StatusEntry {
        path: path.clone(),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    };
    // Basta che la destinazione contenga il file e che il worktree lo abbia non tracciato.
    repo.set_entries(vec![untracked]);
    let before = repo.clone();
    let err = repo
        .checkout(&HeadState::Branch("refs/heads/feature/grafo".to_owned()))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::CheckoutConflict { files, .. } if files == &[path.to_string()]),
        "{err}"
    );
    assert_eq!(repo, before);

    let err = repo
        .checkout(&HeadState::Branch("refs/heads/nessuno".to_owned()))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn delete_refuses_the_current_branch_and_a_moved_one() {
    let mut repo = clean();
    let main = target(&repo, "main").unwrap();
    let grafo = target(&repo, "feature/grafo").unwrap();
    let err = repo.delete_branch("main", &main).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = repo.delete_branch("feature/grafo", &main).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(target(&repo, "feature/grafo"), Some(grafo));

    let out = repo.delete_branch("feature/grafo", &grafo).unwrap();
    assert_eq!(out.backup, "refs/project/backup/delete-1");
    assert_eq!(
        repo.commit_backups().last().unwrap(),
        &("refs/project/backup/delete-1".to_owned(), grafo)
    );
    assert_eq!(target(&repo, "feature/grafo"), None);
    assert!(!repo.upstreams.contains_key("refs/heads/feature/grafo"));
}

#[test]
fn rename_moves_head_and_upstream_and_never_overwrites() {
    let mut repo = clean();
    let main = target(&repo, "main").unwrap();
    let err = repo
        .rename_branch("main", "feature/grafo", &main)
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
    let grafo = target(&repo, "feature/grafo").unwrap();
    assert_eq!(repo.unmerged_count(&grafo, &[main]).unwrap(), 0);
    assert!(repo.unmerged_count(&main, &[grafo]).unwrap() > 0);
    assert_eq!(
        repo.unmerged_count(&main, &[]).unwrap(),
        repo.reachable(main).len()
    );
}

#[test]
fn checkout_refuses_an_untracked_folder_where_the_target_has_a_file() {
    let grafo = target(&clean(), "feature/grafo").unwrap();
    let tree = clean().tree_at(grafo);
    let file = tree.keys().next().unwrap().to_string();
    let dir = tree
        .keys()
        .find_map(|p| p.to_string().split_once('/').map(|(d, _)| d.to_owned()))
        .unwrap();
    // `d/x` non tracciato dove la destinazione ha il file `d`; un file non tracciato `d` dove
    // la destinazione ha la cartella `d/`.
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
            .checkout(&HeadState::Branch("refs/heads/feature/grafo".to_owned()))
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
    let grafo = target(&repo, "feature/grafo").unwrap();
    let out = repo.delete_branch("feature/grafo", &grafo).unwrap();
    let config = vec![
        ConfigEntry {
            key: "branch.feature/grafo.remote".to_owned(),
            value: Some("origin".to_owned()),
        },
        ConfigEntry {
            key: "branch.feature/grafo.merge".to_owned(),
            value: Some("refs/heads/feature/grafo".to_owned()),
        },
    ];
    assert_eq!(out.config, config);
    assert!(!repo.upstreams.contains_key("refs/heads/feature/grafo"));
    repo.set_branch("feature/grafo", grafo);
    assert!(
        repo.restore_branch_config("feature/grafo", &config, false)
            .is_empty()
    );
    assert_eq!(
        repo.upstreams
            .get("refs/heads/feature/grafo")
            .map(String::as_str),
        Some("refs/remotes/origin/feature/grafo")
    );
    // Già presente: non si riscrive, avviso.
    assert_eq!(
        repo.restore_branch_config("feature/grafo", &config, false)
            .len(),
        1
    );
    // Diversa da quella salvata: il redo non la toglie.
    assert_eq!(
        repo.restore_branch_config("feature/grafo", &[], true).len(),
        1
    );
    assert!(
        repo.restore_branch_config("feature/grafo", &config, true)
            .is_empty()
    );
    assert!(!repo.upstreams.contains_key("refs/heads/feature/grafo"));
}
