//! Test di integrazione di `commit.cherry_pick`, `commit.revert`, `cherry_pick.continue`,
//! `cherry_pick.abort`, `revert.continue`, `revert.abort` (S31) con il `CliBackend`: riuscito con
//! undo e redo, autore preservato, messaggio del revert, già applicato (niente resta in corso),
//! conflitto che resta in corso e si conclude dopo `conflict.resolve`/`index.stage`, abort con
//! e senza conferma e il suo backup, commit di merge con e senza `mainline`, modifiche locali,
//! file non tracciati o ignorati al posto di quelli in arrivo, HEAD staccata, firma che fallisce,
//! sequenza di più commit avviata da terminale.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{EngineError, Registry, Session};
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

fn read(repo: &Repo, file: &str) -> Option<String> {
    std::fs::read_to_string(repo.path().join(file)).ok()
}

fn backups(repo: &Repo, kind: &str) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        &format!("refs/project/backup/{kind}-*"),
    ]);
    out.lines().map(str::to_owned).collect()
}

fn clean(repo: &Repo) -> bool {
    repo.git(&["status", "--porcelain"]).is_empty()
}

fn has(repo: &Repo, file: &str) -> bool {
    repo.path().join(".git").join(file).exists()
}

fn parents(repo: &Repo, rev: &str) -> Vec<Oid> {
    repo.git(&["rev-list", "--parents", "-n", "1", rev])
        .split_whitespace()
        .skip(1)
        .map(|h| h.parse().unwrap())
        .collect()
}

/// Un commit di `file` con autore Cleo (il committer dei test è Bob).
fn commit_by_cleo(repo: &Repo, file: &str, content: &str, message: &str) -> Oid {
    repo.write(file, content);
    repo.stage(&[file]);
    repo.git(&[
        "commit",
        "--quiet",
        "--author=Cleo <cleo@example.com>",
        "-m",
        message,
    ]);
    repo.rev("HEAD")
}

/// `main`: `base` (`a.txt` = «base») e poi `c.txt`; `altro` da `base`, un commit di Cleo che
/// aggiunge `b.txt` (con corpo nel messaggio): si applica senza conflitti.
fn picking_repo() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("altro");
    repo.checkout("altro");
    let theirs = commit_by_cleo(&repo, "b.txt", "b\n", "aggiunge b\n\ncon un corpo");
    repo.checkout("main");
    let main = repo.commit_file("c.txt", "c\n", "aggiunge c");
    (repo, main, theirs)
}

/// `main` e `altro` cambiano `a.txt` in modo diverso da `base`; il commit di `altro` è di Cleo.
fn conflicting_repo() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("altro");
    repo.checkout("altro");
    let theirs = commit_by_cleo(&repo, "a.txt", "loro\n", "a.txt su altro");
    repo.checkout("main");
    let main = repo.commit_file("a.txt", "nostra\n", "a.txt su main");
    (repo, main, theirs)
}

#[test]
fn cherry_pick_keeps_the_author_undo_and_redo() {
    let (repo, main, theirs) = picking_repo();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(out["commit"], theirs.to_string());
    assert_eq!(out["before"], main.to_string());
    let picked = repo.rev("main");
    assert_eq!(out["after"], picked.to_string());
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an <%ae>|%cn|%B", "main"]),
        "Cleo <cleo@example.com>|Bob|aggiunge b\n\ncon un corpo"
    );
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("b\n"));
    assert!(clean(&repo));
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), main);
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "main"]);
    assert!(
        reflog.starts_with("project: commit.cherry_pick"),
        "{reflog}"
    );
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Cherry-pick di altro su `main`"),
        "{}",
        out["next_undo"]
    );

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    assert!(read(&repo, "b.txt").is_none());
    assert!(clean(&repo));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), picked);
    assert!(clean(&repo));

    // Già applicato: NotAllowed, niente in corso, niente `MERGE_MSG`, il backup tolto.
    let before = backups(&repo, "cherry-pick").len();
    let err = r
        .run(
            "commit.cherry_pick",
            &mut s,
            json!({ "rev": theirs.to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("non cambierebbe niente"), "{err}");
    assert_eq!(repo.rev("main"), picked);
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert!(!has(&repo, "MERGE_MSG"));
    assert!(clean(&repo));
    assert_eq!(backups(&repo, "cherry-pick").len(), before);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], Value::Null);
}

#[test]
fn revert_message_and_already_reverted() {
    let (repo, main, _) = picking_repo();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD" }));
    assert_eq!(out["in_progress"], false);
    let reverted = repo.rev("main");
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an|%B", "main"]),
        format!("Ada|Revert \"aggiunge c\"\n\nThis reverts commit {main}.")
    );
    assert!(read(&repo, "c.txt").is_none());
    assert!(clean(&repo));
    assert!(!has(&repo, "REVERT_HEAD"));
    assert_eq!(backups(&repo, "revert").len(), 1);
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "main"]);
    assert!(reflog.starts_with("project: commit.revert"), "{reflog}");

    // `revert.reference` non cambia il messaggio.
    repo.git(&["config", "revert.reference", "true"]);
    // Già annullato: niente cambia, niente in corso.
    let err = r
        .run("commit.revert", &mut s, json!({ "rev": main.to_string() }))
        .unwrap_err();
    assert!(err.to_string().contains("non cambierebbe niente"), "{err}");
    assert_eq!(repo.rev("main"), reverted);
    assert!(!has(&repo, "REVERT_HEAD"));
    assert!(!has(&repo, "MERGE_MSG"));
    assert_eq!(backups(&repo, "revert").len(), 1);

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("c\n"));
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", "main"]),
        "Revert \"aggiunge c\""
    );
}

#[test]
fn conflict_stays_in_progress_then_resolve_and_continue() {
    let (repo, main, theirs) = conflicting_repo();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], main.to_string());
    assert_eq!(out["can_undo"], false);
    assert!(out["message"].as_str().unwrap().contains("a.txt"));
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    assert!(!has(&repo, "sequencer"));
    assert_eq!(repo.rev("main"), main);
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert_eq!(repo.rev(&backup), main);

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["message"], "a.txt su altro");
    assert_eq!(op["continue"], "cherry_pick.continue");
    assert_eq!(op["abort"], "cherry_pick.abort");
    assert_eq!(op["ready"], false);

    for (id, params) in [
        ("cherry_pick.continue", json!({})),
        ("merge.continue", json!({})),
        ("revert.abort", json!({ "confirm": true })),
        ("commit.revert", json!({ "rev": "altro" })),
        ("edit.undo", json!({})),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }
    assert!(has(&repo, "CHERRY_PICK_HEAD"));

    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "unita\n" }),
    );
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["ready"], true);
    let out = run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(out["message"], "a.txt su altro");
    let done = repo.rev("main");
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an|%cn|%B", "main"]),
        "Cleo|Bob|a.txt su altro"
    );
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("unita\n"));
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert!(clean(&repo));
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Cherry-pick di altro su `main`")
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("nostra\n"));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), done);
}

#[test]
fn revert_conflict_index_stage_and_continue() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "uno\n", "uno");
    let x = repo.commit_file("a.txt", "due\n", "due");
    let last = repo.commit_file("a.txt", "tre\n", "tre");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD~1" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert!(has(&repo, "REVERT_HEAD"));
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Revert");
    assert_eq!(op["continue"], "revert.continue");
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("non un cherry-pick"), "{err}");

    repo.write("a.txt", "uno e tre\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    let out = run(
        &r,
        &mut s,
        "revert.continue",
        json!({ "message": "Revert di due, a mano" }),
    );
    assert_eq!(out["message"], "Revert di due, a mano");
    assert_eq!(parents(&repo, "main"), [last]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%an|%B", "main"]),
        "Ada|Revert di due, a mano"
    );
    assert!(!has(&repo, "REVERT_HEAD"));
    assert!(clean(&repo));
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with(&format!("Revert di {} su `main`", &x.to_string()[..7]))
    );
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), last);
    assert!(clean(&repo));
}

#[test]
fn abort_needs_confirm_and_saves_a_backup() {
    let (repo, main, _) = conflicting_repo();
    let (r, mut s) = open(&repo);
    let err = r
        .run("cherry_pick.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(err.to_string().contains("non c'è un cherry-pick"), "{err}");

    run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    let err = r.run("cherry_pick.abort", &mut s, json!({})).unwrap_err();
    match &err {
        EngineError::ConfirmRequired { reason, .. } => {
            assert!(reason.contains("`a.txt`"), "{reason}");
            assert!(reason.contains("cherry-pick-abort"), "{reason}");
        }
        other => panic!("{other}"),
    }
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    // Una risoluzione a metà, che l'abort cancellerebbe: è nel backup.
    repo.write("a.txt", "a metà\n");
    let out = run(&r, &mut s, "cherry_pick.abort", json!({ "confirm": true }));
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/cherry-pick-abort-"),
        "{backup}"
    );
    assert_eq!(out["files"], json!(["a.txt"]));
    assert_eq!(out["can_undo"], false);
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
    assert_eq!(repo.rev("main"), main);
    assert!(clean(&repo));
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("nostra\n"));
    assert_eq!(repo.git(&["show", &format!("{backup}:a.txt")]), "a metà");
    assert_eq!(parents(&repo, &backup).len(), 2);
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "HEAD"]);
    assert!(!reflog.is_empty());

    // Il revert fermo si annulla allo stesso modo.
    let out = run(&r, &mut s, "commit.revert", json!({ "rev": "HEAD" }));
    assert_eq!(out["in_progress"], false);
    run(&r, &mut s, "edit.undo", json!({}));
    repo.commit_file("a.txt", "ancora\n", "ancora");
    let out = run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": main.to_string() }),
    );
    assert_eq!(out["in_progress"], true);
    let out = run(&r, &mut s, "revert.abort", json!({ "confirm": true }));
    assert!(
        out["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/revert-abort-")
    );
    assert!(!has(&repo, "REVERT_HEAD"));
    assert!(clean(&repo));
}

#[test]
fn merge_commit_with_and_without_mainline() {
    let repo = Repo::new();
    let base = repo.commit_file("a.txt", "base\n", "base");
    repo.branch("feat");
    repo.checkout("feat");
    repo.commit_file("f.txt", "f\n", "aggiunge f");
    repo.checkout("main");
    let m1 = repo.commit_file("m.txt", "m\n", "aggiunge m");
    let merge = repo.merge("feat");
    // Un branch da `base` dove applicare il merge.
    repo.git(&["checkout", "--quiet", "-b", "dest", &base.to_string()]);
    let dest = repo.commit_file("d.txt", "d\n", "aggiunge d");
    let (r, mut s) = open(&repo);
    let id = merge.to_string();
    for params in [
        json!({ "rev": id }),
        json!({ "rev": id, "mainline": 0 }),
        json!({ "rev": id, "mainline": 3 }),
        json!({ "rev": "HEAD", "mainline": 1 }),
    ] {
        let err = r
            .run("commit.cherry_pick", &mut s, params.clone())
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
    let err = r
        .run("commit.revert", &mut s, json!({ "rev": id }))
        .unwrap_err();
    assert!(err.to_string().contains("mainline"), "{err}");
    assert!(backups(&repo, "cherry-pick").is_empty());

    // Rispetto al primo genitore: le modifiche di `feat` (`f.txt`).
    let out = run(
        &r,
        &mut s,
        "commit.cherry_pick",
        json!({ "rev": id, "mainline": 1 }),
    );
    assert_eq!(out["mainline"], 1);
    assert_eq!(parents(&repo, "dest"), [dest]);
    assert_eq!(read(&repo, "f.txt").as_deref(), Some("f\n"));
    assert!(read(&repo, "m.txt").is_none());
    assert!(clean(&repo));

    // Il revert del merge su `main`, rispetto al primo genitore: toglie `f.txt`.
    repo.checkout("main");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "commit.revert",
        json!({ "rev": id, "mainline": 1 }),
    );
    assert!(read(&repo, "f.txt").is_none());
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "main"]),
        format!(
            "Revert \"Merge branch 'feat'\"\n\nThis reverts commit {merge}, reversing\nchanges \
             made to {m1}."
        )
    );
}

#[test]
fn local_changes_and_files_in_the_way_are_refused() {
    let (repo, main, _) = picking_repo();
    let (r, mut s) = open(&repo);
    // Una modifica a un file che il commit non tocca, fuori stage e poi in stage.
    repo.write("a.txt", "cambiato\n");
    for id in ["commit.cherry_pick", "commit.revert"] {
        let rev = if id == "commit.revert" {
            "HEAD"
        } else {
            "altro"
        };
        let err = r.run(id, &mut s, json!({ "rev": rev })).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains("`a.txt`"), "{id}: {err}");
        assert!(err.to_string().contains("stash.push"), "{id}: {err}");
    }
    repo.stage(&["a.txt"]);
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "altro" }))
        .unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    repo.git(&["reset", "--quiet", "--hard"]);

    // Un non tracciato al posto del file in arrivo: niente scritto, niente backup.
    repo.write("b.txt", "mio\n");
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "altro" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mio\n"));
    // Ignorato: lo stesso (git lo sovrascriverebbe senza dirlo).
    repo.write(".git/info/exclude", "b.txt\n");
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "altro" }))
        .unwrap_err();
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mio\n"));
    // Il revert che ricrea un file: un non tracciato al suo posto.
    std::fs::remove_file(repo.path().join("b.txt")).unwrap();
    repo.git(&["rm", "--quiet", "c.txt"]);
    let gone = repo.commit("toglie c");
    repo.write("c.txt", "mio\n");
    let err = r
        .run("commit.revert", &mut s, json!({ "rev": gone.to_string() }))
        .unwrap_err();
    assert!(err.to_string().contains("`c.txt`"), "{err}");
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("mio\n"));
    assert_eq!(repo.rev("main"), gone);
    assert_ne!(gone, main);
    assert!(backups(&repo, "cherry-pick").is_empty());
    assert!(backups(&repo, "revert").is_empty());
    // Non tracciati altrove vanno bene.
    std::fs::remove_file(repo.path().join("c.txt")).unwrap();
    repo.write("altro.txt", "x\n");
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], false);
    assert_eq!(read(&repo, "altro.txt").as_deref(), Some("x\n"));
}

#[test]
fn detached_head_is_refused() {
    let (repo, main, _) = picking_repo();
    repo.detach(main);
    let (r, mut s) = open(&repo);
    for id in ["commit.cherry_pick", "commit.revert"] {
        let err = r.run(id, &mut s, json!({ "rev": "altro" })).unwrap_err();
        assert!(err.to_string().contains("HEAD è staccata"), "{id}: {err}");
    }
    assert_eq!(repo.rev("HEAD"), main);
    assert!(clean(&repo));
}

#[test]
fn failing_signature_leaves_it_in_progress() {
    let (repo, main, _) = picking_repo();
    repo.git(&["config", "commit.gpgSign", "true"]);
    repo.git(&["config", "gpg.program", "false"]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!([]));
    assert!(out["message"].as_str().unwrap().contains("gpg"), "{out}");
    assert_eq!(repo.rev("main"), main);
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    // Fallito dopo il backup: il backup resta.
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), main);
    // Con la firma ancora rotta il continue riporta l'errore di git e resta in corso.
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("gpg"), "{err}");
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    repo.git(&["config", "commit.gpgSign", "false"]);
    run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(parents(&repo, "main"), [main]);
    assert_eq!(repo.git(&["log", "-1", "--format=%an", "main"]), "Cleo");
    assert!(!has(&repo, "CHERRY_PICK_HEAD"));
}

#[test]
fn a_sequence_started_from_the_terminal_stays_there() {
    let (repo, main, _) = conflicting_repo();
    repo.checkout("altro");
    let p2 = repo.commit_file("z.txt", "z\n", "z");
    repo.checkout("main");
    repo.git_fails(&["cherry-pick", "altro~1", &p2.to_string()]);
    assert!(has(&repo, "sequencer"));
    let (r, mut s) = open(&repo);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["continue"], Value::Null);
    assert_eq!(op["abort"], Value::Null);
    for (id, params) in [
        ("cherry_pick.continue", json!({})),
        ("cherry_pick.abort", json!({ "confirm": true })),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
        assert!(err.to_string().contains("terminale"), "{id}: {err}");
    }
    assert!(has(&repo, "CHERRY_PICK_HEAD"));
    assert!(backups(&repo, "cherry-pick-abort").is_empty());
    repo.git(&["cherry-pick", "--abort"]);
    assert_eq!(repo.rev("main"), main);
}

#[test]
fn rerere_auto_update_does_not_hide_the_conflict() {
    let (repo, main, _) = conflicting_repo();
    repo.git(&["config", "rerere.enabled", "true"]);
    repo.git(&["config", "rerere.autoUpdate", "true"]);
    let (r, mut s) = open(&repo);
    run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    let shown = run(&r, &mut s, "conflict.show", json!({ "path": "a.txt" }));
    run(
        &r,
        &mut s,
        "conflict.resolve",
        json!({ "path": "a.txt", "digest": shown["digest"], "content": "unita\n" }),
    );
    run(&r, &mut s, "cherry_pick.continue", json!({}));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), main);
    // Di nuovo: rerere rimette la risoluzione nel file, ma il conflitto resta da confermare.
    let out = run(&r, &mut s, "commit.cherry_pick", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("unita\n"));
    let err = r
        .run("cherry_pick.continue", &mut s, json!({}))
        .unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    run(&r, &mut s, "cherry_pick.continue", json!({}));
    assert_eq!(parents(&repo, "main"), [main]);
}

#[test]
fn assume_unchanged_files_and_option_like_revs_are_refused() {
    let repo = Repo::new();
    repo.commit_file("a.txt", "base\n", "base");
    repo.branch("x");
    repo.checkout("x");
    repo.commit_file("a.txt", "x\n", "cambia a");
    repo.checkout("main");
    let main = repo.commit_file("c.txt", "c\n", "aggiunge c");
    repo.git(&["update-index", "--assume-unchanged", "a.txt"]);
    repo.write("a.txt", "nascosto\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run("commit.cherry_pick", &mut s, json!({ "rev": "x" }))
        .unwrap_err();
    assert!(err.to_string().contains("assume-unchanged"), "{err}");
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("nascosto\n"));
    assert_eq!(repo.rev("main"), main);
    assert!(backups(&repo, "cherry-pick").is_empty());

    for rev in ["--hard", "-x"] {
        let err = r
            .run("commit.cherry_pick", &mut s, json!({ "rev": rev }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{rev}: {err}");
    }
}
