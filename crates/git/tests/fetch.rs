//! Test di integrazione di `remote.fetch` (S25) con il `CliBackend` e remoti locali (`git init
//! --bare` in una cartella temporanea): commit nuovi, niente di nuovo, un solo branch, tag,
//! remoto inesistente o con l'URL verso una cartella che non c'è, nomi di remoto che iniziano
//! con `-`, avanzamento, credenziali rifiutate, nessuna attesa di input, annullamento.

mod common;

use common::Repo;
use project_domain::Oid;
use project_engine::{CancelToken, EngineError, JobContext, Progress, Registry, Session};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn open(repo: &Repo) -> (Registry, Session) {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(Repo::opener()));
    r.run("repo.open", &mut s, json!({ "path": repo.path_str() }))
        .unwrap();
    (r, s)
}

fn fetch(r: &Registry, s: &mut Session, params: Value) -> Value {
    r.run("remote.fetch", s, params)
        .unwrap_or_else(|e| panic!("remote.fetch: {e}"))
}

/// Il remoto bare, un clone che ci scrive (`other`) e il repo sotto test (`work`), che ha il
/// remoto `origin` ma non ha ancora scaricato niente.
struct Setup {
    remote: Repo,
    other: Repo,
    work: Repo,
}

fn setup() -> Setup {
    let remote = Repo::bare();
    let other = Repo::new();
    other.add_remote("origin", &remote);
    let work = Repo::new();
    work.add_remote("origin", &remote);
    Setup {
        remote,
        other,
        work,
    }
}

fn update(name: &str, before: Option<Oid>, after: Option<Oid>) -> Value {
    json!({
        "name": name,
        "before": before.map(|o| o.to_string()),
        "after": after.map(|o| o.to_string()),
    })
}

fn all_refs(repo: &Repo) -> String {
    repo.git(&["for-each-ref", "--format=%(refname) %(objectname)"])
}

#[test]
fn fetch_brings_new_commits_then_nothing() {
    let Setup {
        remote: _remote,
        other,
        work,
    } = setup();
    let c1 = other.commit_file("a.txt", "uno\n", "primo");
    other.push("origin", "HEAD", "main");
    let (r, mut s) = open(&work);

    let out = fetch(&r, &mut s, json!({}));
    assert_eq!(out["remote"], "origin");
    assert_eq!(out["branch"], Value::Null);
    assert_eq!(
        out["updated"],
        json!([update("refs/remotes/origin/main", None, Some(c1))])
    );
    assert_eq!(out["warnings"], json!([]));
    let messages: Vec<&str> = out["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert!(
        messages.iter().any(|m| m.starts_with("From ")),
        "{messages:?}"
    );
    assert!(
        messages.iter().any(|m| m.contains("[new branch]")),
        "{messages:?}"
    );
    // Niente righe di avanzamento nei messaggi.
    assert!(
        messages.iter().all(|m| !m.contains("objects:")),
        "{messages:?}"
    );
    assert!(
        out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "refs/remotes/origin/main")
    );
    assert_eq!(work.rev("origin/main"), c1);
    // Branch locali, HEAD e diario intatti.
    assert!(
        work.git_output(&["rev-parse", "-q", "--verify", "HEAD"])
            .stdout
            .is_empty()
    );
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);

    // Un commit nuovo sul remoto, e un tag annotato che lo indica.
    let c2 = other.commit_file("a.txt", "due\n", "secondo");
    other.annotated_tag("v1", "HEAD", "versione 1");
    other.git(&[
        "push",
        "--quiet",
        "origin",
        "HEAD:refs/heads/main",
        "refs/tags/v1",
    ]);
    let out = fetch(&r, &mut s, json!({ "remote": "origin" }));
    assert_eq!(
        out["updated"],
        json!([
            update("refs/remotes/origin/main", Some(c1), Some(c2)),
            update("refs/tags/v1", None, Some(c2)),
        ])
    );

    // Niente di nuovo: nessun ref cambiato, niente scritto da git.
    let before = all_refs(&work);
    let out = fetch(&r, &mut s, json!({}));
    assert_eq!(out["updated"], json!([]));
    assert_eq!(out["messages"], json!([]));
    assert_eq!(all_refs(&work), before);
}

#[test]
fn fetch_of_a_single_branch() {
    let Setup {
        remote: _remote,
        other,
        work,
    } = setup();
    let c1 = other.commit_file("a.txt", "uno\n", "primo");
    other.push("origin", "HEAD", "main");
    other.push("origin", "HEAD", "feature");
    let (r, mut s) = open(&work);

    let out = fetch(&r, &mut s, json!({ "branch": "feature" }));
    assert_eq!(out["branch"], "feature");
    assert_eq!(
        out["updated"],
        json!([update("refs/remotes/origin/feature", None, Some(c1))])
    );
    let err = r
        .run("remote.fetch", &mut s, json!({ "branch": "manca" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("manca"), "{err}");
}

#[test]
fn the_default_remote_follows_the_upstream() {
    let Setup {
        remote,
        other,
        work,
    } = setup();
    let c1 = other.commit_file("a.txt", "uno\n", "primo");
    other.push("origin", "HEAD", "main");
    work.add_remote("fork", &remote);
    work.git(&["fetch", "--quiet", "fork"]);
    work.git(&["switch", "--quiet", "-c", "main", "fork/main"]);
    work.set_upstream("main", "fork");
    let c2 = other.commit_file("a.txt", "due\n", "secondo");
    other.push("origin", "HEAD", "main");
    let (r, mut s) = open(&work);

    let out = fetch(&r, &mut s, json!({}));
    assert_eq!(out["remote"], "fork");
    assert_eq!(
        out["updated"],
        json!([update("refs/remotes/fork/main", Some(c1), Some(c2))])
    );
    // `origin` non è stato toccato.
    assert!(
        work.git_output(&["rev-parse", "-q", "--verify", "refs/remotes/origin/main"])
            .stdout
            .is_empty()
    );
}

#[test]
fn unknown_remote_and_missing_folder() {
    let Setup {
        remote: _remote,
        work,
        ..
    } = setup();
    let (r, mut s) = open(&work);
    let before = all_refs(&work);

    // Non configurato: mai passato a git (che lo userebbe come percorso o URL).
    let elsewhere = Repo::bare();
    for remote in ["nessuno", elsewhere.path_str()] {
        let err = r
            .run("remote.fetch", &mut s, json!({ "remote": remote }))
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::NotAllowed(_) | EngineError::InvalidParams { .. }
            ),
            "{remote}: {err}"
        );
    }

    // Configurato, con l'URL verso una cartella che non c'è.
    let gone = work.path().join("non-esiste");
    work.git(&["remote", "add", "rotto", gone.to_str().unwrap()]);
    let (r, mut s) = open(&work);
    let err = r
        .run("remote.fetch", &mut s, json!({ "remote": "rotto" }))
        .unwrap_err();
    match &err {
        EngineError::RemoteUnreachable { remote, message } => {
            assert_eq!(remote, "rotto");
            assert!(
                message.contains("does not appear to be a git repository"),
                "{message}"
            );
        }
        other => panic!("errore inatteso: {other}"),
    }
    assert!(err.to_string().contains("non è raggiungibile"), "{err}");
    assert_eq!(all_refs(&work), before);
}

#[test]
fn remote_names_that_look_like_options() {
    let Setup {
        remote,
        other,
        work,
    } = setup();
    let c1 = other.commit_file("a.txt", "uno\n", "primo");
    other.push("origin", "HEAD", "main");
    // Due remoti con nomi che git leggerebbe come opzioni senza `--end-of-options`; il secondo,
    // come opzione, farebbe eseguire `false` al posto di `git-upload-pack`.
    for name in ["-x", "--upload-pack=false"] {
        work.git(&["remote", "add", "--", name, remote.path_str()]);
    }
    let (r, mut s) = open(&work);
    let remotes = ["--upload-pack=false", "-x"];
    for name in remotes {
        let out = fetch(&r, &mut s, json!({ "remote": name, "branch": "main" }));
        assert_eq!(out["remote"], name);
        assert_eq!(
            out["updated"],
            json!([update(&format!("refs/remotes/{name}/main"), None, Some(c1))])
        );
    }
}

#[test]
fn progress_events_arrive_while_fetching() {
    let Setup {
        remote: _remote,
        other,
        work,
    } = setup();
    for i in 0..30 {
        other.commit_file(
            &format!("f{i}.txt"),
            &format!("{i}\n"),
            &format!("commit {i}"),
        );
    }
    other.push("origin", "HEAD", "main");
    let (r, mut s) = open(&work);

    let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
    let sink = Arc::clone(&seen);
    let job = JobContext::new().with_progress(move |p| sink.lock().unwrap().push(p.clone()));
    let out = r.run_with("remote.fetch", &mut s, json!({}), job).unwrap();
    assert_eq!(out["updated"].as_array().unwrap().len(), 1);
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty(), "nessun avanzamento");
    assert!(seen.iter().any(|p| p.done), "{seen:?}");
    assert!(
        seen.iter()
            .any(|p| p.percent.is_some() && p.total.is_some()),
        "{seen:?}"
    );
    assert!(
        seen.iter()
            .all(|p| !p.phase.is_empty() && !p.phase.contains(':')),
        "{seen:?}"
    );
}

/// Uno script al posto di `ssh` (`core.sshCommand`) per simulare il remoto: il repo ha il
/// remoto `origin` con un URL ssh finto.
#[cfg(unix)]
fn with_fake_ssh(script: &str) -> (Repo, tempfile::TempDir) {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("finto-ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let work = Repo::new();
    work.git(&["remote", "add", "origin", "ssh://git@example.invalid/r.git"]);
    work.git(&["config", "core.sshCommand", path.to_str().unwrap()]);
    (work, dir)
}

#[cfg(unix)]
#[test]
fn rejected_credentials_are_an_auth_error() {
    let (work, _dir) =
        with_fake_ssh("echo 'git@example.invalid: Permission denied (publickey).' >&2\nexit 255");
    let (r, mut s) = open(&work);
    let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
    match &err {
        EngineError::Auth { remote, message } => {
            assert_eq!(remote, "origin");
            assert!(
                message.contains("Permission denied (publickey)"),
                "{message}"
            );
        }
        other => panic!("errore inatteso: {other}"),
    }
}

#[cfg(unix)]
#[test]
fn never_waits_for_input() {
    // Un "ssh" che chiede una passphrase o la conferma della chiave dell'host dal terminale,
    // come fa ssh (il suo stdin è il canale con git, non la tastiera). Il processo non ha un
    // terminale di controllo: l'apertura di `/dev/tty` fallisce subito, anche se i test girano
    // in un terminale.
    let (work, _dir) = with_fake_ssh(
        "if read answer < /dev/tty; then echo 'letto dal terminale' >&2; exit 0; fi\n\
         echo 'Host key verification failed.' >&2\nexit 255",
    );
    let (r, mut s) = open(&work);
    let start = Instant::now();
    let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "ha aspettato input"
    );
    assert!(matches!(err, EngineError::Auth { .. }), "{err}");
    assert!(!err.to_string().contains("letto"), "{err}");
}

#[cfg(unix)]
#[test]
fn cancel_stops_the_fetch() {
    let (work, _dir) = with_fake_ssh("sleep 60");
    let (r, mut s) = open(&work);
    let token = CancelToken::new();
    let canceller = token.clone();
    let start = Instant::now();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        canceller.cancel();
    });
    let err = r
        .run_with(
            "remote.fetch",
            &mut s,
            json!({}),
            JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    thread.join().unwrap();
    assert!(matches!(err, EngineError::Cancelled { .. }), "{err}");
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "{:?}",
        start.elapsed()
    );
    // Dopo l'annullamento il repo funziona: nessun lock rimasto.
    let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
    assert_eq!(refs, json!([]));
}

#[test]
fn backend_lists_remotes() {
    use project_engine::GitBackend as _;
    let Setup { remote, work, .. } = setup();
    work.add_remote("b-secondo", &remote);
    assert_eq!(work.open().remotes().unwrap(), ["b-secondo", "origin"]);
}
