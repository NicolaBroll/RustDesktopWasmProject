//! Test di integrazione dei comandi `branch.*` (S24) con il `CliBackend` su repo veri:
//! creazione, checkout (anche rifiutato per modifiche locali, con i file intatti), eliminazione
//! (con conferma se non unito, backup, undo e redo), rinomina e undo; nomi che iniziano con `-`
//! (anche un branch chiamato `--force`), worktree aggiuntivi.

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

/// Tutti i ref con il valore e HEAD, per controllare che niente cambi.
fn refs(repo: &Repo) -> String {
    let head = repo.git_output(&["symbolic-ref", "-q", "HEAD"]).stdout;
    format!(
        "{}\nHEAD -> {}{}",
        repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"]),
        String::from_utf8_lossy(&head).trim(),
        repo.git(&["rev-parse", "-q", "--verify", "HEAD"])
    )
}

/// Il valore del ref `name` (nome completo), `None` se non esiste.
fn ref_value(repo: &Repo, name: &str) -> Option<Oid> {
    let out = repo.git_output(&["rev-parse", "-q", "--verify", "--end-of-options", name]);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().parse().unwrap())
}

fn head_branch(repo: &Repo) -> String {
    repo.git(&["symbolic-ref", "HEAD"])
}

/// Indice, `git status` e contenuto dei file indicati: tutto quello che un checkout rifiutato
/// non deve toccare.
fn snapshot(repo: &Repo, files: &[&str]) -> (String, String, String, Vec<Option<Vec<u8>>>) {
    (
        refs(repo),
        repo.git(&["ls-files", "-s"]),
        repo.git(&["status", "--porcelain=v1", "--ignored", "-uall"]),
        files
            .iter()
            .map(|f| std::fs::read(repo.path().join(f)).ok())
            .collect(),
    )
}

/// `main` con `a.txt`, e `altro` (un commit dopo) che cambia `a.txt` e aggiunge `b.txt`; HEAD
/// su `main`.
fn two_branches() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    let main = repo.commit_file("a.txt", "a\n", "primo");
    repo.branch("altro");
    repo.checkout("altro");
    repo.write("b.txt", "b\n");
    repo.stage(&["b.txt"]);
    let altro = repo.commit_file("a.txt", "a su altro\n", "altro");
    repo.checkout("main");
    (repo, main, altro)
}

#[test]
fn create_from_head_and_from_a_rev_never_overwrites() {
    let (repo, main, altro) = two_branches();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.create", json!({ "name": "nuovo" }));
    assert_eq!(out["branch"], "refs/heads/nuovo");
    assert_eq!(out["target"], main.to_string());
    assert!(
        out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "refs/heads/nuovo")
    );
    assert_eq!(ref_value(&repo, "refs/heads/nuovo"), Some(main));
    assert_eq!(head_branch(&repo), "refs/heads/main");
    // Reflog con il nostro messaggio.
    let log = repo.git(&["reflog", "show", "--format=%gs", "refs/heads/nuovo"]);
    assert!(log.starts_with("project: branch.create nuovo"), "{log}");

    let out = run(
        &r,
        &mut s,
        "branch.create",
        json!({ "name": "da-altro", "start": "altro" }),
    );
    assert_eq!(out["target"], altro.to_string());

    // Un ref simbolico verso un ref che non c'è: non si segue (si creerebbe `refs/heads/target`).
    repo.git(&["symbolic-ref", "refs/heads/dang", "refs/heads/target"]);
    // Esistente, contenuto in un altro (`altro/x`), che ne contiene uno (`nuovo` → `nuovo`
    // esiste), rev sconosciuta: niente cambia.
    let before = refs(&repo);
    for (params, kind) in [
        (json!({ "name": "altro" }), "NotAllowed"),
        (json!({ "name": "altro/x" }), "NotAllowed"),
        (json!({ "name": "dang" }), "NotAllowed"),
        (json!({ "name": "x", "start": "nessuno" }), "UnknownRev"),
        (json!({ "name": "-x" }), "InvalidParams"),
        (json!({ "name": "--force" }), "InvalidParams"),
        (json!({ "name": "@" }), "InvalidParams"),
        (json!({ "name": "a b" }), "InvalidParams"),
    ] {
        let err = r.run("branch.create", &mut s, params.clone()).unwrap_err();
        let got = match err {
            EngineError::NotAllowed(_) => "NotAllowed",
            EngineError::Git { .. } => "Git",
            EngineError::UnknownRev(_) => "UnknownRev",
            EngineError::InvalidParams { .. } => "InvalidParams",
            ref other => panic!("{params}: {other}"),
        };
        assert_eq!(got, kind, "{params}: {err}");
    }
    assert_eq!(refs(&repo), before);
    assert_eq!(ref_value(&repo, "refs/heads/target"), None);
    assert_eq!(
        repo.git(&["symbolic-ref", "refs/heads/dang"]),
        "refs/heads/target"
    );

    // Undo elimina il branch, redo lo ricrea.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/da-altro"), None);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/da-altro"), Some(altro));
}

#[test]
fn checkout_and_undo() {
    let (repo, main, altro) = two_branches();
    // Una modifica che non tocca i file diversi resta.
    repo.write("c.txt", "non tracciato\n");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "altro" }));
    assert_eq!(out["branch"], "refs/heads/altro");
    assert_eq!(out["previous"], json!({ "branch": "refs/heads/main" }));
    assert_eq!(out["changed"], true);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["status"]["head"]["Branch"]["name"], "altro");
    assert_eq!(head_branch(&repo), "refs/heads/altro");
    assert_eq!(repo.rev("HEAD"), altro);
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "a su altro\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.path().join("c.txt")).unwrap(),
        "non tracciato\n"
    );

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "branch.checkout");
    assert_eq!(head_branch(&repo), "refs/heads/main");
    assert_eq!(repo.rev("HEAD"), main);
    assert!(!repo.path().join("b.txt").exists());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(head_branch(&repo), "refs/heads/altro");

    // Undo rifiutato se HEAD è cambiata da fuori.
    repo.checkout("main");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(head_branch(&repo), "refs/heads/main");
}

#[test]
fn checkout_from_detached_head_is_undone_to_the_same_commit() {
    let (repo, main, altro) = two_branches();
    repo.detach(main);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.checkout", json!({ "name": "altro" }));
    assert_eq!(out["previous"], json!({ "detached": main.to_string() }));
    assert_eq!(repo.rev("HEAD"), altro);
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        !repo
            .git_output(&["symbolic-ref", "-q", "HEAD"])
            .status
            .success()
    );
    assert_eq!(repo.rev("HEAD"), main);
}

#[test]
fn checkout_refused_by_local_changes_leaves_everything_intact() {
    let files = ["a.txt", "b.txt", "ign.txt"];

    // Modifica fuori stage, poi in stage, a un file diverso nel branch di destinazione.
    for staged in [false, true] {
        let (repo, _, _) = two_branches();
        repo.write("a.txt", "mia modifica\n");
        if staged {
            repo.stage(&["a.txt"]);
        }
        let before = snapshot(&repo, &files);
        let (r, mut s) = open(&repo);
        let err = r
            .run("branch.checkout", &mut s, json!({ "name": "altro" }))
            .unwrap_err();
        let EngineError::CheckoutConflict {
            target,
            files: hit,
            stderr,
        } = &err
        else {
            panic!("{err}");
        };
        assert_eq!(target, "altro");
        assert_eq!(hit, &["a.txt"], "{stderr}");
        assert!(err.to_string().contains("Niente modificato"), "{err}");
        assert_eq!(snapshot(&repo, &files), before);
        assert!(!s.journal().unwrap().can_undo());
    }

    // Un non tracciato e un ignorato che il branch di destinazione contiene.
    let (repo, _, _) = two_branches();
    repo.checkout("altro");
    repo.commit_file("ign.txt", "versione del branch\n", "ign");
    repo.checkout("main");
    std::fs::write(repo.path().join(".git/info/exclude"), "ign.txt\n").unwrap();
    repo.write("ign.txt", "mio, ignorato\n");
    repo.write("b.txt", "mio, non tracciato\n");
    let before = snapshot(&repo, &files);
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "altro" }))
        .unwrap_err();
    let EngineError::CheckoutConflict { files: hit, .. } = &err else {
        panic!("{err}");
    };
    assert_eq!(hit, &["b.txt", "ign.txt"]);
    assert_eq!(snapshot(&repo, &files), before);
}

#[test]
fn checkout_refused_during_a_merge_or_with_assume_unchanged_files() {
    let (repo, _, _) = two_branches();
    repo.branch("terzo");
    repo.commit_file("a.txt", "a su main\n", "main");
    repo.conflict("altro");
    let before = snapshot(&repo, &["a.txt"]);
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "terzo" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(snapshot(&repo, &["a.txt"]), before);

    // `--assume-unchanged`: git sovrascriverebbe la modifica senza dirlo.
    let (repo, _, _) = two_branches();
    repo.git(&["update-index", "--assume-unchanged", "a.txt"]);
    repo.write("a.txt", "modifica nascosta\n");
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "altro" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "modifica nascosta\n"
    );
    assert_eq!(head_branch(&repo), "refs/heads/main");
}

#[test]
fn names_starting_with_a_dash() {
    let (repo, main, altro) = two_branches();
    // Branch che git non lascerebbe creare, ma che possono esistere.
    repo.git(&["update-ref", "refs/heads/-x", &altro.to_string()]);
    repo.git(&["update-ref", "refs/heads/--force", &altro.to_string()]);
    repo.git(&["update-ref", "refs/heads/-d", &altro.to_string()]);
    let (r, mut s) = open(&repo);

    // Un branch `--force` non forza niente: la modifica locale blocca il checkout.
    repo.write("a.txt", "mia modifica\n");
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "--force" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::CheckoutConflict { .. }), "{err}");
    assert_eq!(
        std::fs::read_to_string(repo.path().join("a.txt")).unwrap(),
        "mia modifica\n"
    );
    assert_eq!(head_branch(&repo), "refs/heads/main");
    repo.git(&["checkout", "--", "a.txt"]);

    run(&r, &mut s, "branch.checkout", json!({ "name": "-x" }));
    assert_eq!(head_branch(&repo), "refs/heads/-x");
    run(&r, &mut s, "branch.checkout", json!({ "name": "main" }));

    // `-d` non diventa "elimina": si elimina solo `-d`.
    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "-d", "confirm": true }),
    );
    assert_eq!(ref_value(&repo, "refs/heads/-d"), None);
    assert_eq!(ref_value(&repo, "refs/heads/main"), Some(main));
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(ref_value(&repo, backup), Some(altro));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/-d"), Some(altro));

    // Rinomina di `--force` in un nome valido.
    run(
        &r,
        &mut s,
        "branch.rename",
        json!({ "name": "--force", "new_name": "forzato" }),
    );
    assert_eq!(ref_value(&repo, "refs/heads/--force"), None);
    assert_eq!(ref_value(&repo, "refs/heads/forzato"), Some(altro));

    // Un nome nuovo con `-` è rifiutato prima di arrivare a git.
    let before = refs(&repo);
    for (id, params) in [
        ("branch.create", json!({ "name": "-n" })),
        ("branch.rename", json!({ "name": "-x", "new_name": "-y" })),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{id}: {err}"
        );
    }
    assert_eq!(refs(&repo), before);
}

#[test]
fn delete_unmerged_needs_confirm_and_undo_recreates() {
    let (repo, main, altro) = two_branches();
    repo.git(&["config", "branch.altro.description", "da tenere?"]);
    let (r, mut s) = open(&repo);
    let before = refs(&repo);
    let err = r
        .run("branch.delete", &mut s, json!({ "name": "altro" }))
        .unwrap_err();
    let EngineError::ConfirmRequired { command, reason } = &err else {
        panic!("{err}");
    };
    assert_eq!(*command, "branch.delete");
    assert!(reason.contains("1 commit"), "{reason}");
    assert_eq!(refs(&repo), before);
    assert!(!s.journal().unwrap().can_undo());

    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "altro", "confirm": true }),
    );
    assert_eq!(out["target"], altro.to_string());
    assert_eq!(out["unmerged"], 1);
    assert_eq!(out["warnings"], json!([]));
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(
        backup.starts_with("refs/project/backup/delete-"),
        "{backup}"
    );
    assert_eq!(ref_value(&repo, &backup), Some(altro));
    assert_eq!(ref_value(&repo, "refs/heads/altro"), None);
    // Come `git branch -d`: via la configurazione del branch.
    assert!(
        !repo
            .git_output(&["config", "--get", "branch.altro.description"])
            .status
            .success()
    );
    // Il backup resiste al gc.
    repo.git(&["gc", "--quiet", "--prune=now"]);
    assert_eq!(repo.git(&["cat-file", "-t", &altro.to_string()]), "commit");

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "branch.delete");
    assert_eq!(out["backups"], json!([]));
    assert_eq!(ref_value(&repo, "refs/heads/altro"), Some(altro));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/altro"), None);

    // Ricreato da fuori su un altro commit: l'undo non lo sovrascrive.
    repo.git(&["branch", "altro", &main.to_string()]);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(ref_value(&repo, "refs/heads/altro"), Some(main));
}

#[test]
fn delete_merged_current_and_open_in_another_worktree() {
    let (repo, main, _) = two_branches();
    repo.branch("unito");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.delete", json!({ "name": "unito" }));
    assert_eq!(out["unmerged"], 0);
    assert_eq!(ref_value(&repo, "refs/heads/unito"), None);
    assert_eq!(
        ref_value(&repo, out["backup"].as_str().unwrap()),
        Some(main)
    );

    let before = refs(&repo);
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "main", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "nessuno", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(refs(&repo), before);

    // `altro` aperto in un altro worktree: rifiutato, niente cambia (nemmeno un backup).
    let wt = tempfile::tempdir().unwrap();
    let wt_path = wt.path().join("wt");
    repo.git(&[
        "worktree",
        "add",
        "--quiet",
        wt_path.to_str().unwrap(),
        "altro",
    ]);
    let before_wt = refs(&repo);
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "altro", "confirm": true }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("worktree")),
        "{err}"
    );
    assert_eq!(refs(&repo), before_wt);

    // Nemmeno il redo di un'eliminazione cancella un branch aperto altrove.
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/unito"), Some(main));
    let wt2 = wt.path().join("wt2");
    repo.git(&["worktree", "add", "--quiet", wt2.to_str().unwrap(), "unito"]);
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains("worktree")),
        "{err}"
    );
    assert_eq!(ref_value(&repo, "refs/heads/unito"), Some(main));
}

#[test]
fn delete_merged_into_its_upstream_needs_no_confirm() {
    let (repo, _, altro) = two_branches();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.push("origin", "altro", "altro");
    repo.set_upstream("altro", "origin");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "branch.delete", json!({ "name": "altro" }));
    assert_eq!(out["unmerged"], 0);
    assert_eq!(ref_value(&repo, "refs/remotes/origin/altro"), Some(altro));
}

#[test]
fn rename_and_undo() {
    let (repo, main, _) = two_branches();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.push("origin", "main", "main");
    repo.set_upstream("main", "origin");
    let reflog_before = repo.git(&["reflog", "show", "--format=%H", "refs/heads/main"]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.rename",
        json!({ "name": "main", "new_name": "principale" }),
    );
    assert_eq!(out["branch"], "refs/heads/principale");
    assert_eq!(out["previous"], "refs/heads/main");
    assert_eq!(out["target"], main.to_string());
    assert_eq!(out["head"], true);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(head_branch(&repo), "refs/heads/principale");
    assert_eq!(ref_value(&repo, "refs/heads/main"), None);
    assert_eq!(ref_value(&repo, "refs/heads/principale"), Some(main));
    // Upstream e reflog seguono il branch.
    assert_eq!(
        repo.git(&["config", "branch.principale.merge"]),
        "refs/heads/main"
    );
    let reflog = repo.git(&["reflog", "show", "--format=%H", "refs/heads/principale"]);
    assert!(reflog.ends_with(&reflog_before), "{reflog}");

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["undone"]["command"], "branch.rename");
    assert_eq!(head_branch(&repo), "refs/heads/main");
    assert_eq!(ref_value(&repo, "refs/heads/main"), Some(main));
    assert_eq!(ref_value(&repo, "refs/heads/principale"), None);
    assert_eq!(repo.git(&["config", "branch.main.remote"]), "origin");
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(head_branch(&repo), "refs/heads/principale");

    // Rifiuti senza cambiare niente: nome già preso, branch assente.
    let before = refs(&repo);
    for params in [
        json!({ "name": "principale", "new_name": "altro" }),
        json!({ "name": "nessuno", "new_name": "x" }),
    ] {
        let err = r.run("branch.rename", &mut s, params.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
    }
    assert_eq!(refs(&repo), before);

    // Il vecchio nome ripreso da fuori: l'undo è rifiutato.
    repo.git(&["branch", "main", &main.to_string()]);
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(head_branch(&repo), "refs/heads/principale");
}

#[test]
fn rename_of_another_branch_keeps_head() {
    let (repo, _, altro) = two_branches();
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.rename",
        json!({ "name": "altro", "new_name": "feature/altro" }),
    );
    assert_eq!(out["head"], false);
    assert_eq!(head_branch(&repo), "refs/heads/main");
    assert_eq!(ref_value(&repo, "refs/heads/feature/altro"), Some(altro));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/altro"), Some(altro));
    assert_eq!(ref_value(&repo, "refs/heads/feature/altro"), None);
}

#[test]
fn checkout_refused_when_a_folder_with_untracked_files_would_become_a_file() {
    let (repo, _, _) = two_branches();
    repo.checkout("altro");
    repo.commit_file("d", "file d nel branch\n", "d");
    repo.checkout("main");
    repo.write("d/x", "mio, non tracciato\n");
    let before = snapshot(&repo, &["d/x"]);
    let (r, mut s) = open(&repo);
    let err = r
        .run("branch.checkout", &mut s, json!({ "name": "altro" }))
        .unwrap_err();
    let EngineError::CheckoutConflict { files, stderr, .. } = &err else {
        panic!("{err}");
    };
    assert!(stderr.contains("would lose untracked files"), "{stderr}");
    assert!(files.iter().any(|f| f.starts_with('d')), "{files:?}");
    assert_eq!(snapshot(&repo, &["d/x"]), before);
}

#[test]
fn delete_saves_the_branch_config_and_undo_rewrites_it() {
    let (repo, _, altro) = two_branches();
    let remote = Repo::bare();
    repo.add_remote("origin", &remote);
    repo.push("origin", "main", "altro-remoto");
    repo.git(&["config", "branch.altro.remote", "origin"]);
    repo.git(&["config", "branch.altro.merge", "refs/heads/altro-remoto"]);
    repo.git(&[
        "config",
        "branch.altro.description",
        "-inizia con un trattino",
    ]);
    let config = |repo: &Repo| {
        let out = repo.git_output(&["config", "--local", "--get-regexp", "^branch\\.altro\\."]);
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let saved = config(&repo);
    assert_eq!(saved.lines().count(), 3, "{saved}");
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "altro", "confirm": true }),
    );
    assert_eq!(
        out["removed_config"],
        json!([
            { "key": "branch.altro.remote", "value": "origin" },
            { "key": "branch.altro.merge", "value": "refs/heads/altro-remoto" },
            { "key": "branch.altro.description", "value": "-inizia con un trattino" }
        ])
    );
    assert_eq!(config(&repo), "");
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"][0]["branch_config"], out["removed_config"]);

    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(ref_value(&repo, "refs/heads/altro"), Some(altro));
    assert_eq!(config(&repo), saved);
    // L'upstream funziona di nuovo.
    let refs = run(&r, &mut s, "repo.refs", json!({}));
    let branch = refs
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "refs/heads/altro")
        .unwrap();
    assert_eq!(
        branch["upstream"]["name"],
        "refs/remotes/origin/altro-remoto"
    );

    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(config(&repo), "");

    // Una configurazione nuova comparsa nel frattempo non si sovrascrive: avviso.
    repo.git(&["config", "branch.altro.description", "nuova"]);
    let out = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/altro"), Some(altro));
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(
        repo.git(&["config", "--get-all", "branch.altro.description"]),
        "nuova"
    );
    // E il redo non la toglie.
    let out = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(ref_value(&repo, "refs/heads/altro"), None);
    assert_eq!(out["warnings"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(repo.git(&["config", "branch.altro.description"]), "nuova");
}

#[test]
fn delete_with_expected_refuses_a_branch_that_moved() {
    let (repo, main, altro) = two_branches();
    let (r, mut s) = open(&repo);
    // Visto su `altro`, poi spostato da fuori.
    repo.git(&["branch", "-f", "altro", &main.to_string()]);
    let before = refs(&repo);
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "altro", "confirm": true, "expected": altro.to_string() }),
        )
        .unwrap_err();
    assert!(
        matches!(&err, EngineError::NotAllowed(m) if m.contains(&main.to_string())),
        "{err}"
    );
    assert_eq!(refs(&repo), before);
    assert!(!s.journal().unwrap().can_undo());
    let err = r
        .run(
            "branch.delete",
            &mut s,
            json!({ "name": "altro", "expected": &main.to_string()[..7] }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

    let out = run(
        &r,
        &mut s,
        "branch.delete",
        json!({ "name": "altro", "expected": main.to_string() }),
    );
    assert_eq!(out["target"], main.to_string());
    assert_eq!(ref_value(&repo, "refs/heads/altro"), None);
}

#[test]
fn create_does_not_follow_a_dangling_symref() {
    let (repo, _, _) = two_branches();
    repo.git(&["symbolic-ref", "refs/heads/dang", "refs/heads/target"]);
    let (r, mut s) = open(&repo);
    let before = refs(&repo);
    let err = r
        .run("branch.create", &mut s, json!({ "name": "dang" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(refs(&repo), before);
    assert_eq!(ref_value(&repo, "refs/heads/target"), None);
    assert_eq!(
        repo.git(&["symbolic-ref", "refs/heads/dang"]),
        "refs/heads/target"
    );
    assert!(!s.journal().unwrap().can_undo());
}
