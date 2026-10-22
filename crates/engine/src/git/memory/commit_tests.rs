//! Test di `commit` sul `MemoryBackend`.

use super::*;
use crate::{DiffSpec, LogQuery};
use project_domain::Head;

fn p(path: &str) -> RepoPath {
    RepoPath::from(path)
}

/// `(percorso, X, Y)` di ogni voce dello stato.
fn entries(repo: &MemoryBackend) -> Vec<(String, char, char)> {
    repo.status()
        .unwrap()
        .entries
        .iter()
        .map(|e| (e.path.to_string(), e.index.letter(), e.worktree.letter()))
        .collect()
}

fn e(path: &str, x: char, y: char) -> (String, char, char) {
    (path.to_owned(), x, y)
}

fn sample_head() -> Oid {
    MemoryBackend::sample().head().unwrap().target().unwrap()
}

fn changed(repo: &MemoryBackend, rev: &str) -> Vec<(String, Change)> {
    repo.show_commit(rev)
        .unwrap()
        .files
        .iter()
        .map(|f| (f.path.to_string(), f.change))
        .collect()
}

#[test]
fn commit_of_the_staged_changes() {
    let mut repo = MemoryBackend::sample();
    let before = sample_head();
    let out = repo
        .commit(&CommitRequest::new(
            "\nAggiorna README  \ne guida\n\n\nCorpo è UTF-8\n# resta\n\n",
        ))
        .unwrap();
    assert_eq!(out.previous, Some(before));
    assert_eq!(out.branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(out.backup, None);
    assert_eq!(out.commit.parents, [before]);
    assert_eq!(out.commit.summary, "Aggiorna README e guida");
    assert_eq!(out.commit.body, "Corpo è UTF-8\n# resta");
    assert_eq!(out.commit.author.name, "Ada Rossi");
    assert_eq!(out.commit.author, out.commit.committer);
    assert!(out.commit.committer.time > repo.commits[&before].committer.time);

    // HEAD, log, stato e diff vedono il nuovo commit.
    assert_eq!(repo.head().unwrap().target(), Some(out.commit.id));
    assert_eq!(
        repo.log(&LogQuery::page(0, 1)).unwrap(),
        std::slice::from_ref(&out.commit)
    );
    assert_eq!(
        changed(&repo, "HEAD"),
        [
            ("README.md".to_owned(), Change::Modified),
            ("docs/guida.md".to_owned(), Change::Added),
        ]
    );
    assert_eq!(
        entries(&repo),
        [
            e("README.md", '.', 'M'),
            e("src/lib.rs", '.', 'M'),
            e("note.txt", '?', '?'),
        ]
    );
    assert!(
        repo.diff(&DiffSpec::Index { path: None })
            .unwrap()
            .files
            .is_empty()
    );
    // Fuori stage resta solo la modifica non in stage di README.md.
    let readme = repo
        .diff(&DiffSpec::Worktree {
            path: Some(p("README.md")),
        })
        .unwrap();
    assert_eq!(readme.files.len(), 1);
    assert_eq!(
        repo.diff(&DiffSpec::Commit {
            rev: "HEAD".to_owned(),
            path: Some(p("docs/guida.md")),
        })
        .unwrap()
        .files[0]
            .change,
        Change::Added
    );
    // `main` ora è avanti di 2 rispetto a `origin/main`.
    let upstream = repo.status().unwrap().upstream.unwrap();
    assert_eq!(upstream.ahead, Some(2));

    // Di nuovo: niente da committare, e niente cambia.
    let snapshot = repo.clone();
    let err = repo.commit(&CommitRequest::new("ancora")).unwrap_err();
    assert!(matches!(err, EngineError::NothingToCommit), "{err}");
    assert_eq!(repo, snapshot);
}

#[test]
fn invalid_messages_change_nothing() {
    let mut repo = MemoryBackend::sample();
    for message in ["", " \n\n\t", "a\0b"] {
        let err = repo.commit(&CommitRequest::new(message)).unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }
    assert_eq!(repo, MemoryBackend::sample());
}

#[test]
fn amend_rewrites_the_last_commit() {
    let mut repo = MemoryBackend::sample();
    let first = repo.commit(&CommitRequest::new("primo")).unwrap().commit;

    // Solo il messaggio: stessi genitori, autore e file; il vecchio commit nel backup.
    let out = repo
        .commit(&CommitRequest::new("messaggio corretto").amend())
        .unwrap();
    assert_eq!(out.previous, Some(first.id));
    assert_eq!(out.backup.as_deref(), Some("refs/project/backup/amend-1"));
    assert_eq!(
        repo.commit_backups(),
        [("refs/project/backup/amend-1".to_owned(), first.id)]
    );
    assert_ne!(out.commit.id, first.id);
    assert_eq!(out.commit.parents, first.parents);
    assert_eq!(out.commit.author, first.author);
    assert_eq!(out.commit.summary, "messaggio corretto");
    assert_eq!(
        changed(&repo, "HEAD"),
        changed(&repo, &first.id.to_string())
    );
    let log = repo.log(&LogQuery::page(0, 2)).unwrap();
    assert_eq!(log[0].id, out.commit.id);
    assert_eq!(log[1].id, sample_head());

    // Con un file nuovo in stage.
    repo.stage(&[p("note.txt")]).unwrap();
    let out = repo
        .commit(&CommitRequest::new("con gli appunti").amend())
        .unwrap();
    assert_eq!(out.backup.as_deref(), Some("refs/project/backup/amend-2"));
    assert_eq!(
        changed(&repo, "HEAD"),
        [
            ("README.md".to_owned(), Change::Modified),
            ("docs/guida.md".to_owned(), Change::Added),
            ("note.txt".to_owned(), Change::Added),
        ]
    );
    assert_eq!(
        entries(&repo),
        [e("README.md", '.', 'M'), e("src/lib.rs", '.', 'M')]
    );
}

#[test]
fn amend_of_a_published_commit_needs_confirm() {
    let mut repo = MemoryBackend::sample();
    // HEAD è anche su `origin/main`.
    repo.set_remote_branch("origin/main", sample_head());
    let snapshot = repo.clone();
    let err = repo
        .commit(&CommitRequest::new("riscrivo").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(
        err.to_string().contains("refs/remotes/origin/main"),
        "{err}"
    );
    assert_eq!(repo, snapshot);

    let out = repo
        .commit(&CommitRequest::new("riscrivo").amend().confirmed())
        .unwrap();
    assert_eq!(out.previous, Some(sample_head()));
    assert!(out.backup.is_some());
    // Un commit nuovo non chiede conferma, anche se HEAD è pubblicato.
    let mut repo = snapshot;
    repo.commit(&CommitRequest::new("nuovo")).unwrap();
}

#[test]
fn amend_that_would_empty_the_commit_is_refused() {
    // HEAD è un commit vuoto (nessun file cambiato) e niente è in stage.
    let mut repo = MemoryBackend::sample();
    let mut empty = repo.log(&LogQuery::page(0, 1)).unwrap().remove(0);
    empty.parents = vec![empty.id];
    empty.id = MemoryBackend::fake_oid(99);
    empty.summary = "vuoto".to_owned();
    repo.insert_commit(empty.clone());
    repo.set_branch("main", empty.id);
    repo.set_entries(Vec::new());
    let snapshot = repo.clone();
    let err = repo
        .commit(&CommitRequest::new("ancora vuoto").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo, snapshot);
}

/// Repo vuoto con un file non tracciato e l'identità.
fn empty_repo() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Eva", "eva@example.com");
    repo.set_worktree_file("a.txt", "ciao\n");
    repo.set_entries(vec![StatusEntry {
        path: p("a.txt"),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    }]);
    repo
}

#[test]
fn first_commit() {
    let mut repo = empty_repo();
    let err = repo.commit(&CommitRequest::new("primo")).unwrap_err();
    assert!(matches!(err, EngineError::NothingToCommit), "{err}");
    let err = repo
        .commit(&CommitRequest::new("primo").amend())
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    repo.stage(&[p("a.txt")]).unwrap();
    let out = repo.commit(&CommitRequest::new("primo")).unwrap();
    assert_eq!(out.previous, None);
    assert_eq!(out.branch.as_deref(), Some("refs/heads/main"));
    assert!(out.commit.parents.is_empty());
    assert_eq!(out.commit.committer.name, "Eva");
    assert_eq!(out.commit.committer.time, START);
    assert_eq!(
        repo.head().unwrap(),
        Head::Branch {
            name: "main".to_owned(),
            target: Some(out.commit.id)
        }
    );
    assert!(entries(&repo).is_empty());
    assert_eq!(
        changed(&repo, "main"),
        [("a.txt".to_owned(), Change::Added)]
    );
}

#[test]
fn missing_identity_is_an_error() {
    let mut repo = empty_repo();
    repo.identity = None;
    repo.stage(&[p("a.txt")]).unwrap();
    let snapshot = repo.clone();
    let err = repo.commit(&CommitRequest::new("primo")).unwrap_err();
    assert!(err.to_string().contains("identity unknown"), "{err}");
    assert_eq!(repo, snapshot);
}

#[test]
fn detached_head_moves_only_head() {
    let mut repo = MemoryBackend::sample();
    let feature = repo.resolve("feature/grafo").unwrap();
    repo.detach_head(feature);
    repo.stage(&[p("note.txt")]).unwrap();
    let out = repo.commit(&CommitRequest::new("staccato")).unwrap();
    assert_eq!(out.branch, None);
    assert_eq!(out.previous, Some(feature));
    assert_eq!(repo.head().unwrap(), Head::Detached(out.commit.id));
    assert_eq!(repo.resolve("feature/grafo").unwrap(), feature);
    assert_eq!(repo.resolve("main").unwrap(), sample_head());
}

#[test]
fn operations_in_progress() {
    for op in [
        Operation::Rebase,
        Operation::CherryPick,
        Operation::Revert,
        Operation::Merge,
    ] {
        let mut repo = MemoryBackend::sample();
        repo.set_operation(Some(op));
        let snapshot = repo.clone();
        let err = repo.commit(&CommitRequest::new("x")).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{op:?}: {err}");
        assert_eq!(repo, snapshot);
    }
}
