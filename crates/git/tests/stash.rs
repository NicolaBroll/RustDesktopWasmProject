//! Test di integrazione degli stash (S28: `stash.list`, `stash.push`, `stash.apply`,
//! `stash.pop`, `stash.drop` e il loro undo/redo) con il `CliBackend` su repo temporanei:
//! push con le opzioni (non tracciati, `keep_index`, percorsi, messaggio che inizia con `-`),
//! undo del push che rimette anche la divisione fra in stage e no, rifiuti senza cambiare
//! niente (modifiche locali, non tracciati e ignorati al posto dei file dello stash: casi in cui
//! `git stash apply` scriverebbe a metà), conflitti lasciati con lo stash tenuto, `restore_index`
//! che non si applica, drop con conferma, backup e undo, basi degli stash nel grafo.

mod common;

use common::Repo;
use project_engine::{EngineError, GitBackend, Registry, Session};
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

/// Stato (con i non tracciati e gli ignorati), contenuto dei file e ref: per vedere che niente
/// cambia, o che tutto torna com'era.
fn snapshot(repo: &Repo) -> (String, Vec<(String, String)>, String) {
    let status = repo.git(&["status", "--porcelain", "-uall", "--ignored"]);
    let mut files = Vec::new();
    for name in ["a.txt", "b.txt", "nuovo.txt", "dir/u.txt", "x.log", "c.txt"] {
        if let Ok(content) = std::fs::read_to_string(repo.path().join(name)) {
            files.push((name.to_owned(), content));
        }
    }
    let refs = repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]);
    (status, files, refs)
}

/// Un repo con due file committati e ogni tipo di modifica: `a.txt` cambiato in stage e poi
/// ancora fuori (`MM`), `b.txt` cambiato fuori stage, `nuovo.txt` aggiunto in stage,
/// `dir/u.txt` non tracciato, `x.log` ignorato.
fn dirty() -> Repo {
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignora i log");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("due file");
    repo.write("a.txt", "a in stage\n");
    repo.stage(&["a.txt"]);
    repo.write("a.txt", "a fuori stage\n");
    repo.write("b.txt", "b fuori stage\n");
    repo.write("nuovo.txt", "nuovo\n");
    repo.stage(&["nuovo.txt"]);
    repo.write("dir/u.txt", "non tracciato\n");
    repo.write("x.log", "ignorato\n");
    repo
}

#[test]
fn push_list_undo_and_redo() {
    let repo = dirty();
    let before = snapshot(&repo);
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    assert_eq!(
        run(&r, &mut s, "stash.list", json!({})),
        json!({ "stashes": [] })
    );

    let out = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "message": "-x --all prova", "include_untracked": true }),
    );
    let stash = out["stash"].clone();
    assert_eq!(stash["message"], "On main: -x --all prova");
    assert_eq!(stash["branch"], "main");
    assert_eq!(stash["base"], head.to_string());
    assert_eq!(stash["untracked"], true);
    assert_eq!(stash["id"], repo.rev("refs/stash").to_string());
    assert_eq!(out["stashes"], json!([stash]));
    // Resta solo l'ignorato; niente `--all`.
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall", "--ignored"]),
        "!! x.log"
    );
    assert_eq!(
        repo.git(&["stash", "list"]),
        "stash@{0}: On main: -x --all prova"
    );
    // Il commit dello stash si legge come un commit: i file tracciati rispetto alla base.
    let show = run(&r, &mut s, "commit.show", json!({ "id": stash["id"] }));
    let files: Vec<&str> = show["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["a.txt", "b.txt", "nuovo.txt"]);
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"], json!([stash]));

    // Undo: tutto come prima, anche in stage e fuori, con il commit dello stash nel backup.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["undone"]["command"], "stash.push");
    assert_eq!(snapshot(&repo).0, before.0);
    assert_eq!(snapshot(&repo).1, before.1);
    let backup = undo["backups"][0].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/undo-"), "{backup}");
    assert_eq!(repo.rev(&backup).to_string(), stash["id"]);
    assert_eq!(repo.git(&["stash", "list"]), "");

    // Redo: di nuovo da parte, con gli stessi parametri.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["redone"]["command"], "stash.push");
    assert_eq!(
        repo.git(&["stash", "list"]),
        "stash@{0}: On main: -x --all prova"
    );
    assert_eq!(
        redo["redone"]["stash"]["id"],
        repo.rev("refs/stash").to_string()
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(snapshot(&repo).1, before.1);
}

#[test]
fn push_with_keep_index_and_paths() {
    let repo = dirty();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({ "keep_index": true }));
    // L'indice resta, il worktree come l'indice.
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall"]),
        "M  a.txt\nA  nuovo.txt\n?? dir/u.txt"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a in stage\n"
    );

    let repo = dirty();
    let (r, mut s) = open(&repo);
    // Un percorso che inizia con `-` e uno vero: vanno a git su stdin, alla lettera.
    let err = r
        .run("stash.push", &mut s, json!({ "paths": ["-x"] }))
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("niente da mettere da parte")),
        "{err}"
    );
    let out = run(&r, &mut s, "stash.push", json!({ "paths": ["b.txt"] }));
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall"]),
        "MM a.txt\nA  nuovo.txt\n?? dir/u.txt"
    );
    assert!(
        out["stash"]["message"]
            .as_str()
            .unwrap()
            .starts_with("WIP on main: "),
        "{out}"
    );
}

#[test]
fn push_refusals_change_nothing() {
    // Niente da mettere da parte (solo un ignorato e, senza `include_untracked`, un non
    // tracciato).
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignora");
    repo.write("x.log", "?\n");
    repo.write("u.txt", "?\n");
    let (r, mut s) = open(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.git(&["stash", "list"]), "");

    // Un file `intent-to-add`: git non saprebbe salvarlo.
    repo.write("i.txt", "x\n");
    repo.git(&["add", "-N", "i.txt"]);
    let before = snapshot(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("intent-to-add"), "{err}");
    assert_eq!(snapshot(&repo), before);

    // Merge in conflitto.
    let repo = common::repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    let before = snapshot(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("è in corso un merge"), "{err}");
    assert_eq!(snapshot(&repo), before);

    // Branch senza commit.
    let repo = Repo::new();
    repo.write("a.txt", "x\n");
    repo.stage(&["a.txt"]);
    let (r, mut s) = open(&repo);
    let err = r.run("stash.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("non ha ancora commit"), "{err}");
}

#[test]
fn apply_and_pop() {
    let repo = dirty();
    let before = snapshot(&repo);
    let (r, mut s) = open(&repo);
    let id = run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    )["stash"]["id"]
        .clone();

    // Apply senza indice: tutto fuori stage tranne il file nuovo, lo stash resta.
    let out = run(&r, &mut s, "stash.apply", json!({ "expected": id }));
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["stashes"][0]["id"], id);
    assert_eq!(
        repo.git(&["status", "--porcelain", "-uall"]),
        " M a.txt\n M b.txt\nA  nuovo.txt\n?? dir/u.txt"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a fuori stage\n"
    );
    // Non è nel diario: il prossimo undo è il push.
    assert_eq!(
        run(&r, &mut s, "edit.history", json!({}))["done"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Di nuovo sopra le stesse modifiche: rifiutato prima di git, niente cambia.
    let now = snapshot(&repo);
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`a.txt`") && m.contains("`dir/u.txt`")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), now);

    // Pop con l'indice da un worktree pulito: tutto come all'inizio, con il backup.
    repo.git(&["reset", "--hard", "--quiet"]);
    repo.git(&["clean", "-fdq"]);
    let out = run(
        &r,
        &mut s,
        "stash.pop",
        json!({ "expected": id, "restore_index": true }),
    );
    assert_eq!(out["dropped"], true);
    assert_eq!(out["stashes"], json!([]));
    assert_eq!(snapshot(&repo).0, before.0);
    assert_eq!(snapshot(&repo).1, before.1);
    let backup = out["backup"].as_str().unwrap();
    assert!(backup.starts_with("refs/project/backup/stash-"), "{backup}");
    assert_eq!(repo.rev(backup).to_string(), id);
}

#[test]
fn files_in_the_way_block_the_apply_before_git_writes() {
    // `git stash apply` con un non tracciato dello stash già presente rimetterebbe i file
    // tracciati e poi fallirebbe: qui niente cambia.
    let repo = dirty();
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    repo.write("dir/u.txt", "un altro\n");
    let before = snapshot(&repo);
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`dir/u.txt`")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), before);

    // Un file al posto della cartella di un non tracciato dello stash.
    std::fs::remove_dir_all(repo.path().join("dir")).unwrap();
    repo.write("dir", "file\n");
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`dir`")),
        "{err}"
    );

    // Un ignorato al posto di un file che lo stash aggiunge: git lo sovrascriverebbe.
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignora");
    repo.write("nuovo.log", "in stash\n");
    repo.git(&["add", "-f", "nuovo.log"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.write("nuovo.log", "ignorato, da non perdere\n");
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`nuovo.log`")),
        "{err}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("nuovo.log")).unwrap(),
        "ignorato, da non perdere\n"
    );
    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
}

#[test]
fn conflicts_are_left_and_pop_keeps_the_stash() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "uno\ndue\ntre\n", "primo");
    repo.write("a.txt", "uno\nDUE dallo stash\ntre\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let (r, mut s) = open(&repo);
    let id = run(&r, &mut s, "stash.push", json!({}))["stash"]["id"].clone();
    repo.commit_file("a.txt", "uno\nDUE da main\ntre\n", "cambia a");

    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["dropped"], false);
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["stashes"][0]["id"], id);
    assert!(
        out["warnings"][0]
            .as_str()
            .unwrap()
            .contains("resta nell'elenco"),
        "{out}"
    );
    let a = std::fs::read_to_string(repo.path().join("a.txt")).unwrap();
    assert!(
        a.contains("<<<<<<<") && a.contains("DUE dallo stash"),
        "{a}"
    );
    assert!(repo.git(&["status", "--porcelain"]).contains("UU a.txt"));
    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
    // Con il conflitto aperto, niente stash.
    let err = r.run("stash.apply", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("in conflitto"), "{err}");
}

#[test]
fn restore_index_that_does_not_apply_changes_nothing() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "a\nb\nc\n", "primo");
    repo.write("a.txt", "a\nS\nc\n");
    repo.stage(&["a.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.commit_file("a.txt", "a\nT\nc\n", "cambia a");
    let before = snapshot(&repo);
    let err = r
        .run("stash.apply", &mut s, json!({ "restore_index": true }))
        .unwrap_err();
    assert!(err.to_string().contains("senza `restore_index`"), "{err}");
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn drop_needs_confirm_and_undo_puts_it_back_on_top() {
    let repo = dirty();
    let (r, mut s) = open(&repo);
    let first = run(&r, &mut s, "stash.push", json!({ "paths": ["b.txt"] }))["stash"].clone();
    let second = run(&r, &mut s, "stash.push", json!({ "message": "il resto" }))["stash"].clone();
    let refs = repo.git(&["for-each-ref"]);

    let err = r
        .run("stash.drop", &mut s, json!({ "index": 1 }))
        .unwrap_err();
    let EngineError::ConfirmRequired { expected, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(expected.as_deref(), first["id"].as_str());
    let err2 = r
        .run("stash.drop", &mut s, json!({ "index": 1, "confirm": true }))
        .unwrap_err();
    assert!(matches!(err2, EngineError::InvalidParams { .. }), "{err2}");
    // Con dei percorsi git mette nello stash comunque tutto l'indice: anche `a.txt` e
    // `nuovo.txt`, che erano in stage (e restano nel worktree).
    assert!(err.to_string().contains("3 file tracciati"), "{err}");
    assert_eq!(repo.git(&["for-each-ref"]), refs);
    // La conferma vale per lo stash visto.
    let err = r
        .run(
            "stash.drop",
            &mut s,
            json!({ "index": 1, "expected": second["id"], "confirm": true }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("ora è `stash@{0}`"), "{err}");

    let out = run(
        &r,
        &mut s,
        "stash.drop",
        json!({ "index": 1, "expected": first["id"], "confirm": true }),
    );
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/stash-"), "{backup}");
    assert_eq!(repo.rev(&backup).to_string(), first["id"]);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(
        repo.git(&["stash", "list", "--format=%H"]),
        second["id"].as_str().unwrap()
    );

    // Undo: in cima.
    run(&r, &mut s, "edit.undo", json!({}));
    let list = run(&r, &mut s, "stash.list", json!({}));
    assert_eq!(list["stashes"][0]["id"], first["id"]);
    assert_eq!(list["stashes"][0]["message"], first["message"]);
    assert_eq!(list["stashes"][1]["id"], second["id"]);
    // Redo: di nuovo via, ovunque sia. Il backup del drop è stato tolto a mano: se ne fa uno
    // nuovo prima di eliminare.
    repo.git(&["update-ref", "-d", &backup]);
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    let again = redo["backups"][0].as_str().unwrap().to_owned();
    assert!(again.starts_with("refs/project/backup/redo-"), "{redo}");
    assert_eq!(repo.rev(&again).to_string(), first["id"]);
    assert_eq!(
        repo.git(&["stash", "list", "--format=%H"]),
        second["id"].as_str().unwrap()
    );
    run(&r, &mut s, "edit.undo", json!({}));
    // E si applica ancora.
    let out = run(&r, &mut s, "stash.pop", json!({ "expected": first["id"] }));
    assert_eq!(out["dropped"], true);
    assert!(repo.git(&["status", "--porcelain"]).contains(" M b.txt"));
}

#[test]
fn undo_of_a_push_is_refused_once_head_moved() {
    let repo = dirty();
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.commit_file("c.txt", "c\n", "da terminale");
    let before = snapshot(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::UndoConflict { refs, .. } if refs[0].starts_with("HEAD")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), before);
}

#[test]
fn log_reaches_the_stash_bases() {
    let repo = Repo::new();
    let first = repo.commit_file("a.txt", "a\n", "primo");
    let base = repo.commit_file("a.txt", "b\n", "secondo");
    repo.write("a.txt", "c\n");
    let (r, mut s) = open(&repo);
    let stash = run(&r, &mut s, "stash.push", json!({}))["stash"].clone();
    assert_eq!(stash["base"], base.to_string());
    // `main` torna indietro: la base resta solo come genitore dello stash.
    repo.git(&["reset", "--hard", "--quiet", &first.to_string()]);
    let ids = |out: &Value| -> Vec<String> {
        out["commits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let all = run(&r, &mut s, "repo.log", json!({ "all": true }));
    assert_eq!(ids(&all), [first.to_string()]);
    let with = run(
        &r,
        &mut s,
        "repo.log",
        json!({ "all": true, "stashes": true }),
    );
    assert_eq!(ids(&with), [base.to_string(), first.to_string()]);
    // Il backend dice lo stesso.
    let backend = repo.open();
    assert_eq!(backend.stashes().unwrap()[0].base, base);
}

#[test]
fn push_refuses_assume_unchanged_files() {
    // `git stash push` non vedrebbe la modifica di `b.txt` e poi la cancellerebbe.
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("due file");
    repo.git(&["update-index", "--assume-unchanged", "b.txt"]);
    repo.write("a.txt", "a cambiato\n");
    repo.write("b.txt", "b cambiato, da non perdere\n");
    let (r, mut s) = open(&repo);
    for params in [
        json!({}),
        json!({ "keep_index": true }),
        json!({ "include_untracked": true }),
        json!({ "paths": ["b.txt"] }),
    ] {
        let err = r.run("stash.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.contains("assume-unchanged") && m.contains("`b.txt`")),
            "{params}: {err}"
        );
    }
    assert_eq!(repo.git(&["stash", "list"]), "");
    // Fuori dai percorsi scelti non conta.
    run(&r, &mut s, "stash.push", json!({ "paths": ["a.txt"] }));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("b.txt")).unwrap(),
        "b cambiato, da non perdere\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a\n"
    );
}

#[test]
fn apply_does_not_follow_directory_renames_onto_an_ignored_file() {
    // Lo stash aggiunge `old/x.log`; poi HEAD rinomina `old` in `new`, dove c'è un `x.log`
    // ignorato: con la rilevazione delle cartelle rinominate git lo sovrascriverebbe.
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignora");
    repo.commit_file("old/a.txt", "a\n", "old");
    repo.write("old/x.log", "dallo stash\n");
    repo.git(&["add", "-f", "old/x.log"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({}));
    repo.git(&["mv", "old", "new"]);
    repo.commit("rinomina la cartella");
    repo.write("new/x.log", "ignorato, da non perdere\n");
    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(out["dropped"], true);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("new/x.log")).unwrap(),
        "ignorato, da non perdere\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("old/x.log")).unwrap(),
        "dallo stash\n"
    );
}

/// `a.txt` cambiato solo in stage, `b.txt` solo fuori stage.
fn staged_and_unstaged() -> Repo {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("due file");
    repo.write("a.txt", "a in stage\n");
    repo.stage(&["a.txt"]);
    repo.write("b.txt", "b fuori stage\n");
    repo
}

#[test]
fn undo_of_keep_index_and_paths_pushes() {
    for params in [json!({ "keep_index": true }), json!({ "paths": ["b.txt"] })] {
        let repo = staged_and_unstaged();
        let before = snapshot(&repo);
        let (r, mut s) = open(&repo);
        run(&r, &mut s, "stash.push", params.clone());
        // `a.txt` resta in stage (anche con `paths`, ma git lo mette pure nello stash).
        assert_eq!(repo.git(&["status", "--porcelain"]), "M  a.txt", "{params}");
        let undo = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(undo["undone"]["command"], "stash.push", "{params}");
        assert_eq!(snapshot(&repo).0, before.0, "{params}");
        assert_eq!(snapshot(&repo).1, before.1, "{params}");
        // E il pop dopo un push così.
        run(&r, &mut s, "edit.redo", json!({}));
        let out = run(&r, &mut s, "stash.pop", json!({ "restore_index": true }));
        assert_eq!(out["dropped"], true, "{params}");
        assert_eq!(snapshot(&repo).0, before.0, "{params}");
    }
}

#[test]
fn untracked_to_put_back_blocks_even_when_the_status_shows_it() {
    // Lo stash ha il non tracciato `p` e una modifica a `t.txt`. Poi `p` è committato, tolto
    // dall'indice (`D.`), ignorato e riscritto: `git stash apply` farebbe il merge di `t.txt` e
    // poi fallirebbe su `p`. Qui niente cambia.
    let repo = Repo::new();
    repo.commit_file("t.txt", "t\n", "primo");
    repo.write("p", "dallo stash\n");
    repo.write("t.txt", "t cambiato\n");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "stash.push",
        json!({ "include_untracked": true }),
    );
    repo.commit_file("p", "committato\n", "p");
    repo.git(&["rm", "--cached", "--quiet", "p"]);
    repo.write(".git/info/exclude", "p\n");
    repo.write("p", "LOCAL\n");
    assert_eq!(repo.git(&["status", "--porcelain"]), "D  p");
    let before = snapshot(&repo);
    let p = || std::fs::read_to_string(repo.path().join("p")).unwrap();
    let err = r.run("stash.pop", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("`p`")),
        "{err}"
    );
    assert_eq!(snapshot(&repo), before);
    assert_eq!(p(), "LOCAL\n");
    assert_eq!(repo.git(&["stash", "list"]).lines().count(), 1);
}

#[test]
fn undo_of_a_paths_push_with_other_staged_files_explains_the_way_out() {
    // `a.txt` e `b.txt` in stage, push di `b.txt`: git mette nello stash tutto l'indice, e
    // `pop --index` non si applica sopra l'indice che ha già `a.txt`.
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.stage(&["a.txt", "b.txt"]);
    repo.commit("due file");
    repo.write("a.txt", "a in stage\n");
    repo.write("b.txt", "b in stage\n");
    repo.stage(&["a.txt", "b.txt"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "stash.push", json!({ "paths": ["b.txt"] }));
    let before = snapshot(&repo);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("`stash.pop` senza `restore_index`") && !text.contains("riprovare senza"),
        "{text}"
    );
    assert_eq!(snapshot(&repo), before);
    assert_eq!(run(&r, &mut s, "edit.history", json!({}))["can_undo"], true);
    // La via d'uscita indicata funziona.
    let out = run(&r, &mut s, "stash.pop", json!({}));
    assert_eq!(out["dropped"], true);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("b.txt")).unwrap(),
        "b in stage\n"
    );
}
