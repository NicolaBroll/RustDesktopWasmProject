//! Test di integrazione di `remote.pull` (S26) con il `CliBackend` e un remoto locale (`git
//! init --bare`) su cui un secondo clone pubblica: fast-forward, merge, rebase secondo la
//! configurazione, conflitti (il merge resta in corso, il rebase si annulla), modifiche locali, branch senza upstream, undo e redo,
//! branch non attivo, nomi che iniziano con `-`.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{EngineError, PullOperation, Registry, Session};
use serde_json::{Value, json};
use std::sync::Arc;

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn pull(r: &Registry, s: &mut Session, params: Value) -> Value {
    r.run("remote.pull", s, params)
        .unwrap_or_else(|e| panic!("remote.pull: {e}"))
}

/// Il remoto bare, un clone che ci pubblica (`other`) e il repo sotto test (`work`): tutti e
/// due con `main` su `c1` (`a.txt`), che segue `origin/main`.
struct Setup {
    _remote: Repo,
    other: Repo,
    work: Repo,
    c1: Oid,
}

fn setup() -> Setup {
    let remote = Repo::bare();
    let other = Repo::new();
    other.add_remote("origin", &remote);
    let c1 = other.commit_file("a.txt", "uno\n", "primo");
    other.push("origin", "HEAD", "main");
    other.set_upstream("main", "origin");
    let work = Repo::new();
    work.add_remote("origin", &remote);
    work.git(&["fetch", "--quiet", "origin"]);
    work.git(&["reset", "--quiet", "--hard", "origin/main"]);
    work.set_upstream("main", "origin");
    Setup {
        _remote: remote,
        other,
        work,
        c1,
    }
}

/// `other` pubblica un commit su `main` che scrive `file`.
fn publish(other: &Repo, file: &str, content: &str, message: &str) -> Oid {
    let id = other.commit_file(file, content, message);
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    id
}

fn read(repo: &Repo, file: &str) -> Option<String> {
    std::fs::read_to_string(repo.path().join(file)).ok()
}

fn backups(repo: &Repo) -> Vec<String> {
    let out = repo.git(&[
        "for-each-ref",
        "--format=%(refname)",
        "refs/project/backup/",
    ]);
    out.lines().map(str::to_owned).collect()
}

fn clean(repo: &Repo) -> bool {
    repo.git(&["status", "--porcelain"]).is_empty()
}

#[test]
fn fast_forward_then_undo_and_redo() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "b.txt", "bi\n", "secondo");
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(out["strategy_source"], "default");
    assert_eq!(out["branch"], "main");
    assert_eq!(out["remote"], "origin");
    assert_eq!(out["upstream"], "refs/remotes/origin/main");
    assert_eq!(out["before"], c1.to_string());
    assert_eq!(out["after"], c2.to_string());
    assert_eq!(
        (out["ahead"].as_u64(), out["behind"].as_u64()),
        (Some(0), Some(1))
    );
    assert_eq!(
        out["fetch"]["updated"],
        json!([{ "name": "refs/remotes/origin/main", "before": c1.to_string(), "after": c2.to_string() }])
    );
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/pull-"), "{backup}");
    assert_eq!(work.rev(&backup), c1);
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "b.txt").as_deref(), Some("bi\n"));
    assert!(clean(&work));
    assert_eq!(out["can_undo"], true);
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Pull di `main` (fast-forward")
    );
    assert_eq!(out["status"]["head"]["Branch"]["target"], c2.to_string());

    // Undo: branch, indice e worktree tornano a `c1`; le modifiche non legate restano.
    work.write("a.txt", "modifica mia\n");
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(undo["undone"]["command"], "remote.pull");
    assert_eq!(work.rev("HEAD"), c1);
    assert_eq!(work.git(&["symbolic-ref", "HEAD"]), "refs/heads/main");
    assert_eq!(read(&work, "b.txt"), None);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("modifica mia\n"));
    // `c2` resta su `origin/main`: nessun backup in più.
    assert_eq!(undo["backups"], json!([]));

    let redo = r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(redo["redone"]["command"], "remote.pull");
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "b.txt").as_deref(), Some("bi\n"));
    assert_eq!(read(&work, "a.txt").as_deref(), Some("modifica mia\n"));

    // Già aggiornato: niente da fare, niente nel diario.
    let out = pull(&r, &mut s, json!({ "remote": "origin" }));
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(out["backup"], Value::Null);
    assert_eq!(out["before"], out["after"]);
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["done"].as_array().unwrap().len(), 1);
}

#[test]
fn undo_of_a_pull_never_overwrites_local_changes() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "a.txt", "due\n", "secondo");
    let (r, mut s) = open(&work);
    pull(&r, &mut s, json!({}));
    assert_eq!(work.rev("HEAD"), c2);

    // `a.txt` è cambiato dal pull e ora ha una modifica locale: l'undo la perderebbe.
    work.write("a.txt", "mia\n");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("mia\n"));
    assert_eq!(
        r.run("edit.history", &mut s, json!({})).unwrap()["can_undo"],
        true
    );

    // Un file che l'undo ricreerebbe e che ora esiste come ignorato: rifiutato anche quello.
    work.git(&["checkout", "--quiet", "--", "a.txt"]);
    other.git(&["rm", "--quiet", "a.txt"]);
    other.commit("togli a");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    pull(&r, &mut s, json!({}));
    assert!(read(&work, "a.txt").is_none());
    work.write(".git/info/exclude", "a.txt\n");
    work.write("a.txt", "ignorato ma mio\n");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(read(&work, "a.txt").as_deref(), Some("ignorato ma mio\n"));
    std::fs::remove_file(work.path().join("a.txt")).unwrap();
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), c2);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("due\n"));

    // Il branch spostato da fuori: `UndoConflict`, niente cambia.
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(undo["undone"]["command"], "remote.pull");
    assert_eq!(work.rev("HEAD"), c1);
    r.run("edit.redo", &mut s, json!({})).unwrap();
    work.commit_file("c.txt", "c\n", "da terminale");
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
}

#[test]
fn diverged_branches_merge_by_default() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "b.txt", "bi\n", "secondo");
    let mine = work.commit_file("c.txt", "ci\n", "mio");
    // Opzioni del merge dalla configurazione che cambierebbero il risultato: neutralizzate.
    work.git(&["config", "branch.main.mergeOptions", "--squash --no-commit"]);
    work.git(&["config", "merge.ff", "only"]);
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["strategy_source"], "default");
    let merge = work.rev("HEAD");
    assert_eq!(out["after"], merge.to_string());
    assert_eq!(
        work.git(&["log", "-1", "--format=%P"]),
        format!("{mine} {c2}")
    );
    assert_eq!(
        work.git(&["log", "-1", "--format=%s"]),
        "Merge remote-tracking branch 'origin/main'"
    );
    assert!(clean(&work));
    assert_eq!(read(&work, "b.txt").as_deref(), Some("bi\n"));

    // Undo: il commit di merge va in un backup, il branch torna a `mine`.
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), mine);
    let saved = undo["backups"].as_array().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(work.rev(saved[0].as_str().unwrap()), merge);
    assert!(clean(&work));
    assert_eq!(read(&work, "b.txt"), None);
    let _ = c1;

    // `pull.ff=false`: un commit di merge anche quando si potrebbe fare fast-forward.
    let (r, mut s) = open(&work);
    work.git(&["reset", "--quiet", "--hard", "origin/main~1"]);
    work.git(&["config", "pull.ff", "false"]);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["strategy_source"], "config");
    assert_eq!(
        work.git(&["log", "-1", "--format=%P"]).split(' ').count(),
        2
    );

    // `pull.ff=only` con branch divergenti: rifiutato, il branch non si tocca.
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);
    work.git(&["config", "pull.ff", "only"]);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(work.rev("HEAD"), mine);
    // `strategy` sostituisce la configurazione.
    let out = pull(&r, &mut s, json!({ "strategy": "rebase" }));
    assert_eq!(out["strategy"], "rebase");
    assert_eq!(out["strategy_source"], "param");
}

#[test]
fn rebase_from_the_configuration() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "b.txt", "bi\n", "secondo");
    let mine = work.commit_file("c.txt", "ci\n", "mio");
    work.git(&["config", "pull.rebase", "true"]);
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "rebase");
    assert_eq!(out["strategy_source"], "config");
    let rebased = work.rev("HEAD");
    assert_ne!(rebased, mine);
    assert_eq!(work.git(&["log", "-1", "--format=%P"]), c2.to_string());
    assert_eq!(work.git(&["log", "-1", "--format=%s"]), "mio");
    assert!(clean(&work));
    // Il commit di prima è nel backup del pull.
    let backup = out["backup"].as_str().unwrap();
    assert_eq!(work.rev(backup), mine);
    assert!(next_undo(&r, &mut s).contains("rebase su origin/main"));

    // Undo: torna al commit originale (tenuto dal backup del pull, nessun backup in più per
    // lui; il commit riscritto sì).
    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), mine);
    let saved = undo["backups"].as_array().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(work.rev(saved[0].as_str().unwrap()), rebased);
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), rebased);

    // `branch.<nome>.rebase` vince su `pull.rebase`.
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);
    work.git(&["config", "branch.main.rebase", "false"]);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    // `interactive`: rifiutato prima del fetch.
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);
    work.git(&["config", "branch.main.rebase", "interactive"]);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(work.rev("HEAD"), mine);
}

fn next_undo(r: &Registry, s: &mut Session) -> String {
    r.run("edit.history", s, json!({})).unwrap()["next_undo"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn a_merge_conflict_stays_in_progress_then_continue() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    let theirs = publish(&other, "a.txt", "loro\n", "loro");
    let mine = work.commit_file("a.txt", "nostra\n", "nostra");
    // Un non tracciato qualsiasi non blocca il pull e resta com'è.
    work.write("appunti.txt", "?\n");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert!(
        out["message"].as_str().unwrap().contains("CONFLICT"),
        "{out}"
    );
    assert_eq!(out["before"], mine.to_string());
    assert_eq!(out["after"], mine.to_string());
    assert_eq!(out["warnings"], json!([]));
    // Il backup resta, niente nel diario, il fetch è fatto.
    let backup = out["backup"].as_str().unwrap().to_owned();
    assert!(backup.starts_with("refs/project/backup/pull-"), "{backup}");
    assert_eq!(backups(&work), vec![backup.clone()]);
    assert_eq!(work.rev(&backup), mine);
    assert_eq!(out["can_undo"], false);
    assert_eq!(work.rev("origin/main"), theirs);
    // Il merge è in corso: MERGE_HEAD è l'upstream, il messaggio è quello del pull.
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(work.rev("MERGE_HEAD"), theirs);
    let merge_msg = std::fs::read_to_string(work.path().join(".git/MERGE_MSG")).unwrap();
    assert!(
        merge_msg.starts_with("Merge remote-tracking branch 'origin/main'\n"),
        "{merge_msg}"
    );
    assert_eq!(out["status"]["operation"], "Merge");
    let op = r.run("repo.operation", &mut s, json!({})).unwrap();
    assert_eq!(op["operation"], "Merge");
    assert_eq!(op["conflicts"], json!(["a.txt"]));
    assert_eq!(op["incoming"][0]["label"], "origin/main");
    assert_eq!(op["continue"], "merge.continue");
    assert_eq!(op["message"], "Merge remote-tracking branch 'origin/main'");
    // Un altro pull è rifiutato finché il merge è in corso; anche l'undo.
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(r.run("edit.undo", &mut s, json!({})).is_err());

    // Risolto e messo in stage, `merge.continue` fa il commit di merge e va nel diario.
    work.write("a.txt", "risolto\n");
    r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
        .unwrap();
    let done = r.run("merge.continue", &mut s, json!({})).unwrap();
    assert_eq!(
        done["message"],
        "Merge remote-tracking branch 'origin/main'"
    );
    let merge = work.rev("HEAD");
    assert_eq!(work.rev("HEAD^1"), mine);
    assert_eq!(work.rev("HEAD^2"), theirs);
    assert_eq!(
        work.git(&["log", "-1", "--format=%B"]).trim(),
        "Merge remote-tracking branch 'origin/main'"
    );
    assert!(!work.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(done["can_undo"], true);
    assert!(
        next_undo(&r, &mut s).contains("origin/main"),
        "{}",
        next_undo(&r, &mut s)
    );
    assert_eq!(read(&work, "appunti.txt").as_deref(), Some("?\n"));
    // L'undo riporta il branch a prima del pull, con indice e worktree.
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("nostra\n"));
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD"), merge);
}

#[test]
fn a_merge_conflict_then_abort_is_back_to_before() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "a.txt", "loro\n", "loro");
    publish(&other, "b.txt", "nuovo\n", "b dal remoto");
    let mine = work.commit_file("a.txt", "nostra\n", "nostra");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({ "strategy": "merge" }));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(read(&work, "b.txt").as_deref(), Some("nuovo\n"));
    let backup = out["backup"].as_str().unwrap().to_owned();

    // Senza conferma niente cambia.
    let err = r.run("merge.abort", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    assert!(work.path().join(".git/MERGE_HEAD").exists());
    let aborted = r
        .run("merge.abort", &mut s, json!({ "confirm": true }))
        .unwrap();
    assert!(
        aborted["backup"]
            .as_str()
            .unwrap()
            .starts_with("refs/project/backup/merge-abort-"),
        "{aborted}"
    );
    // Tutto com'era prima del pull: il worktree era pulito.
    assert_eq!(work.rev("HEAD"), mine);
    assert!(clean(&work), "{}", work.git(&["status", "--porcelain"]));
    assert_eq!(read(&work, "a.txt").as_deref(), Some("nostra\n"));
    assert_eq!(read(&work, "b.txt"), None);
    assert!(!work.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(aborted["can_undo"], false);
    // Il backup del pull resta (è il branch com'era).
    assert!(backups(&work).contains(&backup), "{:?}", backups(&work));
    // Il pull si può ripetere.
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["in_progress"], true);
}

#[test]
fn a_rebase_conflict_is_still_aborted_and_reported() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "a.txt", "loro\n", "loro");
    let mine = work.commit_file("a.txt", "nostra\n", "nostra");
    work.git(&["config", "pull.rebase", "true"]);
    let (r, mut s) = open(&work);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    let EngineError::PullConflict {
        branch,
        upstream,
        operation,
        files,
        message,
    } = &err
    else {
        panic!("{err}");
    };
    assert_eq!(branch, "main");
    assert_eq!(upstream, "origin/main");
    assert_eq!(*operation, PullOperation::Rebase);
    assert_eq!(files, &["a.txt"]);
    assert!(message.contains("CONFLICT"), "{message}");
    // Annullato: tutto com'era, nessuna operazione in corso, nessun backup rimasto.
    assert_eq!(work.rev("HEAD"), mine);
    assert!(clean(&work), "{}", work.git(&["status", "--porcelain"]));
    assert_eq!(read(&work, "a.txt").as_deref(), Some("nostra\n"));
    assert!(!work.path().join(".git/MERGE_HEAD").exists());
    assert!(!work.path().join(".git/rebase-merge").exists());
    assert_eq!(backups(&work), Vec::<String>::new());
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
    // Il fetch resta fatto.
    assert_ne!(work.rev("origin/main"), work.rev("HEAD~1"));
}

/// Installa l'hook `name` di `repo` con lo script `body` (dopo `#!/bin/sh`).
fn install_hook(repo: &Repo, name: &str, body: &str) -> std::path::PathBuf {
    let hook = repo.path().join(".git/hooks").join(name);
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, format!("#!/bin/sh\n{body}")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    hook
}

#[test]
fn a_failed_rebase_abort_keeps_the_backup() {
    // L'hook `post-checkout` (il rebase lo esegue dopo il checkout dell'upstream) lascia
    // `index.lock`: il pick si ferma e anche `rebase --abort` fallisce, il rebase resta in corso.
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "b.txt", "b\n", "dal remoto");
    let mine = work.commit_file("c.txt", "c\n", "mio");
    install_hook(
        &work,
        "post-checkout",
        ": > \"$(git rev-parse --git-dir)/index.lock\"\n",
    );
    let (r, mut s) = open(&work);
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::Git { .. }), "{err}");
    let text = err.to_string();
    assert!(text.contains("--abort` non è riuscito"), "{text}");
    let kept = backups(&work);
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].starts_with("refs/project/backup/pull-"), "{kept:?}");
    assert!(text.contains(&kept[0]), "l'errore nomina il backup: {text}");
    assert_eq!(work.rev(&kept[0]), mine);
    assert!(work.path().join(".git/rebase-merge").exists());
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
    // Pulizia da terminale: tolto il lock, il rebase si annulla e il branch è com'era.
    std::fs::remove_file(work.path().join(".git/index.lock")).unwrap();
    std::fs::remove_file(work.path().join(".git/hooks/post-checkout")).unwrap();
    work.git(&["rebase", "--abort"]);
    assert_eq!(work.rev("HEAD"), mine);
}

#[cfg(unix)]
#[test]
fn a_non_utf8_conflict_is_listed_once() {
    use std::os::unix::ffi::OsStrExt;
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    let name = std::ffi::OsStr::from_bytes(b"caf\xe9.txt");
    std::fs::write(other.path().join(name), "loro\n").unwrap();
    other.git(&["add", "-A"]);
    other.commit("loro");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    std::fs::write(work.path().join(name), "nostra\n").unwrap();
    work.git(&["add", "-A"]);
    work.commit("nostra");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["in_progress"], true);
    assert_eq!(
        out["conflicts"].as_array().unwrap().len(),
        1,
        "{}",
        out["conflicts"]
    );
}

#[test]
fn a_merge_stopped_by_a_hook_stays_in_progress() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "b.txt", "b\n", "dal remoto");
    let mine = work.commit_file("c.txt", "c\n", "mio");
    let hook = install_hook(&work, "pre-merge-commit", "echo no >&2\nexit 1\n");
    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!([]));
    assert_eq!(work.rev("HEAD"), mine);
    assert!(work.path().join(".git/MERGE_HEAD").exists());
    assert_eq!(out["can_undo"], false);
    let op = r.run("repo.operation", &mut s, json!({})).unwrap();
    assert_eq!(op["ready"], true);
    // Tolto l'hook, `merge.continue` conclude (`git commit` non usa `pre-merge-commit`).
    std::fs::remove_file(&hook).unwrap();
    r.run("merge.continue", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD^1"), mine);
    assert_eq!(read(&work, "b.txt").as_deref(), Some("b\n"));
}

#[test]
fn local_changes_block_the_pull() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let c2 = publish(&other, "a.txt", "due\n", "secondo");
    let (r, mut s) = open(&work);

    // Fast-forward che sovrascriverebbe una modifica locale: git rifiuta, niente cambia.
    work.write("a.txt", "mia\n");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`a.txt`"), "{err}");
    assert_eq!(work.rev("HEAD"), c1);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("mia\n"));
    assert_eq!(work.rev("origin/main"), c2, "il fetch è fatto");
    assert_eq!(backups(&work), Vec::<String>::new());

    // Un non tracciato che il pull creerebbe: rifiutato, il file resta.
    work.git(&["checkout", "--quiet", "--", "a.txt"]);
    publish(&other, "nuovo.txt", "loro\n", "nuovo");
    work.write("nuovo.txt", "mio\n");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(read(&work, "nuovo.txt").as_deref(), Some("mio\n"));
    assert_eq!(work.rev("HEAD"), c1);
    std::fs::remove_file(work.path().join("nuovo.txt")).unwrap();

    // Un merge con qualunque modifica ai file tracciati: rifiutato prima di scrivere.
    work.commit_file("c.txt", "c\n", "mio");
    let mine = work.rev("HEAD");
    work.write("c.txt", "modifica\n");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("autostash"), "{err}");
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(read(&work, "c.txt").as_deref(), Some("modifica\n"));
    // Un non tracciato qualsiasi invece non blocca.
    work.git(&["checkout", "--quiet", "--", "c.txt"]);
    work.write("appunti.txt", "?\n");
    let out = pull(&r, &mut s, json!({}));
    assert_eq!(out["strategy"], "merge");
    assert_eq!(read(&work, "appunti.txt").as_deref(), Some("?\n"));
}

#[test]
fn no_upstream_detached_head_and_operations_in_progress() {
    let Setup {
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    let (r, mut s) = open(&work);
    work.branch("solo");
    work.checkout("solo");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let text = err.to_string();
    assert!(text.contains("--set-upstream-to"), "{text}");
    assert!(text.contains("remote.push"), "{text}");

    // `remote` diverso da quello dell'upstream.
    work.checkout("main");
    let err = r
        .run("remote.pull", &mut s, json!({ "remote": "altro" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    work.detach(c1);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("staccata"), "{err}");

    // Un merge in corso.
    work.checkout("main");
    let (theirs, _) = (work.diverge("lato", "a.txt", "x\n", "y\n"), ());
    work.conflict("lato");
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let _ = theirs;

    for params in [
        json!({ "strategy": "octopus" }),
        json!({ "force": true }),
        json!({ "branch": "" }),
        json!({ "remote": "a b" }),
    ] {
        let err = r.run("remote.pull", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
}

#[test]
fn a_branch_that_is_not_checked_out_only_fast_forwards() {
    let Setup {
        other,
        work,
        c1,
        _remote: _keep,
        ..
    } = setup();
    // Un branch `-f` sul remoto e in locale (creato con `update-ref`: git non accetta più un
    // nome così per un branch nuovo), che segue `origin/-f`.
    other.branch("feat");
    other.checkout("feat");
    other.commit_file("f.txt", "f\n", "su -f");
    other.git(&["push", "--quiet", "origin", "refs/heads/feat:refs/heads/-f"]);
    work.git(&["update-ref", "refs/heads/-f", &c1.to_string()]);
    work.git(&["config", "branch.-f.remote", "origin"]);
    work.git(&["config", "branch.-f.merge", "refs/heads/-f"]);
    let f2 = other.commit_file("f.txt", "ff\n", "ancora su -f");
    other.git(&["push", "--quiet", "origin", "refs/heads/feat:refs/heads/-f"]);
    let (r, mut s) = open(&work);

    let out = pull(&r, &mut s, json!({ "branch": "-f" }));
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(work.rev("refs/heads/-f"), f2);
    // HEAD e worktree non si toccano.
    assert_eq!(work.rev("HEAD"), c1);
    assert!(read(&work, "f.txt").is_none());
    assert!(clean(&work));
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("refs/heads/-f"), c1);

    // Divergente: solo dal branch stesso.
    work.git(&[
        "update-ref",
        "refs/heads/-f",
        &work.commit_file("g.txt", "g\n", "g").to_string(),
    ]);
    let err = r
        .run("remote.pull", &mut s, json!({ "branch": "-f" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("branch.checkout"), "{err}");
}

#[test]
fn cancel_and_unreachable_remote() {
    let Setup {
        other,
        work,
        c1,
        _remote: remote,
    } = setup();
    publish(&other, "b.txt", "b\n", "secondo");
    let (r, mut s) = open(&work);
    let token = project_engine::CancelToken::new();
    token.cancel();
    let err = r
        .run_with(
            "remote.pull",
            &mut s,
            json!({}),
            project_engine::JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::Cancelled {
                command: "remote.pull"
            }
        ),
        "{err}"
    );
    assert_eq!(work.rev("origin/main"), c1, "niente scaricato");
    let missing = work.path().join("non-esiste.git");
    work.git(&["remote", "set-url", "origin", missing.to_str().unwrap()]);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(err, EngineError::RemoteUnreachable { .. }),
        "{err}"
    );
    assert_eq!(work.rev("HEAD"), c1);
    drop(remote);
}

/// `work` ignora `*.env` e ha un suo `segreto.env`; il remoto aggiunge `segreto.env`
/// tracciato con un altro contenuto.
fn ignored_secret(other: &Repo, work: &Repo) {
    work.write(".git/info/exclude", "*.env\n");
    work.write("segreto.env", "la mia chiave\n");
    other.write("segreto.env", "quella del remoto\n");
    other.stage(&["segreto.env"]);
    other.commit("aggiungi segreto.env");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
}

#[test]
fn ignored_files_are_never_overwritten() {
    // Fast-forward, rebase e un merge che finirebbe in conflitto su un altro file: rifiutati
    // prima di scrivere, il file ignorato resta identico.
    for case in ["ff", "rebase", "merge_conflict"] {
        let Setup {
            other,
            work,
            _remote: _keep,
            ..
        } = setup();
        match case {
            "rebase" => {
                work.commit_file("c.txt", "c\n", "mio");
                work.git(&["config", "pull.rebase", "true"]);
            }
            "merge_conflict" => {
                publish(&other, "a.txt", "loro\n", "loro");
                work.commit_file("a.txt", "nostra\n", "nostra");
            }
            _ => {}
        }
        ignored_secret(&other, &work);
        let head = work.rev("HEAD");
        let (r, mut s) = open(&work);
        let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{case}: {err}");
        assert!(err.to_string().contains("segreto.env"), "{case}: {err}");
        assert_eq!(
            read(&work, "segreto.env").as_deref(),
            Some("la mia chiave\n"),
            "{case}"
        );
        assert_eq!(work.rev("HEAD"), head, "{case}");
        assert!(clean(&work), "{case}");
        assert_eq!(backups(&work), Vec::<String>::new(), "{case}");

        // Spostato il file, il pull riesce (salvo il conflitto, che resta in corso).
        std::fs::remove_file(work.path().join("segreto.env")).unwrap();
        let out = pull(&r, &mut s, json!({}));
        assert_eq!(
            out["in_progress"],
            case == "merge_conflict",
            "{case}: {out}"
        );
        assert_eq!(
            read(&work, "segreto.env").as_deref(),
            Some("quella del remoto\n"),
            "{case}"
        );
    }
}

#[test]
fn a_conflict_resolved_by_rerere_is_still_a_conflict() {
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "a.txt", "loro\n", "loro");
    let mine = work.commit_file("a.txt", "nostra\n", "nostra");
    work.git(&["config", "rerere.enabled", "true"]);
    work.git(&["config", "rerere.autoUpdate", "true"]);
    // Una risoluzione registrata da rerere per lo stesso conflitto.
    work.git(&["fetch", "--quiet", "origin"]);
    work.git_fails(&["merge", "--no-edit", "origin/main"]);
    work.write("a.txt", "risolto\n");
    work.stage(&["a.txt"]);
    work.git(&["commit", "--quiet", "--no-edit"]);
    work.git(&["reset", "--quiet", "--hard", &mine.to_string()]);

    let (r, mut s) = open(&work);
    let out = pull(&r, &mut s, json!({}));
    // Il merge resta fermo e il file conta come conflitto anche se rerere l'ha già risolto
    // (in stage): va controllato e poi concluso con `merge.continue`.
    assert_eq!(out["in_progress"], true);
    assert_eq!(out["conflicts"], json!(["a.txt"]));
    assert_eq!(work.rev("HEAD"), mine);
    assert_eq!(read(&work, "a.txt").as_deref(), Some("risolto\n"));
    assert!(work.path().join(".git/MERGE_HEAD").exists());
    r.run("merge.continue", &mut s, json!({})).unwrap();
    assert_eq!(work.rev("HEAD^1"), mine);
}

#[test]
fn rebase_never_deletes_an_ignored_file_added_and_removed_by_local_commits() {
    // C1 committa `x.env` per sbaglio, C2 lo toglie e lo ignora; il file vero resta nel
    // worktree. Il rebase riapplicherebbe C1 (scrive `x.env`) e C2 (lo cancella).
    // Lo stesso per un file rinominato (`y.env` → `z.txt`) e per uno tolto e ricreato.
    for case in ["added_then_removed", "renamed", "recreated"] {
        let Setup {
            other,
            work,
            _remote: _keep,
            ..
        } = setup();
        let file = match case {
            "added_then_removed" => "x.env",
            "renamed" => "y.env",
            _ => "w.env",
        };
        work.commit_file(file, "per sbaglio\n", "C1");
        match case {
            "renamed" => {
                work.git(&["mv", file, "z.txt"]);
                work.commit("C2: rinomina");
            }
            "recreated" => {
                work.git(&["rm", "--quiet", file]);
                work.commit("C2: togli");
                work.commit_file(file, "di nuovo\n", "C3: rimetti");
                work.git(&["rm", "--quiet", file]);
                work.commit("C4: togli ancora");
            }
            _ => {
                work.git(&["rm", "--quiet", file]);
                work.commit("C2: togli");
            }
        }
        work.write(".git/info/exclude", "*.env\n");
        work.write(file, "MYSECRET\n");
        publish(&other, "b.txt", "b\n", "dal remoto");
        let head = work.rev("HEAD");
        let (r, mut s) = open(&work);
        let err = r
            .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{case}: {err}");
        assert!(err.to_string().contains(file), "{case}: {err}");
        assert_eq!(read(&work, file).as_deref(), Some("MYSECRET\n"), "{case}");
        assert_eq!(work.rev("HEAD"), head, "{case}");
        assert_eq!(backups(&work), Vec::<String>::new(), "{case}");
        // Con `--rebase-merges` lo stesso.
        work.git(&["config", "pull.rebase", "merges"]);
        let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{case}: {err}");
        assert_eq!(read(&work, file).as_deref(), Some("MYSECRET\n"), "{case}");
        // Il merge non passa da quegli alberi: il file resta e il pull riesce.
        let out = pull(&r, &mut s, json!({ "strategy": "merge" }));
        assert_eq!(out["strategy"], "merge");
        assert_eq!(read(&work, file).as_deref(), Some("MYSECRET\n"), "{case}");
    }
}

#[test]
fn rebase_never_deletes_an_ignored_file_modified_locally_and_removed_upstream() {
    // La base traccia `x.env`; l'upstream lo toglie (e lo ignora). In locale un commit lo
    // modifica e uno successivo lo toglie; il file vero resta nel worktree, ignorato. Il
    // rebase si fermerebbe su un conflitto modify/delete scrivendo `x.env`, e l'abort lo
    // cancellerebbe.
    let Setup {
        other,
        work,
        _remote: _keep,
        ..
    } = setup();
    publish(&other, "x.env", "base\n", "base con x.env");
    work.git(&["fetch", "--quiet", "origin"]);
    work.git(&["merge", "--quiet", "--ff-only", "origin/main"]);
    other.git(&["rm", "--quiet", "x.env"]);
    other.commit("togli x.env");
    other.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    work.commit_file("x.env", "modificato\n", "C1: modifica");
    work.git(&["rm", "--quiet", "x.env"]);
    work.commit("C2: togli");
    work.write(".git/info/exclude", "*.env\n");
    work.write("x.env", "MYSECRET\n");
    let head = work.rev("HEAD");
    let (r, mut s) = open(&work);
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("x.env"), "{err}");
    assert_eq!(read(&work, "x.env").as_deref(), Some("MYSECRET\n"));
    assert_eq!(work.rev("HEAD"), head);
    assert_eq!(backups(&work), Vec::<String>::new());
}
