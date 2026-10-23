//! Test di integrazione di `merge.start`, `merge.continue`, `merge.abort` e `repo.operation`
//! (S29) con il `CliBackend`: fast-forward, commit di merge, conflitto che resta in corso (lo
//! scenario di S07, `repo_with_merge_conflict`, e uno avviato da qui), continue con conflitti
//! rifiutato, risoluzione e continue, abort con e senza conferma e il suo backup, modifiche
//! locali, file non tracciati o ignorati al posto di quelli in arrivo, HEAD staccata, undo e
//! redo, nomi che iniziano con `-`, rebase e cherry-pick in corso visti da `repo.operation`.

mod common;

use common::{Repo, repo_with_divergent_branches, repo_with_merge_conflict};
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

fn merging(repo: &Repo) -> bool {
    repo.path().join(".git/MERGE_HEAD").exists()
}

fn parents(repo: &Repo, rev: &str) -> Vec<Oid> {
    repo.git(&["rev-list", "--parents", "-n", "1", rev])
        .split_whitespace()
        .skip(1)
        .map(|h| h.parse().unwrap())
        .collect()
}

/// `main` su `c1` (`a.txt`), `avanti` un commit dopo (`b.txt`): un fast-forward.
fn ahead_branch() -> (Repo, Oid, Oid) {
    let repo = Repo::new();
    let c1 = repo.commit_file("a.txt", "uno\n", "primo");
    repo.branch("avanti");
    repo.checkout("avanti");
    let c2 = repo.commit_file("b.txt", "b\n", "secondo");
    repo.checkout("main");
    (repo, c1, c2)
}

#[test]
fn fast_forward_undo_and_redo() {
    let (repo, c1, c2) = ahead_branch();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "avanti" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(out["before"], c1.to_string());
    assert_eq!(out["after"], c2.to_string());
    assert_eq!(out["in_progress"], false);
    assert_eq!(repo.rev("main"), c2);
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("b\n"));
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.rev(backup), c1);
    // Il reflog dice chi ha spostato il branch.
    let reflog = repo.git(&["reflog", "-1", "--format=%gs", "main"]);
    assert!(reflog.starts_with("project: merge.start"), "{reflog}");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c1);
    assert!(read(&repo, "b.txt").is_none());
    assert!(clean(&repo));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), c2);
    assert!(clean(&repo));

    // Già contenuto: niente da fare, niente backup né diario.
    let before = backups(&repo, "merge").len();
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "avanti" }));
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(backups(&repo, "merge").len(), before);
}

#[test]
fn merge_commit_with_default_and_custom_message() {
    let (repo, c1, c2) = ahead_branch();
    // `main` va avanti per conto suo: divergenti, senza conflitti.
    let m1 = repo.commit_file("c.txt", "c\n", "su main");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "avanti" }));
    assert_eq!(out["strategy"], "merge");
    let merge: Oid = out["after"].as_str().unwrap().parse().unwrap();
    assert_eq!(parents(&repo, "HEAD"), [m1, c2]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "HEAD"]),
        "Merge branch 'avanti'"
    );
    assert!(clean(&repo));
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("b\n"));
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), m1);

    // Undo: il branch torna indietro con indice e worktree; il commit di merge resta in un
    // backup.
    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), m1);
    assert!(read(&repo, "b.txt").is_none());
    assert!(clean(&repo));
    let kept = undo["backups"][0].as_str().unwrap();
    assert_eq!(repo.rev(kept), merge);
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), merge);
    run(&r, &mut s, "edit.undo", json!({}));

    // `no_ff` e un messaggio scelto, su un branch che non è `main`.
    repo.branch("dev");
    repo.checkout("dev");
    repo.git(&["reset", "--quiet", "--hard", &c1.to_string()]);
    let (r, mut s) = open(&repo);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "avanti", "no_ff": true, "message": "Unisci\n\ncon corpo" }),
    );
    assert_eq!(out["strategy"], "merge");
    assert_eq!(parents(&repo, "dev"), [c1, c2]);
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "dev"]),
        "Unisci\n\ncon corpo"
    );
    run(&r, &mut s, "edit.undo", json!({}));
    // Il messaggio di default con ` into <branch>`.
    run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "avanti", "no_ff": true }),
    );
    assert_eq!(
        repo.git(&["log", "-1", "--format=%s", "dev"]),
        "Merge branch 'avanti' into dev"
    );
}

#[test]
fn ff_only_refuses_divergent_branches() {
    let (repo, theirs) = repo_with_divergent_branches();
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let err = r
        .run(
            "merge.start",
            &mut s,
            json!({ "rev": "altro", "ff_only": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(repo.rev("HEAD"), head);
    assert!(backups(&repo, "merge").is_empty());
    assert_ne!(theirs, head);
}

#[test]
fn conflict_stays_in_progress_then_continue_and_undo() {
    let (repo, theirs) = repo_with_divergent_branches();
    let ours = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "altro" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(out["after"], ours.to_string());
    assert_eq!(out["can_undo"], false);
    assert_eq!(out["status"]["operation"], "Merge");
    assert!(
        out["message"]
            .as_str()
            .unwrap()
            .contains("Merge conflict in a.txt")
    );
    assert!(merging(&repo));
    assert!(read(&repo, "a.txt").unwrap().contains("<<<<<<<"));
    // Il backup resta: il merge è in corso.
    assert_eq!(repo.rev(out["backup"].as_str().unwrap()), ours);

    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Merge");
    assert_eq!(op["conflicts"], json!(["a.txt"]));
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["incoming"][0]["label"], "altro");
    assert_eq!(op["incoming"][0]["summary"], "a.txt su altro");
    assert_eq!(op["message"], "Merge branch 'altro'");
    assert_eq!(op["ready"], false);
    assert_eq!(op["continue"], "merge.continue");
    assert_eq!(op["abort"], "merge.abort");

    // Con il merge in corso: niente undo, niente altro merge, niente continue con conflitti.
    for (id, params) in [
        ("edit.undo", json!({})),
        ("merge.start", json!({ "rev": "altro" })),
        ("merge.continue", json!({})),
    ] {
        let err = r.run(id, &mut s, params).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{id}: {err}");
    }
    let err = r.run("merge.continue", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert!(merging(&repo));

    repo.write("a.txt", "risolto\n");
    run(&r, &mut s, "index.stage", json!({ "paths": ["a.txt"] }));
    assert_eq!(run(&r, &mut s, "repo.operation", json!({}))["ready"], true);
    let out = run(&r, &mut s, "merge.continue", json!({}));
    // Il messaggio di `MERGE_MSG` senza il blocco `# Conflicts:`.
    assert_eq!(out["message"], "Merge branch 'altro'");
    assert_eq!(
        repo.git(&["log", "-1", "--format=%B", "HEAD"]),
        "Merge branch 'altro'"
    );
    assert_eq!(parents(&repo, "HEAD"), [ours, theirs]);
    assert!(!merging(&repo));
    assert!(clean(&repo));
    assert_eq!(out["status"]["operation"], Value::Null);
    let merge = repo.rev("HEAD");

    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), ours);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("nostra\n"));
    assert!(clean(&repo));
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(repo.rev("main"), merge);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("risolto\n"));
}

#[test]
fn scenario_of_s07_from_the_terminal() {
    // Il merge in conflitto avviato da terminale (S07): stato, banner, continue con un
    // messaggio scelto.
    let repo = repo_with_merge_conflict();
    let ours = repo.rev("HEAD");
    let theirs = repo.rev("altro");
    let (r, mut s) = open(&repo);
    assert_eq!(
        run(&r, &mut s, "repo.status", json!({}))["operation"],
        "Merge"
    );
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["incoming"][0]["refs"], json!(["refs/heads/altro"]));
    assert_eq!(op["conflicts"], json!(["a.txt"]));

    repo.write("a.txt", "tutte e due\n");
    repo.stage(&["a.txt"]);
    let out = run(
        &r,
        &mut s,
        "merge.continue",
        json!({ "message": "Unisci altro a mano" }),
    );
    assert_eq!(out["commit"]["summary"], "Unisci altro a mano");
    assert_eq!(parents(&repo, "HEAD"), [ours, theirs]);
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Merge di altro")
    );
}

#[test]
fn abort_needs_confirm_and_keeps_a_backup() {
    let repo = repo_with_merge_conflict();
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    // Una risoluzione a metà e un file nuovo in stage: andrebbero persi.
    repo.write("a.txt", "risoluzione a metà\n");
    repo.write("nuovo.txt", "nuovo\n");
    repo.stage(&["nuovo.txt"]);
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    match &err {
        EngineError::ConfirmRequired { reason, .. } => {
            assert!(reason.contains("`a.txt`"), "{reason}");
            assert!(reason.contains("`nuovo.txt`"), "{reason}");
        }
        other => panic!("{other}"),
    }
    assert!(merging(&repo));
    assert!(backups(&repo, "merge-abort").is_empty());

    let out = run(&r, &mut s, "merge.abort", json!({ "confirm": true }));
    assert!(!merging(&repo));
    assert!(clean(&repo));
    assert_eq!(repo.rev("HEAD"), head);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("nostra\n"));
    assert_eq!(out["status"]["operation"], Value::Null);
    assert_eq!(out["can_undo"], false);
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(backups(&repo, "merge-abort"), [backup]);
    // Il backup è come uno stash: worktree (genitori HEAD e l'indice), con i file com'erano.
    assert_eq!(
        repo.git(&["show", &format!("{backup}:a.txt")]),
        "risoluzione a metà"
    );
    assert_eq!(repo.git(&["show", &format!("{backup}:nuovo.txt")]), "nuovo");
    let p = parents(&repo, backup);
    assert_eq!(p[0], head);
    assert_eq!(repo.git(&["show", &format!("{}:nuovo.txt", p[1])]), "nuovo");
    // Nell'indice salvato il file in conflitto ha la versione nostra.
    assert_eq!(repo.git(&["show", &format!("{}:a.txt", p[1])]), "nostra");
    // `git stash apply` lo rimette.
    repo.git(&["stash", "apply", backup]);
    assert_eq!(
        read(&repo, "a.txt").as_deref(),
        Some("risoluzione a metà\n")
    );
    assert_eq!(read(&repo, "nuovo.txt").as_deref(), Some("nuovo\n"));

    // Senza merge in corso.
    let err = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(err.to_string().contains("non c'è un merge"), "{err}");
}

#[test]
fn abort_refused_by_git_keeps_the_backup() {
    let repo = repo_with_merge_conflict();
    let (r, mut s) = open(&repo);
    // Un file in stage con altre modifiche fuori stage: `git merge --abort` rifiuta.
    repo.write("nuovo.txt", "in stage\n");
    repo.stage(&["nuovo.txt"]);
    repo.write("nuovo.txt", "dopo\n");
    let err = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    assert!(err.to_string().contains("not uptodate"), "{err}");
    assert!(merging(&repo));
    assert_eq!(read(&repo, "nuovo.txt").as_deref(), Some("dopo\n"));
    // Il backup resta sempre dopo un abort fallito, e l'errore lo nomina.
    let kept = backups(&repo, "merge-abort");
    assert_eq!(kept.len(), 1);
    assert!(err.to_string().contains(&kept[0]), "{err}");
    assert!(!err.to_string().contains("niente modificato"), "{err}");
}

/// `git merge --abort` che fallisce a metà (una cartella in sola lettura): il backup resta e ha
/// la risoluzione che git potrebbe aver già cancellato.
#[cfg(unix)]
#[test]
fn abort_failing_halfway_keeps_the_backup() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    repo.commit_file("a/x.txt", "base\n", "base a");
    repo.commit_file("z/a.txt", "base\n", "base z");
    repo.branch("altro");
    repo.checkout("altro");
    repo.commit_file("a/x.txt", "loro\n", "a loro");
    repo.commit_file("z/a.txt", "loro\n", "z loro");
    repo.checkout("main");
    repo.commit_file("a/x.txt", "nostra\n", "a nostra");
    repo.commit_file("z/a.txt", "nostra\n", "z nostra");
    repo.conflict("altro");
    repo.write("a/x.txt", "risolto ma non in stage\n");
    let z = repo.path().join("z");
    std::fs::set_permissions(&z, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (r, mut s) = open(&repo);
    let result = r.run("merge.abort", &mut s, json!({ "confirm": true }));
    std::fs::set_permissions(&z, std::fs::Permissions::from_mode(0o755)).unwrap();
    let err = result.unwrap_err();
    let kept = backups(&repo, "merge-abort");
    assert_eq!(kept.len(), 1, "{err}");
    assert!(err.to_string().contains(&kept[0]), "{err}");
    assert!(!err.to_string().contains("niente modificato"), "{err}");
    assert_eq!(
        repo.git(&["show", &format!("{}:a/x.txt", kept[0])]),
        "risolto ma non in stage"
    );
}

/// Un conflitto «cancellato da noi» su un file ignorato: il backup lo salva comunque (`add -f`)
/// e l'abort riesce.
#[test]
fn abort_with_a_conflict_on_an_ignored_path() {
    let repo = Repo::new();
    repo.commit_file(".gitignore", "*.log\n", "ignora i log");
    repo.write("gen.log", "base\n");
    repo.git(&["add", "-f", "gen.log"]);
    repo.commit("gen.log tracciato");
    repo.branch("feat");
    repo.checkout("feat");
    repo.commit_file("gen.log", "feat\n", "modifica il log");
    repo.checkout("main");
    repo.git(&["rm", "--quiet", "gen.log"]);
    repo.commit("toglie il log");
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "feat" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["gen.log"]));
    let out = run(&r, &mut s, "merge.abort", json!({ "confirm": true }));
    assert!(!merging(&repo));
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(repo.git(&["show", &format!("{backup}:gen.log")]), "feat");
}

/// `branch.<b>.mergeOptions=--squash` non trasforma il fast-forward in uno squash.
#[test]
fn fast_forward_ignores_merge_options_squash() {
    let (repo, _, c2) = ahead_branch();
    repo.git(&["config", "branch.main.mergeOptions", "--squash"]);
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "avanti" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(repo.rev("main"), c2);
    assert!(clean(&repo));
    assert_eq!(out["can_undo"], true);
}

/// Un hook `post-merge` che sposta il branch: il merge è fatto, ma non entra nel diario.
#[cfg(unix)]
#[test]
fn branch_moved_during_the_merge_is_not_journaled() {
    use std::os::unix::fs::PermissionsExt;
    let (repo, _, c2) = ahead_branch();
    let hook = repo.path().join(".git/hooks/post-merge");
    std::fs::write(
        &hook,
        "#!/bin/sh\ngit commit --quiet --allow-empty -m altro\n",
    )
    .unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (r, mut s) = open(&repo);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "avanti" }));
    assert_ne!(repo.rev("main"), c2);
    assert_eq!(parents(&repo, "main"), [c2]);
    assert_eq!(out["can_undo"], false);
    assert!(
        out["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("non entra nel diario")),
        "{out}"
    );
}

/// Undo e redo di uno scarto sono rifiutati con un merge in corso.
#[test]
fn undo_of_a_discard_during_a_merge_is_refused() {
    let (repo, _) = repo_with_divergent_branches();
    repo.commit_file("c.txt", "c\n", "c su main");
    repo.write("c.txt", "modificato\n");
    let (r, mut s) = open(&repo);
    run(
        &r,
        &mut s,
        "worktree.discard",
        json!({ "paths": ["c.txt"], "confirm": true }),
    );
    repo.conflict("altro");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("un merge"), "{err}");
    assert_eq!(read(&repo, "c.txt").as_deref(), Some("c\n"));
}

#[test]
fn local_changes_and_files_in_the_way() {
    let (repo, _, _) = ahead_branch();
    repo.commit_file("c.txt", "c\n", "su main");
    let head = repo.rev("HEAD");
    let (r, mut s) = open(&repo);
    // Una modifica a un file tracciato: niente commit di merge (scelta), niente scritto.
    repo.write("a.txt", "modificato\n");
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "avanti" }))
        .unwrap_err();
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert!(err.to_string().contains("autostash"), "{err}");
    assert_eq!(repo.rev("HEAD"), head);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("modificato\n"));
    assert!(backups(&repo, "merge").is_empty());
    repo.git(&["checkout", "--", "a.txt"]);

    // Un file ignorato al posto di uno che il merge porta: rifiutato, niente scritto.
    repo.write(".git/info/exclude", "b.txt\n");
    repo.write("b.txt", "mio, ignorato\n");
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "avanti" }))
        .unwrap_err();
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mio, ignorato\n"));
    assert_eq!(repo.rev("HEAD"), head);
    assert!(backups(&repo, "merge").is_empty());
}

#[test]
fn fast_forward_with_local_changes() {
    let (repo, c1, c2) = ahead_branch();
    let (r, mut s) = open(&repo);
    // Una modifica che il fast-forward non tocca resta.
    repo.write("a.txt", "modificato\n");
    run(&r, &mut s, "merge.start", json!({ "rev": "avanti" }));
    assert_eq!(repo.rev("main"), c2);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("modificato\n"));
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c1);
    assert_eq!(read(&repo, "a.txt").as_deref(), Some("modificato\n"));

    // Un non tracciato al posto di un file in arrivo: git rifiuta, il backup si toglie.
    repo.write("b.txt", "mio\n");
    let before = backups(&repo, "merge").len();
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "avanti" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`b.txt`"), "{err}");
    assert_eq!(repo.rev("main"), c1);
    assert_eq!(read(&repo, "b.txt").as_deref(), Some("mio\n"));
    assert_eq!(backups(&repo, "merge").len(), before);
}

#[test]
fn detached_head_unknown_rev_and_dash_names() {
    let (repo, c1, c2) = ahead_branch();
    let (r, mut s) = open(&repo);
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "nessuno" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    // Un branch che inizia con `-` non diventa un'opzione.
    repo.git(&["update-ref", "refs/heads/-x", &c2.to_string()]);
    let out = run(&r, &mut s, "merge.start", json!({ "rev": "-x" }));
    assert_eq!(out["source"]["name"], "-x");
    assert_eq!(repo.rev("main"), c2);
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(repo.rev("main"), c1);
    let out = run(
        &r,
        &mut s,
        "merge.start",
        json!({ "rev": "-x", "no_ff": true }),
    );
    assert_eq!(out["strategy"], "merge");
    assert_eq!(repo.git(&["log", "-1", "--format=%s"]), "Merge branch '-x'");

    repo.detach(c1);
    let (r, mut s) = open(&repo);
    let err = r
        .run("merge.start", &mut s, json!({ "rev": "avanti" }))
        .unwrap_err();
    assert!(err.to_string().contains("HEAD è staccata"), "{err}");
    assert_eq!(repo.rev("HEAD"), c1);
}

#[test]
fn operation_of_a_rebase_and_a_cherry_pick() {
    let (repo, theirs) = repo_with_divergent_branches();
    let ours = repo.rev("main");
    repo.git_fails(&["rebase", "altro"]);
    let (r, mut s) = open(&repo);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "Rebase");
    assert_eq!(op["onto"], theirs.to_string());
    assert_eq!(op["rebase_branch"], "refs/heads/main");
    assert_eq!(op["conflicts"], json!(["a.txt"]));
    assert_eq!(op["continue"], Value::Null);
    assert_eq!(op["abort"], Value::Null);
    let err = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap_err();
    assert!(err.to_string().contains("un rebase"), "{err}");
    repo.git(&["rebase", "--abort"]);

    repo.git_fails(&["cherry-pick", "altro"]);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], "CherryPick");
    assert_eq!(op["incoming"][0]["id"], theirs.to_string());
    assert_eq!(op["incoming"][0]["label"], "altro");
    assert_eq!(repo.rev("main"), ours);

    repo.git(&["cherry-pick", "--abort"]);
    let op = run(&r, &mut s, "repo.operation", json!({}));
    assert_eq!(op["operation"], Value::Null);
    assert_eq!(op["incoming"], json!([]));
    assert_eq!(op["message"], Value::Null);
    assert_eq!(op["ready"], false);
}
