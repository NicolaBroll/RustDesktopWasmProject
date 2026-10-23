//! Test di integrazione di `worktree.discard` (§7) con il `CliBackend` su repo veri: conferma
//! obbligatoria, backup con `git stash create` e ref che resiste a `git gc`, undo che rimette
//! i file identici byte per byte, redo, rifiuti (non tracciati, conflitti, `intent-to-add`,
//! branch senza commit, file cambiati dopo lo scarto), nomi che sembrano opzioni o pathspec.

mod common;

use common::{Repo, repo_with_merge_conflict, repo_with_special_names};
use project_domain::{Change, RepoPath};
use project_engine::{EngineError, GitBackend, Registry, Session, WorktreeRestore};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// Quello che uno scarto non deve toccare o che l'undo deve rimettere: stato, indice, ref e
/// i byte dei file indicati (`None` = il file non c'è).
fn snapshot(repo: &Repo, files: &[&str]) -> (String, String, String, Vec<Option<Vec<u8>>>) {
    (
        repo.git(&["status", "--porcelain=v2", "--untracked-files=all"]),
        repo.git(&["ls-files", "-s"]),
        repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        files
            .iter()
            .map(|f| std::fs::read(repo.path().join(f)).ok())
            .collect(),
    )
}

/// Lo scarto come lo fa `worktree.discard`: il piano letto prima e poi il backend.
fn planned(
    backend: &mut project_git::CliBackend,
    paths: &[RepoPath],
) -> Result<project_engine::DiscardOutcome, EngineError> {
    let plan = project_engine::discard_plan(&backend.status()?, paths)?;
    backend.discard(paths, &plan)
}

fn bytes(repo: &Repo, file: &str) -> Option<Vec<u8>> {
    std::fs::read(repo.path().join(file)).ok()
}

/// Un repo con un commit e una modifica fuori stage di ogni tipo:
///
/// - `a.txt`: byte qualunque (CRLF, nessun a capo finale, un byte non UTF-8);
/// - `-trattino.txt`: eliminato nel worktree;
/// - `m.txt`: cambiato in stage e poi ancora fuori (`MM`);
/// - `d/x.txt` modificato e `d/nuovo.txt` non tracciato nella cartella `d`;
/// - `script.sh` reso eseguibile (solo su Unix).
fn repo_with_unstaged_changes() -> Repo {
    let repo = Repo::new();
    for f in ["a.txt", "-trattino.txt", "m.txt", "d/x.txt", "script.sh"] {
        repo.write(f, &format!("{f} prima\n"));
    }
    repo.git(&["add", "-A"]);
    repo.commit("primo");
    std::fs::write(repo.path().join("a.txt"), b"riga\r\nbyte \xff\r\nfine").unwrap();
    std::fs::remove_file(repo.path().join("-trattino.txt")).unwrap();
    repo.write("m.txt", "m in stage\n");
    repo.stage(&["m.txt"]);
    repo.write("m.txt", "m in stage e poi fuori\n");
    repo.write("d/x.txt", "x dopo\n");
    repo.write("d/nuovo.txt", "non tracciato\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = repo.path().join("script.sh");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    repo
}

const FILES: &[&str] = &[
    "a.txt",
    "-trattino.txt",
    "m.txt",
    "d/x.txt",
    "d/nuovo.txt",
    "script.sh",
];

#[test]
fn discard_then_undo_restores_the_files_byte_for_byte() {
    let repo = repo_with_unstaged_changes();
    let before = snapshot(&repo, FILES);
    let index_before = repo.git(&["ls-files", "-s"]);
    let a_before = bytes(&repo, "a.txt").unwrap();
    let (r, mut s) = open(&repo);

    let out = run(
        &r,
        &mut s,
        "worktree.discard",
        json!({
            "paths": ["a.txt", "-trattino.txt", "m.txt", "d", "script.sh"],
            "confirm": true
        }),
    );
    let paths: Vec<&str> = out["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    let mut expected = vec!["-trattino.txt", "a.txt", "d/x.txt", "m.txt"];
    if cfg!(unix) {
        expected.push("script.sh");
    }
    assert_eq!(paths, expected);
    assert_eq!(out["files"][0]["change"], "Deleted");
    let backup = out["backup"].as_str().unwrap();
    assert!(
        backup.starts_with("refs/project/backup/discard-"),
        "{backup}"
    );
    let stash = out["stash"].as_str().unwrap();
    assert_eq!(repo.rev(backup).to_string(), stash);
    // Il backup ha i byte del worktree di prima; `refs/stash` non è stato creato.
    let shown = repo.git_output(&["cat-file", "blob", &format!("{stash}:a.txt")]);
    assert_eq!(shown.stdout, a_before);
    assert!(
        repo.git_output(&["rev-parse", "-q", "--verify", "refs/stash"])
            .stdout
            .is_empty()
    );

    // Worktree come l'indice, indice invariato, il non tracciato resta.
    assert_eq!(repo.git(&["ls-files", "-s"]), index_before);
    assert_eq!(repo.git(&["diff", "--name-only"]), "");
    assert_eq!(
        repo.git(&["status", "--porcelain", "--untracked-files=all"]),
        "M  m.txt\n?? d/nuovo.txt"
    );
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"a.txt prima\n");
    assert_eq!(
        bytes(&repo, "-trattino.txt").unwrap(),
        b"-trattino.txt prima\n"
    );
    assert_eq!(bytes(&repo, "m.txt").unwrap(), b"m in stage\n");
    assert_eq!(out["status"]["entries"].as_array().unwrap().len(), 2);
    assert_eq!(out["next_undo"].as_str().unwrap(), {
        let n = expected.len();
        format!("Scarta le modifiche di {n} file («-trattino.txt», «a.txt», «d/x.txt», ...)")
    });

    // Il backup resiste al garbage collector.
    repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    repo.git(&["gc", "--quiet", "--prune=now"]);
    repo.git(&["cat-file", "-e", stash]);

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "worktree.discard");
    assert_eq!(undo["warnings"], json!([]));
    // Tutto come prima, byte per byte (e il modo di `script.sh`); in più resta il backup.
    let after = snapshot(&repo, FILES);
    assert_eq!(
        (&after.0, &after.1, &after.3),
        (&before.0, &before.1, &before.3)
    );
    assert_eq!(after.2, format!("{}\n{backup} {stash}", before.2));

    // Redo: scartato di nuovo, nessun backup nuovo; poi un altro undo.
    let refs = repo.git(&["for-each-ref", "refs/project/backup/"]);
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["warnings"], json!([]));
    assert_eq!(repo.git(&["diff", "--name-only"]), "");
    assert_eq!(repo.git(&["for-each-ref", "refs/project/backup/"]), refs);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(snapshot(&repo, FILES), after);
}

#[test]
fn without_confirm_nothing_changes() {
    let repo = repo_with_unstaged_changes();
    let before = snapshot(&repo, FILES);
    let (r, mut s) = open(&repo);
    for params in [
        json!({ "paths": ["a.txt", "-trattino.txt"] }),
        json!({ "paths": ["d"], "confirm": false }),
    ] {
        let err = r.run("worktree.discard", &mut s, params).unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::ConfirmRequired {
                    command: "worktree.discard",
                    ..
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("\"confirm\": true"), "{err}");
    }
    let err = r
        .run("worktree.discard", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert_eq!(snapshot(&repo, FILES), before);
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), "");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"], json!([]));
}

#[test]
fn names_are_taken_literally() {
    // `file*.txt` non comprende `file1.txt`, `:(top)magia.txt` non è magico, `-trattino.txt`
    // non è un'opzione.
    let repo = repo_with_special_names();
    let mut backend = repo.open();
    let names = ["file*.txt", ":(top)magia.txt", "-trattino.txt", "a[1].txt"];
    let paths: Vec<RepoPath> = names.iter().map(|n| RepoPath::from(*n)).collect();
    let others_before = snapshot(&repo, &["file1.txt", "a1.txt"]).3;
    let all_before = snapshot(&repo, &names).3;
    let out = planned(&mut backend, &paths).unwrap();
    let mut discarded: Vec<String> = out.files.iter().map(|f| f.path.to_string()).collect();
    discarded.sort();
    let mut expected: Vec<String> = names.iter().map(|n| (*n).to_owned()).collect();
    expected.sort();
    assert_eq!(discarded, expected);
    for name in names {
        assert_eq!(bytes(&repo, name).unwrap(), b"prima\n", "{name}");
    }
    assert_eq!(snapshot(&repo, &["file1.txt", "a1.txt"]).3, others_before);
    assert_eq!(
        repo.git(&["status", "--porcelain"]),
        " M a1.txt\n M file1.txt"
    );

    // Undo diretto sul backend.
    let request = WorktreeRestore {
        command: "edit.undo",
        redo: false,
        stash: out.stash,
        files: out.files.clone(),
        raw: out.raw,
    };
    let outcome = backend.restore_worktree(&request).unwrap();
    assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    assert_eq!(snapshot(&repo, &names).3, all_before);
    // Redo diretto.
    let redo = WorktreeRestore {
        command: "edit.redo",
        redo: true,
        ..request
    };
    backend.restore_worktree(&redo).unwrap();
    for name in names {
        assert_eq!(bytes(&repo, name).unwrap(), b"prima\n", "{name}");
    }
}

#[test]
fn undo_refused_if_a_file_changed_after_the_discard() {
    let repo = repo_with_unstaged_changes();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["a.txt", "m.txt"], "confirm": true }),
    );
    repo.write("a.txt", "lavoro nuovo\n");
    let before = snapshot(&repo, FILES);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let EngineError::UndoConflict { command, refs } = &err else {
        panic!("{err}");
    };
    assert_eq!(*command, "edit.undo");
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert!(refs[0].contains("a.txt"), "{refs:?}");
    assert_eq!(snapshot(&repo, FILES), before, "niente cambiato");
    assert!(run(&r, &mut s, "edit.history", json!({}))["can_undo"] == true);

    // Anche un cambio dell'indice blocca l'undo.
    repo.write("a.txt", "a.txt prima\n");
    repo.stage(&["m.txt"]);
    repo.write("m.txt", "m in stage\n");
    repo.stage(&["m.txt"]);
    let index = repo.git(&["ls-files", "-s", "m.txt"]);
    repo.git(&["reset", "--quiet", "--", "m.txt"]);
    assert_ne!(repo.git(&["ls-files", "-s", "m.txt"]), index);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs.iter().any(|r| r.contains("m.txt") && r.contains("stage"))),
        "{err}"
    );
}

#[test]
fn redo_refused_if_a_file_changed_after_the_undo() {
    let repo = repo_with_unstaged_changes();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["d/x.txt"], "confirm": true }),
    );
    run(&r, &mut s, "edit.undo", json!({}));
    repo.write("d/x.txt", "ancora diverso\n");
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { command: "edit.redo", refs } if refs[0].contains("d/x.txt")),
        "{err}"
    );
    assert_eq!(bytes(&repo, "d/x.txt").unwrap(), b"ancora diverso\n");
}

#[test]
fn refusals_change_nothing() {
    let repo = repo_with_unstaged_changes();
    let before = snapshot(&repo, FILES);
    let mut backend = repo.open();
    let p = |s: &str| RepoPath::from(s);
    // Un non tracciato chiesto per nome; niente da scartare; un percorso che non c'è.
    for paths in [
        vec![p("d/nuovo.txt"), p("a.txt")],
        vec![p("non-esiste")],
        vec![],
    ] {
        let err = planned(&mut backend, &paths).unwrap_err();
        assert!(
            matches!(err, EngineError::NotAllowed(_)),
            "{paths:?}: {err}"
        );
    }
    // Un file `intent-to-add` nel repo: `git stash create` non salverebbe il backup.
    repo.write("ita.txt", "x\n");
    repo.git(&["add", "-N", "ita.txt"]);
    let err = planned(&mut backend, &[p("a.txt")]).unwrap_err();
    assert!(err.to_string().contains("intent-to-add"), "{err}");
    repo.git(&["rm", "--cached", "--quiet", "ita.txt"]);
    std::fs::remove_file(repo.path().join("ita.txt")).unwrap();
    assert_eq!(snapshot(&repo, FILES), before);
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), "");

    // Indice bloccato: niente backup, niente scartato.
    std::fs::write(repo.path().join(".git/index.lock"), "").unwrap();
    let err = planned(&mut backend, &[p("a.txt")]).unwrap_err();
    assert!(matches!(err, EngineError::IndexLocked { .. }), "{err}");
    std::fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
    assert_eq!(snapshot(&repo, FILES), before);
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), "");
}

#[test]
fn refused_with_conflicts_and_without_commits() {
    let repo = repo_with_merge_conflict();
    repo.write("a.txt", "risolto a metà\n");
    let mut backend = repo.open();
    let err = planned(&mut backend, &[RepoPath::from("a.txt")]).unwrap_err();
    assert!(err.to_string().contains("conflitto"), "{err}");

    let empty = Repo::new();
    empty.write("a.txt", "x\n");
    empty.stage(&["a.txt"]);
    empty.write("a.txt", "y\n");
    let err = planned(&mut empty.open(), &[RepoPath::from("a.txt")]).unwrap_err();
    assert!(err.to_string().contains("non ha ancora commit"), "{err}");
    assert_eq!(bytes(&empty, "a.txt").unwrap(), b"y\n");
}

#[test]
fn backend_reports_the_change_of_each_file() {
    let repo = repo_with_unstaged_changes();
    let out = planned(
        &mut repo.open(),
        &[RepoPath::from("-trattino.txt"), RepoPath::from("m.txt")],
    )
    .unwrap();
    let changes: Vec<Change> = out.files.iter().map(|f| f.change).collect();
    assert_eq!(changes, [Change::Deleted, Change::Modified]);
    // La voce dell'indice registrata è quella vera.
    let index = repo.git(&["ls-files", "-s", "m.txt"]);
    assert!(index.contains(&out.files[1].oid.to_string()), "{index}");
}

#[test]
fn untracked_file_in_place_of_a_folder_is_never_deleted() {
    let repo = Repo::new();
    repo.commit_file("top/d/x", "x\n", "primo");
    // La cartella `top/d` cancellata e al suo posto un file non tracciato.
    std::fs::remove_dir_all(repo.path().join("top/d")).unwrap();
    repo.write("top/d", "lavoro non tracciato\n");
    let (r, mut s) = open(&repo);
    for paths in [json!(["top"]), json!(["top/d/x"])] {
        let err = r
            .run(
                "worktree.discard",
                &mut s,
                json!({ "paths": paths, "confirm": true }),
            )
            .unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.contains("`top/d`")),
            "{err}"
        );
    }
    assert_eq!(bytes(&repo, "top/d").unwrap(), b"lavoro non tracciato\n");
    assert_eq!(repo.git(&["for-each-ref", "refs/project/"]), "");

    // Ignorato (`git status` non lo mostra): lo ferma il controllo sul file system.
    std::fs::write(repo.path().join(".git/info/exclude"), "top/d\n").unwrap();
    let err = r
        .run(
            "worktree.discard",
            &mut s,
            json!({ "paths": ["top"], "confirm": true }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`top/d`")),
        "{err}"
    );
    assert_eq!(bytes(&repo, "top/d").unwrap(), b"lavoro non tracciato\n");
}

#[test]
fn redo_never_deletes_a_file_in_place_of_a_folder() {
    let repo = Repo::new();
    repo.commit_file("top/d/x", "x\n", "primo");
    repo.write("top/d/x", "modificato\n");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["top"], "confirm": true }),
    );
    run(&r, &mut s, "edit.undo", json!({}));
    std::fs::remove_dir_all(repo.path().join("top/d")).unwrap();
    repo.write("top/d", "nuovo\n");
    std::fs::write(repo.path().join(".git/info/exclude"), "top/d\n").unwrap();
    assert!(r.run("edit.redo", &mut s, json!({})).is_err());
    assert_eq!(bytes(&repo, "top/d").unwrap(), b"nuovo\n");
}

#[test]
fn crlf_with_autocrlf_input_comes_back_identical() {
    let repo = Repo::new();
    repo.git(&["config", "core.autocrlf", "input"]);
    repo.commit_file("a.txt", "uno\n", "primo");
    std::fs::write(repo.path().join("a.txt"), b"uno\r\ndue\r\n").unwrap();
    let before = bytes(&repo, "a.txt").unwrap();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["a.txt"], "confirm": true }),
    );
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"uno\n");
    // Il ref di backup tiene i byte grezzi (e, come genitore, il commit di `stash create`).
    let backup = out["backup"].as_str().unwrap();
    let raw = repo.git_output(&["cat-file", "blob", &format!("{backup}:a.txt")]);
    assert_eq!(raw.stdout, before);
    assert_eq!(repo.rev(&format!("{backup}^")).to_string(), out["stash"]);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert!(history["done"][0]["worktree"]["raw"].is_string());

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["warnings"], json!([]));
    assert_eq!(bytes(&repo, "a.txt").unwrap(), before);
    // Il redo vede che il file è come l'ha lasciato l'undo.
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(bytes(&repo, "a.txt").unwrap(), b"uno\n");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(bytes(&repo, "a.txt").unwrap(), before);
    // Cambiare solo i fine riga dopo l'undo blocca il redo.
    std::fs::write(repo.path().join("a.txt"), b"uno\ndue\r\n").unwrap();
    assert!(matches!(
        r.run("edit.redo", &mut s, json!({})),
        Err(EngineError::UndoConflict { .. })
    ));
}

#[test]
fn undo_refused_if_head_moved() {
    let repo = repo_with_unstaged_changes();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["a.txt"], "confirm": true }),
    );
    repo.git(&["checkout", "--quiet", "-b", "altro"]);
    let before = snapshot(&repo, FILES);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].starts_with("HEAD")),
        "{err}"
    );
    assert_eq!(snapshot(&repo, FILES), before);
    repo.git(&["checkout", "--quiet", "main"]);
    run(&r, &mut s, "edit.undo", json!({}));
}

#[test]
fn plan_changed_since_the_confirmation_is_refused() {
    let repo = repo_with_unstaged_changes();
    let mut backend = repo.open();
    let paths = [RepoPath::from("d")];
    let plan = project_engine::discard_plan(&backend.status().unwrap(), &paths).unwrap();
    repo.write("d/altro.txt", "tracciato\n");
    repo.stage(&["d/altro.txt"]);
    repo.write("d/altro.txt", "e modificato\n");
    let before = snapshot(&repo, &["d/x.txt", "d/altro.txt"]);
    let err = backend.discard(&paths, &plan).unwrap_err();
    assert!(err.to_string().contains("cambiati dalla conferma"), "{err}");
    assert_eq!(snapshot(&repo, &["d/x.txt", "d/altro.txt"]), before);
}
