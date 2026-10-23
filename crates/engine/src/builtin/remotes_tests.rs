//! Test di `remote.list`, `remote.add`, `remote.remove` sul `MemoryBackend`.

use crate::{EngineError, GitBackend, MemoryBackend, MemoryOpener, Registry, Session};
use project_domain::Oid;
use serde_json::{Value, json};
use std::sync::Arc;

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
    r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn remote_refs(s: &Session, remote: &str) -> Vec<(String, Oid)> {
    s.repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .filter(|r| r.name.starts_with(&format!("refs/remotes/{remote}/")))
        .map(|r| (r.name, r.target))
        .collect()
}

fn upstream_of(s: &Session, branch: &str) -> Option<String> {
    s.repo()
        .unwrap()
        .refs()
        .unwrap()
        .into_iter()
        .find(|r| r.name == format!("refs/heads/{branch}"))
        .and_then(|r| r.upstream)
        .map(|u| u.name)
}

#[test]
fn list_masks_credentials_and_marks_the_defaults() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "https://nicola:segreto@example.com/fork.git");
    repo.add_remote("gh", "https://ghp_token@github.com/x/y.git");
    repo.set_config("remote.fork.pushurl", &["ssh://git@example.com/fork.git"]);
    repo.set_config("remote.pushDefault", &["fork"]);
    let (r, mut s) = opened_with(repo);
    let out = run(&r, &mut s, "remote.list", json!({}));
    let text = out.to_string();
    assert!(
        !text.contains("segreto") && !text.contains("ghp_token"),
        "{text}"
    );
    let names: Vec<&str> = out["remotes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["fork", "gh", "origin"]);
    assert_eq!(
        out["remotes"][0],
        json!({
            "name": "fork",
            "url": "https://nicola:***@example.com/fork.git",
            "push_urls": ["ssh://git@example.com/fork.git"],
            "fetch": ["+refs/heads/*:refs/remotes/fork/*"],
            "mirror": false,
            "branches": 0,
            "default": false,
            "push_default": true,
            "credentials": true,
        })
    );
    assert_eq!(out["remotes"][1]["url"], "https://***@github.com/x/y.git");
    let origin = &out["remotes"][2];
    assert_eq!(origin["url"], "https://example.com/progetto.git");
    assert_eq!(origin["credentials"], false);
    assert_eq!(origin["push_urls"], json!([]));
    assert_eq!(origin["branches"], 2);
    assert_eq!(origin["default"], true);
    assert_eq!(out["default"], "origin");
    assert_eq!(out["push_default"], "fork");

    let err = r
        .run("remote.list", &mut s, json!({ "name": "x" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
}

#[test]
fn add_with_fetch_then_undo_and_redo() {
    // Il "server" di origin resta da parte togliendo il remoto: aggiunto con lo stesso URL,
    // il fetch ne scarica i branch.
    let mut repo = MemoryBackend::sample();
    repo.add_remote("copia", "https://u:pw@example.com/copia.git");
    repo.set_branch_on_remote("copia", "main", oid(12));
    let snapshot = repo.remote_snapshot("copia").unwrap();
    repo.remove_remote_config(&snapshot);
    let (r, mut s) = opened_with(repo);

    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "nuovo", "url": "https://u:pw@example.com/copia.git", "fetch": true }),
    );
    assert_eq!(out["remote"], "nuovo");
    assert_eq!(out["url"], "https://u:***@example.com/copia.git");
    assert_eq!(out["credentials"], true);
    assert_eq!(out["fetched"], true);
    assert_eq!(out["fetch_error"], Value::Null);
    assert!(out["warnings"][0].as_str().unwrap().contains("in chiaro"));
    assert_eq!(
        out["fetch"]["updated"],
        json!([{ "name": "refs/remotes/nuovo/main", "before": null, "after": oid(12).to_string() }])
    );
    assert!(!out.to_string().contains("pw@"), "{out}");
    assert!(
        out["remotes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["name"] == "nuovo" && r["branches"] == 1)
    );
    assert_eq!(
        out["next_undo"],
        "Aggiungi remoto `nuovo` (con fetch: 1 branch remoti)"
    );
    // Il diario non mostra la password.
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert!(!history.to_string().contains("pw@"), "{history}");

    run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        !s.repo()
            .unwrap()
            .remotes()
            .unwrap()
            .contains(&"nuovo".to_owned())
    );
    assert!(remote_refs(&s, "nuovo").is_empty());
    run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(
        remote_refs(&s, "nuovo"),
        [("refs/remotes/nuovo/main".to_owned(), oid(12))]
    );
    assert_eq!(
        s.repo().unwrap().config_values("remote.nuovo.url").unwrap(),
        ["https://u:pw@example.com/copia.git"]
    );
    let snapshot = s.repo().unwrap().remote_snapshot("nuovo").unwrap();
    assert_eq!(snapshot.url(), Some("https://u:pw@example.com/copia.git"));
}

#[test]
fn add_without_fetch_and_fetch_errors_do_not_undo_the_add() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "fork", "url": "/srv/fork.git" }),
    );
    assert_eq!(out["fetched"], false);
    assert_eq!(out["fetch"], Value::Null);
    assert_eq!(out["credentials"], false);
    assert_eq!(out["warnings"], json!([]));
    assert_eq!(out["next_undo"], "Aggiungi remoto `fork`");

    let mut repo = MemoryBackend::sample();
    repo.add_remote("rotto", "/srv/rotto.git");
    repo.fail_fetch("rotto", Some(crate::FetchFailure::Auth("HTTP 401".into())));
    let snapshot = repo.remote_snapshot("rotto").unwrap();
    repo.remove_remote_config(&snapshot);
    let (r, mut s) = opened_with(repo);
    let out = run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "rotto", "url": "/srv/rotto.git", "fetch": true }),
    );
    assert_eq!(out["fetched"], false);
    assert_eq!(out["fetch_error"]["kind"], "auth");
    assert!(
        s.repo()
            .unwrap()
            .remotes()
            .unwrap()
            .contains(&"rotto".to_owned())
    );
    assert_eq!(out["can_undo"], true);
}

#[test]
fn add_refuses_existing_names_and_bad_names_or_urls() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let err = r
        .run(
            "remote.add",
            &mut s,
            json!({ "name": "origin", "url": "/srv/x.git" }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(
        s.repo().unwrap().remote_snapshot("origin").unwrap().url(),
        Some("https://example.com/progetto.git")
    );
    for name in ["-x", "--upload-pack=x", "", "a b", "a..b"] {
        let err = r
            .run(
                "remote.add",
                &mut s,
                json!({ "name": name, "url": "/srv/x" }),
            )
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{name}: {err}"
        );
    }
    for url in [
        "-x",
        "--upload-pack=touch x",
        "ext::sh -c touch% /tmp/x",
        "fd::3",
        "",
    ] {
        let err = r
            .run("remote.add", &mut s, json!({ "name": "x", "url": url }))
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{url}: {err}"
        );
    }
    let err = r
        .run(
            "remote.add",
            &mut s,
            json!({ "name": "x", "url": "/a", "prune": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    assert_eq!(s.repo().unwrap().remotes().unwrap(), ["origin"]);
    assert_eq!(
        run(&r, &mut s, "edit.history", json!({}))["can_undo"],
        false
    );

    // Branch remoti rimasti da un remoto tolto a mano: l'undo li cancellerebbe.
    let mut repo = MemoryBackend::sample();
    repo.set_remote_branch("vecchio/main", oid(11));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.add",
            &mut s,
            json!({ "name": "vecchio", "url": "/x" }),
        )
        .unwrap_err();
    assert!(err.to_string().contains("refs/remotes/vecchio/"), "{err}");
}

#[test]
fn remove_needs_confirm_and_undo_restores_everything() {
    let mut repo = MemoryBackend::sample();
    repo.set_config("branch.feature/grafo.pushRemote", &["origin"]);
    let (r, mut s) = opened_with(repo);
    let before = s.repo().unwrap().remote_snapshot("origin").unwrap();
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "origin" }))
        .unwrap_err();
    let EngineError::ConfirmRequired { reason, .. } = &err else {
        panic!("{err}");
    };
    assert!(reason.contains("2 branch remoti"), "{reason}");
    assert!(
        reason.contains("`feature/grafo`, `main`") || reason.contains("`main`"),
        "{reason}"
    );
    assert!(reason.contains("pushRemote"), "{reason}");
    assert_eq!(s.repo().unwrap().remote_snapshot("origin").unwrap(), before);

    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert_eq!(out["remote"], "origin");
    assert_eq!(out["upstreams_removed"], json!(["feature/grafo", "main"]));
    assert_eq!(out["push_remotes_removed"], json!(["feature/grafo"]));
    assert_eq!(out["push_default_removed"], false);
    assert_eq!(out["removed"]["refs"].as_array().unwrap().len(), 2);
    assert_eq!(out["remotes"], json!([]));
    // origin/main (11) e origin/feature/grafo (9) sono raggiungibili da main: niente backup.
    assert_eq!(out["backups"], json!([]));
    assert!(remote_refs(&s, "origin").is_empty());
    assert_eq!(upstream_of(&s, "main"), None);
    assert!(s.repo().unwrap().remotes().unwrap().is_empty());
    assert!(
        s.repo()
            .unwrap()
            .config_values("branch.feature/grafo.pushRemote")
            .unwrap()
            .is_empty()
    );

    let undo = run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(undo["warnings"], json!([]));
    assert_eq!(s.repo().unwrap().remote_snapshot("origin").unwrap(), before);
    assert_eq!(
        upstream_of(&s, "main").as_deref(),
        Some("refs/remotes/origin/main")
    );
    // Il "server" è tornato: il fetch funziona come prima.
    let fetch = run(&r, &mut s, "remote.fetch", json!({}));
    assert_eq!(
        fetch["updated"][0]["name"],
        "refs/remotes/origin/feature/grafo"
    );
    // Dopo il fetch il remoto è cambiato: il redo è rifiutato e niente cambia.
    let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert_eq!(remote_refs(&s, "origin").len(), 2);
}

#[test]
fn remove_a_remote_with_commits_only_there_saves_a_backup() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    run(&r, &mut s, "remote.fetch", json!({}));
    // oid(13) è solo su origin/feature/grafo.
    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert_eq!(out["backups"], json!(["refs/project/backup/remote-1"]));
    run(&r, &mut s, "edit.undo", json!({}));
    assert!(
        remote_refs(&s, "origin")
            .contains(&("refs/remotes/origin/feature/grafo".to_owned(), oid(13)))
    );
    // Redo: di nuovo via, senza un nuovo backup.
    let redo = run(&r, &mut s, "edit.redo", json!({}));
    assert_eq!(redo["backups"], json!([]));
    assert!(remote_refs(&s, "origin").is_empty());
}

#[test]
fn remove_push_default_and_undo_conflicts() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "/srv/fork.git");
    repo.set_remote_branch("fork/main", oid(12));
    repo.set_config("remote.pushDefault", &["fork"]);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "fork" }))
        .unwrap_err();
    assert!(err.to_string().contains("remote.pushDefault"), "{err}");
    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "fork", "confirm": true }),
    );
    assert_eq!(out["push_default_removed"], true);
    let config = |s: &Session| {
        s.repo()
            .unwrap()
            .config_values("remote.pushDefault")
            .unwrap()
    };
    assert!(config(&s).is_empty());
    run(&r, &mut s, "edit.undo", json!({}));
    assert_eq!(config(&s), ["fork"]);
    run(&r, &mut s, "edit.redo", json!({}));

    // Ricreato fuori dal diario: UndoConflict, niente cambia.
    let (r, mut s) = opened_with(MemoryBackend::sample());
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    s.repo_mut()
        .unwrap()
        .create_remote("origin", "/srv/nuovo.git")
        .unwrap();
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    assert!(remote_refs(&s, "origin").is_empty());
    assert_eq!(
        s.repo().unwrap().remote_snapshot("origin").unwrap().url(),
        Some("/srv/nuovo.git")
    );
}

#[test]
fn undo_of_add_is_refused_once_a_branch_uses_the_remote() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    run(
        &r,
        &mut s,
        "remote.add",
        json!({ "name": "fork", "url": "/srv/fork.git" }),
    );
    // Il primo push imposta l'upstream di `main` su `fork/main` (il push non è nel diario).
    run(&r, &mut s, "remote.push", json!({ "remote": "fork" }));
    assert_eq!(
        upstream_of(&s, "main").as_deref(),
        Some("refs/remotes/fork/main")
    );
    let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
    let EngineError::UndoConflict { refs, .. } = &err else {
        panic!("{err}");
    };
    assert!(
        refs.iter().any(|r| r.contains("refs/remotes/fork/main")),
        "{refs:?}"
    );
    assert!(refs.iter().any(|r| r.contains("upstream")), "{refs:?}");
    assert!(
        s.repo()
            .unwrap()
            .remotes()
            .unwrap()
            .contains(&"fork".to_owned())
    );
}

#[test]
fn remove_refuses_unknown_remotes_and_operations() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "manca", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("`origin`"), "{err}");
    let err = r
        .run("remote.remove", &mut s, json!({ "name": "a..b" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(project_domain::Operation::Merge));
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(remote_refs(&s, "origin").len(), 2);
}

#[test]
fn ahead_and_behind_follow_fetch_pull_and_push() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let track = |out: &Value, name: &str| {
        let r = out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone();
        (
            r["upstream"]["ahead"].clone(),
            r["upstream"]["behind"].clone(),
        )
    };
    let refs = run(&r, &mut s, "repo.refs", json!({}));
    let refs = json!({ "refs": refs });
    assert_eq!(track(&refs, "refs/heads/main"), (json!(1), json!(0)));
    assert_eq!(
        track(&refs, "refs/heads/feature/grafo"),
        (json!(0), json!(0))
    );
    let out = run(&r, &mut s, "remote.fetch", json!({}));
    assert_eq!(
        track(&out, "refs/heads/feature/grafo"),
        (json!(0), json!(1))
    );
    let out = run(&r, &mut s, "remote.push", json!({}));
    assert_eq!(track(&out, "refs/heads/main"), (json!(0), json!(0)));
    let out = run(
        &r,
        &mut s,
        "remote.pull",
        json!({ "branch": "feature/grafo" }),
    );
    assert_eq!(
        track(&out, "refs/heads/feature/grafo"),
        (json!(0), json!(0))
    );
}

#[test]
fn remove_refuses_remotes_defined_elsewhere_and_promisors() {
    let mut repo = MemoryBackend::sample();
    repo.set_remote_elsewhere(
        "origin",
        &["`remote.origin.fetch` (global, file:~/.gitconfig)"],
    );
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("~/.gitconfig"), "{err}");
    assert_eq!(remote_refs(&s, "origin").len(), 2);

    for (key, value) in [
        ("remote.origin.promisor", "true"),
        ("extensions.partialClone", "origin"),
    ] {
        let mut repo = MemoryBackend::sample();
        repo.set_config(key, &[value]);
        let (r, mut s) = opened_with(repo);
        let err = r
            .run("remote.remove", &mut s, json!({ "name": "origin" }))
            .unwrap_err();
        assert!(err.to_string().contains("promisor"), "{key}: {err}");
        assert_eq!(remote_refs(&s, "origin").len(), 2);
    }
}

#[test]
fn cancelled_fetch_of_remote_add_names_the_command() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let token = crate::CancelToken::new();
    token.cancel();
    let out = r
        .run_with(
            "remote.add",
            &mut s,
            json!({ "name": "fork", "url": "/srv/fork.git", "fetch": true }),
            crate::JobContext::new().with_cancel(token),
        )
        .unwrap();
    assert_eq!(out["fetch_error"]["kind"], "cancelled");
    assert!(
        out["fetch_error"]["message"]
            .as_str()
            .unwrap()
            .contains("remote.add")
    );
}

#[test]
fn masked_values_and_valueless_keys() {
    let mut repo = MemoryBackend::sample();
    repo.set_config("remote.origin.proxy", &["http://u:segreto@proxy:3128"]);
    let (r, mut s) = opened_with(repo);
    let out = run(
        &r,
        &mut s,
        "remote.remove",
        json!({ "name": "origin", "confirm": true }),
    );
    assert!(!out.to_string().contains("segreto"), "{out}");
    assert!(out.to_string().contains("http://u:***@proxy:3128"), "{out}");

    let mut a = crate::RemoteSnapshot {
        name: "o".into(),
        config: vec![crate::ConfigEntry {
            key: "remote.o.skipFetchAll".into(),
            value: None,
        }],
        ..crate::RemoteSnapshot::default()
    };
    let mut b = a.clone();
    b.config[0].value = Some("true".into());
    assert!(a.differences(&b).is_empty());
    a.config[0].value = Some("false".into());
    assert!(!a.differences(&b).is_empty());
}

#[test]
fn a_remote_still_listed_after_removal_is_put_back() {
    let mut repo = MemoryBackend::sample();
    repo.keep_remote_listed("origin");
    let (r, mut s) = opened_with(repo);
    let before = s.repo().unwrap().remote_snapshot("origin").unwrap();
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("rimesso com'era"), "{err}");
    assert_eq!(s.repo().unwrap().remote_snapshot("origin").unwrap(), before);
    // Rimesso tutto: niente nel diario.
    assert_eq!(
        run(&r, &mut s, "edit.history", json!({}))["can_undo"],
        false
    );
}

#[test]
fn a_failed_put_back_writes_the_config_and_keeps_the_journal_entry() {
    let mut repo = MemoryBackend::sample();
    repo.keep_remote_listed("origin");
    // La rimozione riesce, il ripristino dei branch remoti no.
    repo.fail_restore_refs_after(1);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run(
            "remote.remove",
            &mut s,
            json!({ "name": "origin", "confirm": true }),
        )
        .unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("solo in parte") && text.contains("diario"),
        "{text}"
    );
    // La configurazione è stata riscritta comunque; i branch remoti no.
    let now = s.repo().unwrap().remote_snapshot("origin").unwrap();
    assert_eq!(now.url(), Some("https://example.com/progetto.git"));
    assert!(now.refs.is_empty());
    assert_eq!(
        upstream_of(&s, "main").as_deref(),
        Some("refs/remotes/origin/main")
    );
    let history = run(&r, &mut s, "edit.history", json!({}));
    assert_eq!(history["done"][0]["command"], "remote.remove");
}
