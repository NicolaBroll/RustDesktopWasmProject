//! Test di integrazione di `CliBackend::stage`/`unstage` e dei comandi `index.stage` e
//! `index.unstage` su repo veri.

mod common;

use common::{Repo, repo_with_changes, repo_with_merge_conflict, repo_with_special_names};
use project_domain::{EntryKind, Operation, RepoPath, Status};
use project_engine::{DiffSpec, EngineError, GitBackend, Registry, Session};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// `(percorso, X, Y)` di ogni voce dello stato, come le lettere di `git status`.
fn entries(status: &Status) -> Vec<(String, char, char)> {
    status
        .entries
        .iter()
        .map(|e| (e.path.to_string(), e.index.letter(), e.worktree.letter()))
        .collect()
}

fn entry(path: &str, x: char, y: char) -> (String, char, char) {
    (path.to_owned(), x, y)
}

fn paths(list: &[&str]) -> Vec<RepoPath> {
    list.iter().map(|p| RepoPath::from(*p)).collect()
}

fn read(repo: &Repo, file: &str) -> Vec<u8> {
    std::fs::read(repo.path().join(file)).unwrap()
}

fn exists(repo: &Repo, file: &str) -> bool {
    repo.path().join(file).symlink_metadata().is_ok()
}

fn is_not_allowed(err: &EngineError) -> bool {
    matches!(err, EngineError::NotAllowed(_))
}

#[test]
fn stage_modification_new_file_and_folder() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", '.', 'M'),
            entry("nuovo.txt", 'A', '.'),
            entry("rinominato.txt", 'R', '.'),
            entry("cartella/non tracciato.txt", '?', '?'),
        ]
    );

    backend.stage(&paths(&["a.txt"])).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries[0].path, RepoPath::from("a.txt"));
    assert_eq!(
        (
            status.entries[0].index.letter(),
            status.entries[0].worktree.letter()
        ),
        ('M', '.')
    );
    // Le letture dopo la scrittura vedono il nuovo indice.
    assert_eq!(repo.git(&["show", ":a.txt"]), "modificato");
    let unstaged = backend.diff(&DiffSpec::Worktree { path: None }).unwrap();
    assert!(unstaged.files.is_empty(), "{unstaged:?}");

    // Una cartella comprende i file non tracciati che contiene.
    backend.stage(&paths(&["cartella"])).unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", 'M', '.'),
            entry("cartella/non tracciato.txt", 'A', '.'),
            entry("nuovo.txt", 'A', '.'),
            entry("rinominato.txt", 'R', '.'),
        ]
    );
    // Il worktree non cambia.
    assert_eq!(read(&repo, "a.txt"), b"modificato\n");
}

#[test]
fn stage_deletion_and_worktree_rename() {
    let repo = Repo::new();
    repo.commit_file("via.txt", "da eliminare\n", "primo");
    repo.commit_file(
        "vecchio.txt",
        "contenuto abbastanza lungo da riconoscere la rinomina\n",
        "secondo",
    );
    std::fs::remove_file(repo.path().join("via.txt")).unwrap();
    std::fs::rename(
        repo.path().join("vecchio.txt"),
        repo.path().join("nuovo.txt"),
    )
    .unwrap();
    let mut backend = repo.open();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("vecchio.txt", '.', 'D'),
            entry("via.txt", '.', 'D'),
            entry("nuovo.txt", '?', '?'),
        ]
    );

    backend.stage(&paths(&["via.txt"])).unwrap();
    backend
        .stage(&paths(&["vecchio.txt", "nuovo.txt"]))
        .unwrap();
    let status = backend.status().unwrap();
    assert_eq!(
        entries(&status),
        [entry("nuovo.txt", 'R', '.'), entry("via.txt", 'D', '.')]
    );
    assert!(matches!(
        &status.entries[0].kind,
        EntryKind::Renamed { original, .. } if *original == RepoPath::from("vecchio.txt")
    ));

    // Di nuovo gli stessi percorsi: le cancellazioni sono già in stage, niente da fare
    // (`git add` da solo direbbe "did not match any files").
    backend
        .stage(&paths(&["vecchio.txt", "nuovo.txt", "via.txt"]))
        .unwrap();
    assert_eq!(backend.status().unwrap(), status);
    assert!(!exists(&repo, "via.txt"));
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_staged_and_unstaged_byte_for_byte() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    let name = common::NON_UTF8_NAME;
    std::fs::write(repo.path().join(OsStr::from_bytes(name)), "latin\n").unwrap();
    let mut backend = repo.open();

    let path = RepoPath::from_bytes(name);
    backend.stage(std::slice::from_ref(&path)).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries.len(), 1);
    assert_eq!(status.entries[0].path, path);
    assert_eq!(status.entries[0].index.letter(), 'A');

    backend.unstage(std::slice::from_ref(&path)).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries[0].path, path);
    assert!(matches!(status.entries[0].kind, EntryKind::Untracked));
}

#[test]
fn glob_magic_and_dash_names_are_literal() {
    let repo = repo_with_special_names();
    let mut backend = repo.open();
    for name in ["file*.txt", "a[1].txt", ":(top)magia.txt", "-trattino.txt"] {
        backend.stage(&paths(&[name])).unwrap();
    }
    let mut staged: Vec<_> = entries(&backend.status().unwrap())
        .into_iter()
        .filter(|(_, x, _)| *x == 'M')
        .map(|(p, _, _)| p)
        .collect();
    staged.sort();
    // `file1.txt` e `a1.txt` restano fuori stage: `file*.txt` e `a[1].txt` non sono glob.
    assert_eq!(
        staged,
        ["-trattino.txt", ":(top)magia.txt", "a[1].txt", "file*.txt"]
    );

    backend
        .unstage(&paths(&["file*.txt", ":(top)magia.txt"]))
        .unwrap();
    let status = entries(&backend.status().unwrap());
    assert!(status.contains(&entry("file*.txt", '.', 'M')), "{status:?}");
    assert!(
        status.contains(&entry(":(top)magia.txt", '.', 'M')),
        "{status:?}"
    );
    assert!(
        status.contains(&entry("-trattino.txt", 'M', '.')),
        "{status:?}"
    );
    assert!(status.contains(&entry("file1.txt", '.', 'M')), "{status:?}");
}

#[test]
fn ignored_files_are_refused_and_nothing_is_staged() {
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignora i log");
    repo.write("ok.txt", "ok\n");
    repo.write("debug.log", "rumore\n");
    repo.write("dir/dentro.txt", "dentro\n");
    repo.write("dir/dentro.log", "rumore\n");
    let mut backend = repo.open();
    let before = backend.status().unwrap();

    let err = backend.stage(&paths(&["ok.txt", "debug.log"])).unwrap_err();
    assert!(is_not_allowed(&err), "{err}");
    assert!(err.to_string().contains("debug.log"), "{err}");
    // Neanche `ok.txt` è in stage: o tutto o niente.
    assert_eq!(backend.status().unwrap(), before);

    // In una cartella i file ignorati si saltano, come in git.
    backend.stage(&paths(&["dir"])).unwrap();
    let after = entries(&backend.status().unwrap());
    assert!(
        after.contains(&entry("dir/dentro.txt", 'A', '.')),
        "{after:?}"
    );
    assert!(
        repo.git(&["ls-files", "--", "dir"])
            .lines()
            .eq(["dir/dentro.txt"]),
        "il file ignorato non entra nell'indice"
    );
}

#[test]
fn unstage_never_touches_the_worktree() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "uno\n", "primo");
    repo.commit_file("via.txt", "da eliminare\n", "secondo");
    // `a.txt`: una modifica in stage e un'altra fuori; `nuovo.txt` aggiunto; `via.txt`
    // cancellato in stage.
    repo.write("a.txt", "in stage\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "fuori stage\n");
    repo.write("nuovo.txt", "nuovo\n");
    repo.stage(&["nuovo.txt"]);
    repo.git(&["rm", "--quiet", "via.txt"]);
    let mut backend = repo.open();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", 'M', 'M'),
            entry("nuovo.txt", 'A', '.'),
            entry("via.txt", 'D', '.'),
        ]
    );

    backend
        .unstage(&paths(&["a.txt", "nuovo.txt", "via.txt"]))
        .unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [
            entry("a.txt", '.', 'M'),
            entry("via.txt", '.', 'D'),
            entry("nuovo.txt", '?', '?'),
        ]
    );
    // Il worktree è com'era: niente ripristinato, niente perso.
    assert_eq!(read(&repo, "a.txt"), b"fuori stage\n");
    assert_eq!(read(&repo, "nuovo.txt"), b"nuovo\n");
    assert!(!exists(&repo, "via.txt"), "unstage non ripristina i file");
    // L'indice è tornato come in HEAD.
    assert_eq!(repo.git(&["show", ":a.txt"]), "uno");
    assert!(
        backend
            .diff(&DiffSpec::Index { path: None })
            .unwrap()
            .files
            .is_empty()
    );
}

#[test]
fn unstage_one_side_of_a_staged_rename() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    backend.unstage(&paths(&["rinominato.txt"])).unwrap();
    let status = entries(&backend.status().unwrap());
    assert!(
        status.contains(&entry("vecchio.txt", 'D', '.')),
        "{status:?}"
    );
    assert!(
        status.contains(&entry("rinominato.txt", '?', '?')),
        "{status:?}"
    );
    assert!(exists(&repo, "rinominato.txt"));
}

#[test]
fn unstage_without_commits_leaves_files_untracked() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    let mut backend = repo.open();
    backend.stage(&paths(&["a.txt", "b.txt"])).unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [entry("a.txt", 'A', '.'), entry("b.txt", 'A', '.')]
    );

    backend.unstage(&paths(&["a.txt"])).unwrap();
    assert_eq!(
        entries(&backend.status().unwrap()),
        [entry("b.txt", 'A', '.'), entry("a.txt", '?', '?')]
    );
    assert_eq!(read(&repo, "a.txt"), b"a\n");
    assert_eq!(read(&repo, "b.txt"), b"b\n");
}

#[test]
fn conflicts_resolve_with_stage_and_refuse_unstage() {
    let repo = repo_with_merge_conflict();
    let mut backend = repo.open();
    let before = backend.status().unwrap();
    assert!(matches!(before.entries[0].kind, EntryKind::Unmerged(_)));
    let content = read(&repo, "a.txt");

    // Unstage toglierebbe gli stadi del conflitto: rifiutato, niente cambia.
    let err = backend.unstage(&paths(&["a.txt"])).unwrap_err();
    assert!(is_not_allowed(&err), "{err}");
    assert!(err.to_string().contains("a.txt"), "{err}");
    assert_eq!(backend.status().unwrap(), before);

    // Stage = risoluzione con il contenuto del worktree (qui con i marcatori di conflitto).
    backend.stage(&paths(&["a.txt"])).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(entries(&status), [entry("a.txt", 'M', '.')]);
    assert_eq!(
        status.operation,
        Some(Operation::Merge),
        "il merge resta in corso"
    );
    assert_eq!(read(&repo, "a.txt"), content);
}

#[test]
fn locked_index_is_an_error_and_the_lock_stays() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    let before = backend.status().unwrap();
    let lock = repo.path().join(".git/index.lock");
    std::fs::write(&lock, "").unwrap();

    for result in [
        backend.stage(&paths(&["a.txt"])),
        backend.unstage(&paths(&["nuovo.txt"])),
    ] {
        let err = result.unwrap_err();
        let EngineError::IndexLocked { lock: reported } = &err else {
            panic!("atteso IndexLocked: {err}");
        };
        assert_eq!(Path::new(reported), lock);
        assert!(lock.exists(), "il lock non si cancella mai");
    }
    std::fs::remove_file(&lock).unwrap();
    assert_eq!(backend.status().unwrap(), before);
}

#[test]
fn missing_paths_fail_and_change_nothing() {
    let repo = repo_with_changes();
    let mut backend = repo.open();
    let before = backend.status().unwrap();

    let err = backend
        .stage(&paths(&["a.txt", "nessuno.txt"]))
        .unwrap_err();
    let EngineError::Git { command, stderr } = &err else {
        panic!("atteso un errore di git: {err}");
    };
    assert_eq!(command, "add");
    assert!(stderr.contains("nessuno.txt"), "{stderr}");
    assert_eq!(backend.status().unwrap(), before, "`a.txt` non è in stage");

    let err = backend
        .unstage(&paths(&["nuovo.txt", "nessuno.txt"]))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::Git { command, .. } if command == "restore"),
        "{err}"
    );
    assert_eq!(backend.status().unwrap(), before);

    // Un file non tracciato non è in stage: per git è sconosciuto.
    let err = backend
        .unstage(&paths(&["cartella/non tracciato.txt"]))
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");

    // Nessun percorso: niente da fare, non "tutto".
    backend.stage(&[]).unwrap();
    backend.unstage(&[]).unwrap();
    assert_eq!(backend.status().unwrap(), before);
}

#[test]
fn many_paths_go_through_stdin() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    // Nomi lunghi: insieme superano di molto il limite di un singolo argomento (128 KiB).
    let names: Vec<String> = (0..1500)
        .map(|i| format!("cartella/{}-{i:04}.txt", "x".repeat(150)))
        .collect();
    for name in &names {
        repo.write(name, "?\n");
    }
    let list: Vec<RepoPath> = names.iter().map(|n| RepoPath::from(n.as_str())).collect();
    let mut backend = repo.open();
    backend.stage(&list).unwrap();
    let status = backend.status().unwrap();
    assert_eq!(status.entries.len(), names.len());
    assert!(status.entries.iter().all(|e| e.index.letter() == 'A'));
    backend.unstage(&list).unwrap();
    let status = backend.status().unwrap();
    assert!(
        status
            .entries
            .iter()
            .all(|e| matches!(e.kind, EntryKind::Untracked))
    );
}

fn run(registry: &Registry, session: &mut Session, id: &str, params: Value) -> Value {
    registry
        .run(id, session, params)
        .unwrap_or_else(|e| panic!("{id}: {e}"))
}

#[test]
fn index_commands_on_a_real_repo() {
    let repo = repo_with_changes();
    let registry = Registry::builtin();
    let mut session = Session::new(Arc::new(Repo::opener()));
    let err = registry
        .run("index.stage", &mut session, json!({ "paths": ["a.txt"] }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NoRepo), "{err}");
    run(
        &registry,
        &mut session,
        "repo.open",
        json!({ "path": repo.path_str() }),
    );

    // Il risultato è il nuovo stato, come `repo.status`.
    let out = run(
        &registry,
        &mut session,
        "index.stage",
        json!({ "paths": ["a.txt", "cartella/non tracciato.txt"] }),
    );
    assert_eq!(out, run(&registry, &mut session, "repo.status", json!({})));
    let status: Status = serde_json::from_value(out).unwrap();
    assert!(entries(&status).contains(&entry("a.txt", 'M', '.')));

    let out = run(
        &registry,
        &mut session,
        "index.unstage",
        json!({ "paths": ["a.txt"] }),
    );
    assert_eq!(out["entries"][0]["path"], "a.txt");
    assert_eq!(out["entries"][0]["index"], "Unmodified");
    assert_eq!(out["entries"][0]["worktree"], "Modified");
    let diff = run(
        &registry,
        &mut session,
        "diff.worktree",
        json!({ "path": "a.txt" }),
    );
    assert_eq!(diff["files"][0]["change"], "Modified");

    for params in [
        json!({}),
        json!({ "paths": [] }),
        json!({ "paths": "a.txt" }),
        json!({ "path": ["a.txt"] }),
        json!({ "paths": ["a.txt"], "confirm": true }),
        json!({ "paths": [""] }),
        json!({ "paths": ["/etc/passwd"] }),
        json!({ "paths": ["../fuori"] }),
        json!({ "paths": ["a/../b"] }),
        json!({ "paths": ["./a.txt"] }),
        json!({ "paths": ["."] }),
        json!({ "paths": ["cartella/"] }),
        json!({ "paths": ["a.txt", [97, 0, 98]] }),
    ] {
        for id in ["index.stage", "index.unstage"] {
            let err = registry.run(id, &mut session, params.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{id} {params}: {err}"
            );
        }
    }
}
