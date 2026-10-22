//! Backup dell'indice prima di stage e unstage (§1 e §7): le versioni in stage che non sono né
//! in HEAD né nel worktree si salvano in un ref `refs/project/backup/...` prima di scrivere.

mod common;

use common::{Repo, repo_with_merge_conflict};
use project_domain::RepoPath;
use project_engine::{EngineError, GitBackend};

fn paths(list: &[&str]) -> Vec<RepoPath> {
    list.iter().map(|p| RepoPath::from(*p)).collect()
}

/// I ref di backup, in ordine.
fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ]);
    out.lines().map(str::to_owned).collect()
}

/// `git show <rev>:<path>`.
fn show(repo: &Repo, rev: &str, path: &str) -> String {
    repo.git(&["show", &format!("{rev}:{path}")])
}

fn index_bytes(repo: &Repo) -> Vec<u8> {
    std::fs::read(repo.path().join(".git/index")).unwrap()
}

/// Un repo con `a.txt` committato.
fn base() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "in HEAD\n", "primo");
    repo
}

/// Nessun indice temporaneo rimasto nella cartella git.
fn assert_no_temp_index(repo: &Repo) {
    let left: Vec<_> = std::fs::read_dir(repo.path().join(".git"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("project-backup-index")
        })
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[test]
fn unstage_added_then_deleted_keeps_a_backup() {
    let repo = base();
    repo.write("nuovo.txt", "esiste solo in stage\n");
    repo.stage(&["nuovo.txt"]);
    std::fs::remove_file(repo.path().join("nuovo.txt")).unwrap();
    let mut backend = repo.open();
    assert_eq!(backend.status().unwrap().entries[0].index.letter(), 'A');

    let backup = backend.unstage(&paths(&["nuovo.txt"])).unwrap().unwrap();
    assert!(backup.starts_with("refs/project/backup/index-"), "{backup}");
    assert!(
        backend.status().unwrap().entries.is_empty(),
        "il file non c'è più"
    );
    // Il contenuto si recupera dal ref; il commit ha HEAD come genitore.
    assert_eq!(show(&repo, &backup, "nuovo.txt"), "esiste solo in stage");
    assert_eq!(repo.rev(&format!("{backup}^")), repo.rev("HEAD"));
    let message = repo.git(&["log", "-1", "--format=%B", &backup]);
    assert!(message.contains("index.unstage"), "{message}");
    assert!(message.contains("nuovo.txt"), "{message}");
    assert_no_temp_index(&repo);
}

#[test]
fn stage_added_then_deleted_keeps_a_backup() {
    let repo = base();
    repo.write("nuovo.txt", "esiste solo in stage\n");
    repo.stage(&["nuovo.txt"]);
    std::fs::remove_file(repo.path().join("nuovo.txt")).unwrap();
    let mut backend = repo.open();
    let backup = backend.stage(&paths(&["nuovo.txt"])).unwrap().unwrap();
    assert!(backend.status().unwrap().entries.is_empty());
    assert_eq!(show(&repo, &backup, "nuovo.txt"), "esiste solo in stage");
}

#[test]
fn unstage_modified_then_deleted_keeps_a_backup() {
    let repo = base();
    repo.write("a.txt", "versione in stage\n");
    repo.stage(&["a.txt"]);
    std::fs::remove_file(repo.path().join("a.txt")).unwrap();
    let mut backend = repo.open();
    let backup = backend.unstage(&paths(&["a.txt"])).unwrap().unwrap();
    let status = backend.status().unwrap();
    assert_eq!(
        (
            status.entries[0].index.letter(),
            status.entries[0].worktree.letter()
        ),
        ('.', 'D')
    );
    assert_eq!(show(&repo, &backup, "a.txt"), "versione in stage");
}

#[test]
fn staged_version_unique_to_the_index_is_saved_on_stage_and_unstage() {
    let repo = base();
    repo.write("a.txt", "versione in stage\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "versione nel worktree\n");
    let mut backend = repo.open();

    // Stage di `MM`: la versione in stage sparirebbe.
    let first = backend.stage(&paths(&["a.txt"])).unwrap().unwrap();
    assert_eq!(show(&repo, &first, "a.txt"), "versione in stage");
    assert_eq!(repo.git(&["show", ":a.txt"]), "versione nel worktree");

    repo.write("a.txt", "terza versione\n");
    let second = backend.unstage(&paths(&["a.txt"])).unwrap().unwrap();
    assert_ne!(first, second, "un backup non sovrascrive mai un altro");
    assert_eq!(show(&repo, &second, "a.txt"), "versione nel worktree");
    assert_eq!(show(&repo, &first, "a.txt"), "versione in stage");
    assert_eq!(backups(&repo).len(), 2);
}

#[test]
fn files_inside_a_requested_folder_count_too() {
    let repo = base();
    repo.write("dir/x.txt", "x in stage\n");
    repo.stage(&["dir/x.txt"]);
    repo.write("dir/x.txt", "x nel worktree\n");
    let mut backend = repo.open();
    let backup = backend.stage(&paths(&["dir"])).unwrap().unwrap();
    assert_eq!(show(&repo, &backup, "dir/x.txt"), "x in stage");
}

#[test]
fn nothing_at_risk_means_no_backup() {
    let repo = base();
    repo.write("a.txt", "modificato\n");
    repo.write("nuovo.txt", "nuovo\n");
    repo.write("staged.txt", "in stage e uguale nel worktree\n");
    repo.stage(&["staged.txt"]);
    let mut backend = repo.open();
    assert_eq!(
        backend.stage(&paths(&["a.txt", "nuovo.txt"])).unwrap(),
        None
    );
    // `A.`: la versione in stage è anche nel worktree.
    assert_eq!(
        backend.unstage(&paths(&["staged.txt", "a.txt"])).unwrap(),
        None
    );
    assert!(backups(&repo).is_empty());
}

#[test]
fn failed_backup_leaves_the_index_untouched() {
    let repo = base();
    repo.write("a.txt", "versione in stage\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "versione nel worktree\n");
    // Un file dove andrebbe la cartella dei ref di backup: `update-ref` non può crearlo.
    std::fs::write(repo.path().join(".git/refs/project"), "").unwrap();
    let mut backend = repo.open();
    let before = index_bytes(&repo);
    let status = backend.status().unwrap();

    for result in [
        backend.stage(&paths(&["a.txt"])),
        backend.unstage(&paths(&["a.txt"])),
    ] {
        let err = result.unwrap_err();
        assert!(
            matches!(&err, EngineError::Git { command, .. } if command == "update-ref"),
            "{err}"
        );
        assert_eq!(index_bytes(&repo), before, "l'indice non si tocca");
        assert_eq!(backend.status().unwrap(), status);
    }
    assert_eq!(repo.git(&["show", ":a.txt"]), "versione in stage");
    assert_no_temp_index(&repo);
}

#[test]
fn backup_works_with_conflicts_elsewhere_in_the_index() {
    let repo = repo_with_merge_conflict();
    repo.write("b.txt", "b in stage\n");
    repo.stage(&["b.txt"]);
    repo.write("b.txt", "b nel worktree\n");
    let mut backend = repo.open();
    // `git write-tree` sull'indice vero fallirebbe per il conflitto in `a.txt`.
    let backup = backend.stage(&paths(&["b.txt"])).unwrap().unwrap();
    assert_eq!(show(&repo, &backup, "b.txt"), "b in stage");
    assert_eq!(repo.git(&["show", ":b.txt"]), "b nel worktree");
}

#[test]
fn backup_on_a_branch_without_commits_has_no_parent() {
    let repo = Repo::new();
    repo.write("a.txt", "in stage\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "nel worktree\n");
    let mut backend = repo.open();
    let backup = backend.unstage(&paths(&["a.txt"])).unwrap().unwrap();
    assert_eq!(show(&repo, &backup, "a.txt"), "in stage");
    let parents = repo.git(&["log", "-1", "--format=%P", &backup]);
    assert_eq!(parents, "");
}

/// `a.txt` con una versione in stage e un'altra nel worktree, nascosta da `flag`
/// (`--assume-unchanged` o `--skip-worktree`): `git status` dà `M.`.
fn hidden_worktree_change(flag: &str) -> Repo {
    let repo = base();
    repo.write("a.txt", "versione in stage\n");
    repo.stage(&["a.txt"]);
    repo.git(&["update-index", flag, "a.txt"]);
    repo.write("a.txt", "versione nel worktree\n");
    repo
}

#[test]
fn assume_unchanged_and_skip_worktree_files_are_at_risk() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let repo = hidden_worktree_change(flag);
        let mut backend = repo.open();
        let status = backend.status().unwrap();
        assert_eq!(
            (
                status.entries[0].index.letter(),
                status.entries[0].worktree.letter()
            ),
            ('M', '.'),
            "{flag}: git non vede il worktree"
        );
        let backup = backend
            .unstage(&paths(&["a.txt"]))
            .unwrap()
            .unwrap_or_else(|| panic!("{flag}: nessun backup"));
        assert_eq!(show(&repo, &backup, "a.txt"), "versione in stage", "{flag}");
        assert_eq!(repo.git(&["show", ":a.txt"]), "in HEAD", "{flag}");
    }
}

#[test]
fn split_index_leaves_no_shared_index_behind() {
    let repo = base();
    repo.git(&["config", "core.splitIndex", "true"]);
    repo.git(&["update-index", "--split-index"]);
    let shared = || -> Vec<String> {
        std::fs::read_dir(repo.path().join(".git"))
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("sharedindex."))
            .collect()
    };
    repo.write("a.txt", "versione in stage\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "versione nel worktree\n");
    let before = shared();
    assert!(!before.is_empty(), "lo split index è attivo");
    let mut backend = repo.open();
    assert!(backend.unstage(&paths(&["a.txt"])).unwrap().is_some());
    let after = shared();
    // L'indice temporaneo non ha lasciato un suo `sharedindex.*`.
    let new: Vec<_> = after.iter().filter(|n| !before.contains(n)).collect();
    assert!(new.is_empty(), "{new:?}");
    assert_no_temp_index(&repo);
}
