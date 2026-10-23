//! Test di `remote.pull` e `remote.push` sul `MemoryBackend` (i backend sono provati anche in
//! `git/memory/sync_tests.rs` e, con git vero, in `crates/git/tests/{pull,push}.rs`).

use crate::{
    CancelToken, EngineError, FetchFailure, GitBackend, JobContext, MemoryBackend, MemoryOpener,
    PullOperation, PullSettings, PushRejection, Registry, Session,
};
use project_domain::{Change, ChangedFile, Commit, Oid, RepoPath, Signature};
use serde_json::json;
use std::sync::Arc;

fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
    let mut opener = MemoryOpener::default();
    opener.insert("r", repo);
    let mut s = Session::new(Arc::new(opener));
    let r = Registry::builtin();
    r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
    (r, s)
}

fn oid(n: u64) -> Oid {
    MemoryBackend::fake_oid(n)
}

fn target(r: &Registry, s: &mut Session, name: &str) -> Option<String> {
    r.run("repo.refs", s, json!({}))
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == name)
        .map(|x| x["target"].as_str().unwrap().to_owned())
}

fn who() -> Signature {
    Signature {
        name: "Carla".into(),
        email: "carla@example.com".into(),
        time: 1_700_000_000 + 20 * 3_600,
        offset_min: 0,
    }
}

/// Il repo di esempio senza modifiche locali, con `origin/main` che ha un commit (20) dopo
/// `11` che `main` (su `12`) non ha: branch divergenti. Il commit 20 cambia `file`, se c'è.
fn diverged(file: Option<&str>) -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.set_entries(Vec::new());
    repo.insert_remote_commit(
        "origin",
        Commit {
            id: oid(20),
            parents: vec![oid(11)],
            author: who(),
            committer: who(),
            summary: "Dal remoto".into(),
            body: String::new(),
        },
    );
    if let Some(file) = file {
        repo.set_blob(oid(5_000), "dal remoto\n");
        repo.set_changes(
            oid(20),
            vec![ChangedFile {
                path: RepoPath::from(file),
                change: Change::Added,
                original: None,
                score: None,
                old_mode: 0,
                new_mode: 0o100644,
                old_oid: Oid::from_bytes([0; Oid::LEN]),
                new_oid: oid(5_000),
            }],
        );
    }
    repo.set_branch_on_remote("origin", "main", oid(20));
    repo
}

#[test]
fn pull_of_a_branch_that_is_not_checked_out() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = r
        .run("remote.pull", &mut s, json!({ "branch": "feature/grafo" }))
        .unwrap();
    assert_eq!(out["strategy"], "fast_forward");
    assert_eq!(out["before"], oid(9).to_string());
    assert_eq!(out["after"], oid(13).to_string());
    assert_eq!(out["backup"], "refs/project/backup/pull-1");
    assert_eq!(out["upstream"], "refs/remotes/origin/feature/grafo");
    assert_eq!(
        out["fetch"]["updated"][0]["name"],
        "refs/remotes/origin/feature/grafo"
    );
    assert_eq!(
        target(&r, &mut s, "refs/heads/feature/grafo").unwrap(),
        oid(13).to_string()
    );
    // HEAD e le modifiche locali dell'esempio non si toccano.
    assert_eq!(out["status"]["head"]["Branch"]["name"], "main");
    assert_eq!(out["status"]["entries"].as_array().unwrap().len(), 4);
    assert!(
        out["next_undo"]
            .as_str()
            .unwrap()
            .starts_with("Pull di `feature/grafo` (fast-forward")
    );
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/feature/grafo").unwrap(),
        oid(9).to_string()
    );
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/feature/grafo").unwrap(),
        oid(13).to_string()
    );

    // `main` contiene già `origin/main`: niente da fare.
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "up_to_date");
    assert_eq!(
        (out["ahead"].as_u64(), out["behind"].as_u64()),
        (Some(1), Some(0))
    );
}

#[test]
fn pull_merge_rebase_and_undo() {
    // Modifiche locali: un merge non parte.
    let mut dirty = diverged(None);
    dirty.set_entries(MemoryBackend::sample().status().unwrap().entries);
    let (r, mut s) = opened_with(dirty);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("autostash"), "{err}");

    let (r, mut s) = opened_with(diverged(Some("nuovo.txt")));
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "merge");
    assert_eq!(out["strategy_source"], "default");
    let merge = out["after"].as_str().unwrap().to_owned();
    let log = r.run("repo.log", &mut s, json!({ "limit": 1 })).unwrap();
    let head = &log["commits"][0];
    assert_eq!(head["id"], merge);
    assert_eq!(
        head["parents"],
        json!([oid(12).to_string(), oid(20).to_string()])
    );
    assert_eq!(
        head["summary"],
        "Merge remote-tracking branch 'origin/main'"
    );
    // Il file arrivato è nell'albero di HEAD e lo stato è pulito.
    let show = r
        .run("commit.show", &mut s, json!({ "id": merge }))
        .unwrap();
    assert_eq!(show["files"][0]["path"], "nuovo.txt");
    assert_eq!(out["status"]["entries"], json!([]));

    let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        undo["status"]["head"]["Branch"]["target"],
        oid(12).to_string()
    );
    assert_eq!(undo["backups"], json!(["refs/project/backup/undo-1"]));
    assert_eq!(undo["status"]["entries"], json!([]));
    r.run("edit.redo", &mut s, json!({})).unwrap();
    assert_eq!(target(&r, &mut s, "refs/heads/main").unwrap(), merge);

    // Rebase dalla configurazione: il commit 12 riscritto sopra 20.
    let mut repo = diverged(Some("nuovo.txt"));
    repo.set_pull_settings(PullSettings {
        rebase: Some("true".into()),
        ff: None,
    });
    let (r, mut s) = opened_with(repo);
    let out = r.run("remote.pull", &mut s, json!({})).unwrap();
    assert_eq!(out["strategy"], "rebase");
    assert_eq!(out["strategy_source"], "config");
    let log = r.run("repo.log", &mut s, json!({ "limit": 2 })).unwrap();
    assert_eq!(log["commits"][0]["summary"], "Pannello dei commit");
    assert_eq!(log["commits"][0]["parents"], json!([oid(20).to_string()]));
    assert_eq!(log["commits"][1]["id"], oid(20).to_string());
    r.run("edit.undo", &mut s, json!({})).unwrap();
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );

    // Solo fast-forward: rifiutato, ma il fetch è fatto.
    let (r, mut s) = opened_with(diverged(None));
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "ff_only" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/main").unwrap(),
        oid(20).to_string()
    );
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
}

#[test]
fn pull_conflict_changes_nothing() {
    // 12 crea `src/bin/app.rs` (rinomina), e il remoto lo crea con un altro contenuto.
    for rebase in [false, true] {
        let mut repo = diverged(Some("src/bin/app.rs"));
        if rebase {
            repo.set_pull_settings(PullSettings {
                rebase: Some("true".into()),
                ff: None,
            });
        }
        let (r, mut s) = opened_with(repo);
        let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
        let EngineError::PullConflict {
            operation, files, ..
        } = &err
        else {
            panic!("{err}");
        };
        let expected = if rebase {
            PullOperation::Rebase
        } else {
            PullOperation::Merge
        };
        assert_eq!(*operation, expected);
        assert_eq!(files, &["src/bin/app.rs"]);
        assert!(err.to_string().contains("annullato"), "{err}");
        assert_eq!(
            target(&r, &mut s, "refs/heads/main").unwrap(),
            oid(12).to_string()
        );
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_undo"], false);
    }
}

#[test]
fn pull_refusals() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    r.run("branch.create", &mut s, json!({ "name": "solo" }))
        .unwrap();
    let err = r
        .run("remote.pull", &mut s, json!({ "branch": "solo" }))
        .unwrap_err();
    assert!(err.to_string().contains("--set-upstream-to"), "{err}");
    let err = r
        .run("remote.pull", &mut s, json!({ "remote": "fork" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    let err = r
        .run("remote.pull", &mut s, json!({ "branch": "manca" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.detach_head(oid(11));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("staccata"), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_operation(Some(project_domain::Operation::Rebase));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("rebase"), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_pull_settings(PullSettings {
        rebase: Some("interactive".into()),
        ff: None,
    });
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("interattivo"), "{err}");
    // Niente scaricato.
    let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
    assert!(!refs.to_string().contains(&oid(13).to_string()));

    let mut repo = MemoryBackend::sample();
    repo.fail_fetch("origin", Some(FetchFailure::Auth("no".into())));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.pull", &mut s, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::Auth { .. }), "{err}");

    let (r, mut s) = opened_with(MemoryBackend::sample());
    let token = CancelToken::new();
    token.cancel();
    let err = r
        .run_with(
            "remote.pull",
            &mut s,
            json!({ "branch": "feature/grafo" }),
            JobContext::new().with_cancel(token),
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
}

#[test]
fn push_of_the_sample() {
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["kind"], "fast_forward");
    assert_eq!(out["before"], oid(11).to_string());
    assert_eq!(out["after"], oid(12).to_string());
    assert_eq!(out["set_upstream"], false);
    assert_eq!(
        out["updated"],
        json!([{
            "name": "refs/remotes/origin/main",
            "before": oid(11).to_string(),
            "after": oid(12).to_string(),
        }])
    );
    assert_eq!(out["messages"][0], "To https://example.com/progetto.git");
    let history = r.run("edit.history", &mut s, json!({})).unwrap();
    assert_eq!(history["can_undo"], false);
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["kind"], "up_to_date");

    // Primo push di un branch nuovo: l'upstream si imposta.
    r.run("branch.create", &mut s, json!({ "name": "nuovo" }))
        .unwrap();
    let out = r
        .run("remote.push", &mut s, json!({ "branch": "nuovo" }))
        .unwrap();
    assert_eq!(out["kind"], "new");
    assert_eq!(out["set_upstream"], true);
    let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
    let nuovo = refs
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "refs/heads/nuovo")
        .unwrap();
    assert_eq!(nuovo["upstream"]["name"], "refs/remotes/origin/nuovo");
    // E ora anche il pull lo trova.
    let out = r
        .run("remote.pull", &mut s, json!({ "branch": "nuovo" }))
        .unwrap();
    assert_eq!(out["strategy"], "up_to_date");
}

#[test]
fn push_rejections_and_force() {
    // Il remoto ha 20, che qui non c'è: rifiutato, niente cambia.
    let (r, mut s) = opened_with(diverged(None));
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::NonFastForward,
                ..
            }
        ),
        "{err}"
    );
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/main").unwrap(),
        oid(11).to_string()
    );
    // Il force con il branch remoto locale vecchio (11): il lease fallisce.
    let err = r
        .run("remote.push", &mut s, json!({ "force": true }))
        .unwrap_err();
    assert!(
        matches!(
            err,
            EngineError::PushRejected {
                reason: PushRejection::StaleLease,
                ..
            }
        ),
        "{err}"
    );
    // Dopo il fetch: il force toglierebbe 20, serve la conferma.
    r.run("remote.fetch", &mut s, json!({})).unwrap();
    let err = r
        .run("remote.push", &mut s, json!({ "force": true }))
        .unwrap_err();
    let EngineError::ConfirmRequired {
        reason, expected, ..
    } = &err
    else {
        panic!("{err}");
    };
    assert!(reason.contains("«Dal remoto»"), "{reason}");
    assert_eq!(expected.as_deref(), Some(oid(20).to_string().as_str()));
    assert!(
        reason.contains(&format!("\"expected\": \"{}\"", oid(20))),
        "{reason}"
    );
    // Un `expected` che non è il branch remoto locale: si chiede di nuovo, niente pubblicato.
    let err = r
        .run(
            "remote.push",
            &mut s,
            json!({ "force": true, "confirm": true, "expected": oid(11).to_string() }),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
    let out = r
        .run(
            "remote.push",
            &mut s,
            json!({ "force": true, "confirm": true, "expected": oid(20).to_string() }),
        )
        .unwrap();
    assert_eq!(out["kind"], "forced");
    assert_eq!(out["backup"], "refs/project/backup/push-1");
    assert_eq!(out["lost"][0]["id"], oid(20).to_string());
    assert_eq!(out["before"], oid(20).to_string());

    // Hook `pre-push`, credenziali, annullamento.
    let mut repo = MemoryBackend::sample();
    repo.fail_pre_push(Some("pre-push: no"));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(
            &err,
            EngineError::PushRejected { reason: PushRejection::Hook, message, .. } if message == "pre-push: no"
        ),
        "{err}"
    );
    let mut repo = MemoryBackend::sample();
    repo.fail_fetch("origin", Some(FetchFailure::Unreachable("giù".into())));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(
        matches!(err, EngineError::RemoteUnreachable { .. }),
        "{err}"
    );
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let token = CancelToken::new();
    token.cancel();
    let err = r
        .run_with(
            "remote.push",
            &mut s,
            json!({}),
            JobContext::new().with_cancel(token),
        )
        .unwrap_err();
    assert!(matches!(err, EngineError::Cancelled { .. }), "{err}");
    assert_eq!(
        target(&r, &mut s, "refs/remotes/origin/main").unwrap(),
        oid(11).to_string()
    );
}

#[test]
fn push_refusals() {
    let mut repo = MemoryBackend::sample();
    repo.detach_head(oid(12));
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("staccata"), "{err}");

    let mut repo = MemoryBackend::sample();
    repo.set_upstream("feature/grafo", "origin/main");
    let (r, mut s) = opened_with(repo);
    let err = r
        .run("remote.push", &mut s, json!({ "branch": "feature/grafo" }))
        .unwrap_err();
    assert!(err.to_string().contains("simple"), "{err}");
    for params in [
        json!({ "remote": "nessuno" }),
        json!({ "remote": "-x" }),
        json!({ "branch": "manca" }),
    ] {
        let err = r.run("remote.push", &mut s, params.clone()).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
    }
    for params in [
        json!({ "confirm": true }),
        json!({ "force": true, "confirm": true }),
        json!({ "force": true, "expected": "1234" }),
        json!({ "tags": true }),
        json!({ "branch": "" }),
        json!({ "remote": "a b" }),
    ] {
        let err = r.run("remote.push", &mut s, params.clone()).unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { .. }),
            "{params}: {err}"
        );
    }
}

#[test]
fn need_an_open_repo() {
    let r = Registry::builtin();
    let mut s = Session::new(Arc::new(MemoryOpener::sample()));
    for id in ["remote.pull", "remote.push"] {
        let err = r.run(id, &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
    }
}

#[test]
fn push_remote_from_the_configuration() {
    let mut repo = MemoryBackend::sample();
    repo.add_remote("fork", "https://example.com/fork.git");
    repo.set_config("remote.pushDefault", &["fork"]);
    let (r, mut s) = opened_with(repo.clone());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["remote"], "fork");
    assert_eq!(out["remote_source"], "push_default");
    assert_eq!(out["kind"], "new");
    assert_eq!(out["set_upstream"], false);
    assert_eq!(out["updated"][0]["name"], "refs/remotes/fork/main");
    repo.set_config("branch.main.pushRemote", &["origin"]);
    let (r, mut s) = opened_with(repo.clone());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["remote_source"], "push_remote");
    assert_eq!(out["remote"], "origin");
    repo.set_config("remote.origin.mirror", &["true"]);
    let (r, mut s) = opened_with(repo);
    let err = r.run("remote.push", &mut s, json!({})).unwrap_err();
    assert!(err.to_string().contains("mirror"), "{err}");
    let (r, mut s) = opened_with(MemoryBackend::sample());
    let out = r.run("remote.push", &mut s, json!({})).unwrap();
    assert_eq!(out["remote_source"], "upstream");
}

#[test]
fn pull_never_overwrites_ignored_files() {
    // Il remoto aggiunge `segreto.env`, che qui è ignorato: il pull è rifiutato.
    let mut repo = diverged(Some("segreto.env"));
    repo.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("segreto.env"),
        index: Change::Ignored,
        worktree: Change::Ignored,
        kind: project_domain::EntryKind::Ignored,
    }]);
    for strategy in ["merge", "rebase"] {
        let (r, mut s) = opened_with(repo.clone());
        let err = r
            .run("remote.pull", &mut s, json!({ "strategy": strategy }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(err.to_string().contains("segreto.env"), "{err}");
        assert_eq!(
            target(&r, &mut s, "refs/heads/main").unwrap(),
            oid(12).to_string()
        );
    }
}

#[test]
fn rebase_never_passes_over_ignored_files() {
    // Il commit 12 (da riapplicare) ha `src/bin/app.rs`; qui al suo posto c'è un ignorato,
    // come se un commit successivo l'avesse tolto: il rebase è rifiutato, il merge no.
    let mut repo = diverged(None);
    repo.set_entries(vec![project_domain::StatusEntry {
        path: RepoPath::from("src/bin/app.rs"),
        index: Change::Ignored,
        worktree: Change::Ignored,
        kind: project_domain::EntryKind::Ignored,
    }]);
    let (r, mut s) = opened_with(repo);
    let err = r
        .run("remote.pull", &mut s, json!({ "strategy": "rebase" }))
        .unwrap_err();
    assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    assert!(err.to_string().contains("src/bin/app.rs"), "{err}");
    assert_eq!(
        target(&r, &mut s, "refs/heads/main").unwrap(),
        oid(12).to_string()
    );
}
