//! Test di integrazione di `edit.undo`, `edit.redo` ed `edit.history` (diario delle
//! operazioni, §7) con il `CliBackend` su repo veri: undo e redo di un commit, del primo
//! commit, di un `amend` e con HEAD staccata; rifiuto se i ref sono cambiati da fuori; backup
//! dei commit annullati che resiste a `git gc`; hook che rifiutano la transazione.

mod common;

use common::{Repo, repo_detached_head, repo_with_divergent_branches};
use project_engine::{EngineError, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

/// Registro e sessione con `repo` aperto, come la CLI.
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

/// Tutti i ref con il valore (`for-each-ref`) e HEAD, per controllare che niente cambi.
fn refs(repo: &Repo) -> String {
    let head = repo
        .git_output(&["symbolic-ref", "-q", "HEAD"])
        .stdout
        .clone();
    format!(
        "{}\nHEAD -> {}{}",
        repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        String::from_utf8_lossy(&head).trim(),
        repo.git(&["rev-parse", "-q", "--verify", "HEAD"])
    )
}

/// Indice (`ls-files -s`) e contenuto dei file del worktree indicati.
fn index_and_files(repo: &Repo, files: &[&str]) -> (String, Vec<Option<String>>) {
    let contents = files
        .iter()
        .map(|f| std::fs::read_to_string(repo.path().join(f)).ok())
        .collect();
    (repo.git(&["ls-files", "-s"]), contents)
}

/// `git status --porcelain` (codici XY e percorsi).
fn porcelain(repo: &Repo) -> String {
    repo.git(&["status", "--porcelain"])
}

/// Un repo con un commit (`a.txt`), `b.txt` nuovo in stage e `a.txt` modificato fuori stage.
fn repo_ready_to_commit() -> Repo {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    repo.write("a.txt", "a modificato\n");
    repo
}

const FILES: &[&str] = &["a.txt", "b.txt"];

#[test]
fn undo_and_redo_of_a_commit() {
    let repo = repo_ready_to_commit();
    let before = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    let commit = repo.rev("HEAD");
    assert_eq!(out["commit"]["id"], commit.to_string());
    let after_commit = index_and_files(&repo, FILES);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    // Il branch torna al commit di prima; HEAD resta sul branch.
    assert_eq!(repo.rev("refs/heads/main"), before);
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    // Indice e worktree non cambiano: le modifiche del commit tornano in stage.
    assert_eq!(index_and_files(&repo, FILES), after_commit);
    assert_eq!(porcelain(&repo), " M a.txt\nA  b.txt");
    assert_eq!(
        out["status"]["head"]["Branch"]["target"],
        before.to_string()
    );
    let undone = &out["undone"];
    assert_eq!(undone["command"], "commit.create");
    assert_eq!(
        undone["description"],
        format!("Commit {} «Aggiungi b»", &commit.to_string()[..7])
    );
    assert_eq!(
        undone["refs"],
        json!([{ "name": "refs/heads/main", "before": before.to_string(), "after": commit.to_string() }])
    );
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["can_redo"], true);
    // Il commit annullato è in un ref di backup.
    let backups = out["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 1);
    let backup = backups[0].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/undo-"), "{backup}");
    assert_eq!(repo.rev(backup), commit);
    // Reflog del branch e di HEAD.
    let message = format!(
        "project: undo Commit {} «Aggiungi b»",
        &commit.to_string()[..7]
    );
    assert_eq!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "main"]),
        message
    );
    assert_eq!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "HEAD"]),
        message
    );

    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), commit);
    assert_eq!(index_and_files(&repo, FILES), after_commit);
    assert_eq!(porcelain(&repo), " M a.txt");
    assert_eq!(out["redone"]["id"], undone["id"]);
    assert_eq!(
        out["backups"],
        json!([]),
        "il commit di prima è il genitore"
    );
    assert_eq!(out["redone"]["undo_backups"], json!([backup]));
    assert_eq!(out["can_undo"], true);
    assert_eq!(out["can_redo"], false);
    assert!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "main"])
            .starts_with("project: redo Commit ")
    );
}

#[test]
fn undo_refused_if_the_branch_moved_from_outside() {
    // Un `git commit` da terminale dopo il nostro.
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    repo.commit_file("c.txt", "c\n", "da terminale");
    let before = refs(&repo);
    let index = index_and_files(&repo, FILES);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let EngineError::UndoConflict {
        command,
        refs: changed,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(*command, "edit.undo");
    assert!(changed[0].contains("refs/heads/main"), "{changed:?}");
    assert_eq!(refs(&repo), before, "nessun ref cambiato");
    assert_eq!(index_and_files(&repo, FILES), index);
    assert!(
        !repo
            .git(&["for-each-ref", "refs/project/"])
            .contains("undo-")
    );
    // La voce resta nel diario.
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_undo"], true);

    // Un `git reset` da terminale.
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    repo.git(&["reset", "--quiet", "--soft", "HEAD~1"]);
    let before = refs(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(refs(&repo), before);

    // HEAD passata a un altro branch.
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    repo.git(&["switch", "--quiet", "-c", "altro"]);
    let before = refs(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("HEAD"), "{err}");
    assert_eq!(refs(&repo), before);
}

#[test]
fn redo_refused_if_the_branch_moved_after_undo() {
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    run(&r, &mut s, "edit.undo", json!({}));
    // Da terminale: un altro commit con quello che è in stage.
    repo.commit("da terminale");
    let before = refs(&repo);
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::UndoConflict {
                command: "edit.redo",
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(refs(&repo), before);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_redo"], true);
}

#[test]
fn undo_of_the_first_commit_leaves_an_unborn_branch() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.stage(&["a.txt"]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.create", json!({ "message": "primo" }));
    assert_eq!(out["previous"], Value::Null);
    let first = repo.rev("HEAD");
    let index = index_and_files(&repo, &["a.txt"]);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        !repo
            .git_output(&["rev-parse", "-q", "--verify", "refs/heads/main"])
            .status
            .success(),
        "il branch è cancellato"
    );
    assert_eq!(repo.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(index_and_files(&repo, &["a.txt"]), index);
    assert_eq!(porcelain(&repo), "A  a.txt");
    assert_eq!(out["status"]["head"]["Branch"]["target"], Value::Null);
    let backup = out["backups"][0].as_str().unwrap();
    assert_eq!(repo.rev(backup), first);
    // `repo.log` su un branch senza commit è vuoto.
    let log = run(&r, &mut s, "repo.log", json!({}));
    assert_eq!(log["commits"], json!([]));

    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), first);
    assert_eq!(porcelain(&repo), "");
}

#[test]
fn undo_of_an_amend_returns_to_the_rewritten_commit() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\n", "primo");
    let old = repo.commit_file("b.txt", "b\n", "secondo");
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "secondo, con c", "amend": true }),
    );
    let amended = repo.rev("HEAD");
    assert_eq!(out["previous"], old.to_string());
    let amend_backup = out["backup"].as_str().unwrap().to_owned();
    let index = index_and_files(&repo, &["a.txt", "b.txt", "c.txt"]);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), old);
    assert_eq!(index_and_files(&repo, &["a.txt", "b.txt", "c.txt"]), index);
    // Quello che `amend` aveva aggiunto torna in stage.
    assert_eq!(porcelain(&repo), "A  c.txt");
    assert!(
        out["undone"]["description"]
            .as_str()
            .unwrap()
            .starts_with("Amend ")
    );
    assert_eq!(out["undone"]["backup"], amend_backup);
    // Il commit di `amend` ora è salvato; il commit riscritto era già nel backup di `amend`.
    let backup = out["backups"][0].as_str().unwrap();
    assert_eq!(repo.rev(backup), amended);
    assert_eq!(repo.rev(&amend_backup), old);

    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), amended);
    assert_eq!(
        out["backups"],
        json!([]),
        "il commit riscritto ha già un backup"
    );
    assert_eq!(porcelain(&repo), "");
}

#[test]
fn undo_with_detached_head() {
    let h = repo_detached_head();
    let repo = &h.repo;
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let branches = repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    let (r, mut s) = open(repo);
    let out = run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "staccata" }),
    );
    assert_eq!(out["branch"], Value::Null);
    let commit = repo.rev("HEAD");

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["refs"], json!([]));
    assert_eq!(
        out["undone"]["head"],
        json!({ "before": { "detached": h.f1.to_string() }, "after": { "detached": commit.to_string() } })
    );
    assert!(
        !repo
            .git_output(&["symbolic-ref", "-q", "HEAD"])
            .status
            .success(),
        "HEAD resta staccata"
    );
    assert_eq!(repo.rev("HEAD"), h.f1);
    assert_eq!(porcelain(repo), "A  c.txt");
    let backup = out["backups"][0].as_str().unwrap().to_owned();
    assert_eq!(repo.rev(&backup), commit);
    let only_backup_added = repo
        .git(&["for-each-ref", "--format=%(refname) %(objectname)"])
        .lines()
        .filter(|l| !l.starts_with(&backup))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(only_backup_added, branches, "nessun branch spostato");
    assert_eq!(
        repo.git(&["log", "-g", "-1", "--format=%gs", "HEAD"]),
        format!(
            "project: undo Commit {} «staccata»",
            &commit.to_string()[..7]
        )
    );

    // HEAD spostata da fuori: il redo è rifiutato.
    repo.detach(h.c1);
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(repo.rev("HEAD"), h.c1);
    repo.detach(h.f1);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("HEAD"), commit);
}

#[test]
fn several_undos_then_a_new_commit_clears_redo() {
    let repo = Repo::new();
    let start = repo.commit_file("a.txt", "0\n", "inizio");
    let (r, mut s) = open(&repo);
    let mut ids = Vec::new();
    for n in 1..=3 {
        repo.write("a.txt", &format!("{n}\n"));
        run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
        run(
            &r,
            &mut s,
            "commit.create",
            json!({ "message": format!("commit {n}") }),
        );
        ids.push(repo.rev("HEAD"));
    }
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"].as_array().unwrap().len(), 3);

    for expected in [ids[1], ids[0], start] {
        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(repo.rev("refs/heads/main"), expected);
    }
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    // Il worktree e l'indice sono sempre quelli dell'ultimo commit.
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "3\n"
    );
    assert_eq!(porcelain(&repo), "M  a.txt");

    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), ids[0]);
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["undone"].as_array().unwrap().len(), 2);

    // Una nuova operazione svuota il redo.
    run(&r, &mut s, "commit.create", json!({ "message": "nuovo" }));
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["can_redo"], false);
    assert_eq!(history["undone"], json!([]));
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
}

#[test]
fn the_undone_commit_survives_gc() {
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    let commit = repo.rev("HEAD").to_string();
    let out = run(&r, &mut s, "edit.undo", json!({}));
    let backup = out["backups"][0].as_str().unwrap().to_owned();

    // Il reflog scaduto non lo tiene più: solo il ref di backup.
    repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    repo.git(&["gc", "--quiet", "--prune=now"]);
    repo.git(&["cat-file", "-e", &format!("{commit}^{{commit}}")]);
    // Il redo funziona anche dopo il gc.
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main").to_string(), commit);

    // Controprova: senza il backup (e con il branch indietro) il gc lo cancella.
    run(&r, &mut s, "edit.undo", json!({}));
    repo.git(&["update-ref", "-d", &backup]);
    repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    repo.git(&["gc", "--quiet", "--prune=now"]);
    assert!(
        !repo
            .git_output(&["cat-file", "-e", &commit])
            .status
            .success()
    );
}

#[test]
fn a_hook_that_rejects_the_transaction_changes_nothing() {
    let repo = repo_ready_to_commit();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    let commit = repo.rev("HEAD");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook = repo.path().join(".git/hooks/reference-transaction");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(
            &hook,
            "#!/bin/sh\nif [ \"$1\" = prepared ] && grep -q refs/heads/main; then\n  echo 'no: rifiutato' >&2\n  exit 1\nfi\n",
        )
        .unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::Git { .. }), "{err}");
        assert!(err.to_string().contains("rifiutato"), "{err}");
        assert_eq!(repo.rev("refs/heads/main"), commit);
        std::fs::remove_file(&hook).unwrap();
    }
    // La voce è rimasta: senza l'hook l'undo riesce.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_ne!(repo.rev("refs/heads/main"), commit);
}

#[test]
fn hostile_config_does_not_change_the_result() {
    let repo = repo_ready_to_commit();
    // Configurazione del repo che potrebbe disturbare: niente reflog, colori, pager, alias.
    repo.git(&["config", "core.logAllRefUpdates", "false"]);
    repo.git(&["config", "color.ui", "always"]);
    repo.git(&["config", "core.pager", "false"]);
    repo.git(&["config", "alias.update-ref", "!false"]);
    let before = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.create",
        json!({ "message": "Aggiungi b" }),
    );
    let commit = repo.rev("HEAD");
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), before);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("refs/heads/main"), commit);
}

#[test]
fn refused_while_a_merge_is_in_progress() {
    let (repo, _) = repo_with_divergent_branches();
    repo.write("c.txt", "c\n");
    repo.stage(&["c.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.create", json!({ "message": "c" }));
    repo.conflict("altro");
    let before = refs(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("merge"), "{err}");
    assert_eq!(refs(&repo), before);
}
