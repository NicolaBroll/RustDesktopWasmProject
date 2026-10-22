//! Comandi dei remoti (S25): `remote.fetch`. Comando lento (§6): l'avanzamento e
//! l'annullamento arrivano da [`Session::job`].

use super::{Out, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, FetchRequest, Session, check_branch_name, check_remote_name, default_remote,
    unknown_remote,
};
use project_domain::{Oid, RefInfo};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// Parametri di `remote.fetch`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FetchParams {
    #[serde(default)]
    remote: Option<String>,
    #[serde(default)]
    branch: Option<String>,
}

/// Un ref cambiato dal fetch: `before`/`after` `null` = il ref non c'era / non c'è più (un
/// branch remoto tolto da `fetch.prune` configurato).
#[derive(Debug, Serialize)]
struct RefUpdate {
    name: String,
    before: Option<Oid>,
    after: Option<Oid>,
}

/// Il valore di ogni ref per il confronto prima/dopo: per un tag annotato conta anche l'oggetto
/// tag (un tag spostato su un altro oggetto con lo stesso commit è cambiato).
fn values(refs: &[RefInfo]) -> BTreeMap<&str, (Oid, Option<Oid>)> {
    refs.iter()
        .map(|r| (r.name.as_str(), (r.target, r.tag_object)))
        .collect()
}

/// I ref diversi fra `before` e `after`, in ordine di nome. Il valore riportato è il commit (per
/// un tag annotato, quello puntato).
fn changes(before: &[RefInfo], after: &[RefInfo]) -> Vec<RefUpdate> {
    let (old, new) = (values(before), values(after));
    let mut names: Vec<&str> = old.keys().chain(new.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .filter(|name| old.get(name) != new.get(name))
        .map(|name| RefUpdate {
            name: name.to_owned(),
            before: old.get(name).map(|v| v.0),
            after: new.get(name).map(|v| v.0),
        })
        .collect()
}

/// Avviso per un ref fuori da `refs/remotes/` e `refs/tags/` spostato dal fetch: succede solo
/// con un refspec configurato apposta (es. un mirror verso `refs/heads/`). Il valore di prima
/// resta nel reflog di quel ref.
fn unexpected(update: &RefUpdate) -> Option<String> {
    let expected =
        update.name.starts_with("refs/remotes/") || update.name.starts_with("refs/tags/");
    if expected {
        return None;
    }
    let show = |v: Option<Oid>| v.map_or_else(|| "assente".to_owned(), |id| id.to_string());
    Some(format!(
        "il fetch ha cambiato `{}` (da {} a {}) per un refspec configurato in \
         `remote.<nome>.fetch`: il valore di prima è nel reflog del ref",
        update.name,
        show(update.before),
        show(update.after)
    ))
}

/// Scarica dal remoto `remote` (vedi [`GitBackend::fetch`](crate::GitBackend::fetch)): commit
/// nuovi, branch remoti e tag come dice la configurazione; con `branch` solo quel branch del
/// remoto. `remote` è il nome di un remoto **configurato** (mai un URL o un percorso:
/// [`EngineError::NotAllowed`] se non c'è); se manca, quello dell'upstream del branch attivo,
/// altrimenti `origin`, altrimenti l'unico remoto. Non tocca branch locali, indice, worktree:
/// non va nel diario delle operazioni (i branch remoti rispecchiano il remoto; il fetch dopo
/// un undo li riporterebbe dov'erano). Nessun `--prune` da qui: vale `fetch.prune` /
/// `remote.<nome>.prune` della configurazione.
///
/// Comando lento: l'avanzamento va al [`JobContext`](crate::JobContext) di
/// [`Registry::run_with`](crate::Registry::run_with); annullato, dà
/// [`EngineError::Cancelled`] (i ref già aggiornati da git restano).
///
/// Risultato: `{"remote", "branch": string | null, "updated": [{"name", "before", "after"}] |
/// null, "messages": [string], "warnings": [string], "refs": [RefInfo] | null, "refs_error"?}`.
pub(super) fn remote_fetch(s: &mut Session, p: Value) -> Out {
    const ID: &str = "remote.fetch";
    let FetchParams { remote, branch } = parse_params(ID, p)?;
    if let Some(remote) = &remote {
        check_remote_name(ID, remote)?;
    }
    if let Some(branch) = &branch {
        check_branch_name(ID, branch)?;
    }
    let job = s.job().clone();
    let repo = s.repo_mut()?;
    let remotes = repo.remotes()?;
    let remote = match remote {
        Some(remote) if remotes.contains(&remote) => remote,
        Some(remote) => return Err(unknown_remote(&remote, &remotes)),
        None => default_remote(ID, &repo.status()?, &remotes)?,
    };
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    let before = repo.refs()?;
    let request = FetchRequest {
        remote: remote.clone(),
        branch: branch.clone(),
    };
    let outcome = repo.fetch(&request, &job)?;
    let mut out = json!({
        "remote": remote,
        "branch": branch,
        "messages": outcome.messages,
    });
    match repo.refs() {
        Ok(after) => {
            let updated = changes(&before, &after);
            let warnings: Vec<String> = updated.iter().filter_map(unexpected).collect();
            out["updated"] = to_json(updated);
            out["warnings"] = to_json(warnings);
            out["refs"] = to_json(after);
        }
        Err(e) => {
            out["updated"] = Value::Null;
            out["warnings"] = json!([]);
            out["refs"] = Value::Null;
            out["refs_error"] = Value::String(e.to_string());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use crate::{
        CancelToken, EngineError, FetchFailure, JobContext, MemoryBackend, MemoryOpener, Progress,
        Registry, Session,
    };
    use project_domain::Oid;
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex};

    fn opened_with(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::default();
        opener.insert("r", repo);
        let mut s = Session::new(Arc::new(opener));
        let r = Registry::builtin();
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        (r, s)
    }

    fn sample() -> (Registry, Session) {
        opened_with(MemoryBackend::sample())
    }

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    fn target(out: &Value, name: &str) -> Option<String> {
        out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .map(|r| r["target"].as_str().unwrap().to_owned())
    }

    #[test]
    fn fetch_brings_new_commits_and_reports_the_updated_refs() {
        let (r, mut s) = sample();
        let out = r.run("remote.fetch", &mut s, json!({})).unwrap();
        assert_eq!(out["remote"], "origin");
        assert_eq!(out["branch"], Value::Null);
        assert_eq!(
            out["updated"],
            json!([{
                "name": "refs/remotes/origin/feature/grafo",
                "before": oid(9).to_string(),
                "after": oid(13).to_string(),
            }])
        );
        assert_eq!(out["warnings"], json!([]));
        let messages = out["messages"].as_array().unwrap();
        assert_eq!(messages[0], "From https://example.com/progetto.git");
        assert!(
            messages[1]
                .as_str()
                .unwrap()
                .contains("feature/grafo -> origin/feature/grafo")
        );
        assert_eq!(
            target(&out, "refs/remotes/origin/feature/grafo").unwrap(),
            oid(13).to_string()
        );
        // Il commit nuovo è nel repo, e `feature/grafo` è indietro di uno.
        let log = r
            .run(
                "repo.log",
                &mut s,
                json!({ "rev": "origin/feature/grafo", "limit": 1 }),
            )
            .unwrap();
        assert_eq!(log["commits"][0]["summary"], "Corsie: un colore per branch");
        let local = out["refs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "refs/heads/feature/grafo")
            .unwrap();
        assert_eq!(local["upstream"]["behind"], 1);
        // Non va nel diario: niente da annullare.
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_undo"], false);

        // Il secondo fetch non porta niente.
        let again = r
            .run("remote.fetch", &mut s, json!({ "remote": "origin" }))
            .unwrap();
        assert_eq!(again["updated"], json!([]));
        assert_eq!(again["messages"], json!([]));
    }

    #[test]
    fn fetch_of_one_branch() {
        let (r, mut s) = sample();
        let out = r
            .run(
                "remote.fetch",
                &mut s,
                json!({ "remote": "origin", "branch": "main" }),
            )
            .unwrap();
        assert_eq!(out["branch"], "main");
        assert_eq!(out["updated"], json!([]));
        let out = r
            .run("remote.fetch", &mut s, json!({ "branch": "feature/grafo" }))
            .unwrap();
        assert_eq!(
            out["updated"][0]["name"],
            "refs/remotes/origin/feature/grafo"
        );

        let err = r
            .run("remote.fetch", &mut s, json!({ "branch": "manca" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(err.to_string().contains("manca"), "{err}");
    }

    #[test]
    fn a_new_remote_branch_and_a_new_remote() {
        let mut repo = MemoryBackend::sample();
        repo.add_remote("fork", "/srv/fork.git");
        repo.set_branch_on_remote("fork", "-x", oid(12));
        let (r, mut s) = opened_with(repo);
        let out = r
            .run("remote.fetch", &mut s, json!({ "remote": "fork" }))
            .unwrap();
        assert_eq!(
            out["updated"],
            json!([{ "name": "refs/remotes/fork/-x", "before": null, "after": oid(12).to_string() }])
        );
        assert!(
            out["messages"][1]
                .as_str()
                .unwrap()
                .contains("[new branch]")
        );
    }

    #[test]
    fn unknown_and_invalid_remotes_never_reach_the_backend() {
        let (r, mut s) = sample();
        // Un nome che non è un remoto configurato (git lo userebbe come URL o percorso).
        for remote in ["altro", "tmp/repo", "-x", "--upload-pack=x"] {
            let err = r
                .run("remote.fetch", &mut s, json!({ "remote": remote }))
                .unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{remote}: {err}");
            assert!(err.to_string().contains("`origin`"), "{err}");
        }
        for remote in ["", "a b", "a..b", "https://x/y:z", "/tmp/repo", "../x"] {
            let err = r
                .run("remote.fetch", &mut s, json!({ "remote": remote }))
                .unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{remote}: {err}"
            );
        }
        for branch in ["", "a..b", "refs/heads/main"] {
            let err = r
                .run("remote.fetch", &mut s, json!({ "branch": branch }))
                .unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{branch}: {err}"
            );
        }
        let err = r
            .run("remote.fetch", &mut s, json!({ "prune": true }))
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
    }

    #[test]
    fn default_remote_without_origin() {
        let mut repo = MemoryBackend::new();
        let (r, mut s) = opened_with(repo.clone());
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(err.to_string().contains("non ha remoti"), "{err}");

        repo.add_remote("a", "/a");
        repo.add_remote("b", "/b");
        let (r, mut s) = opened_with(repo.clone());
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        // Con l'upstream del branch attivo su `b`, si usa `b`.
        repo.set_upstream("main", "b/main");
        let (r, mut s) = opened_with(repo);
        let out = r.run("remote.fetch", &mut s, json!({})).unwrap();
        assert_eq!(out["remote"], "b");
    }

    #[test]
    fn credential_and_network_errors_are_distinct() {
        let mut repo = MemoryBackend::sample();
        repo.fail_fetch("origin", Some(FetchFailure::Auth("HTTP 401".into())));
        let (r, mut s) = opened_with(repo.clone());
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(
            matches!(&err, EngineError::Auth { remote, message } if remote == "origin" && message == "HTTP 401"),
            "{err}"
        );

        repo.fail_fetch(
            "origin",
            Some(FetchFailure::Unreachable("Could not resolve host".into())),
        );
        let (r, mut s) = opened_with(repo);
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(
            matches!(err, EngineError::RemoteUnreachable { .. }),
            "{err}"
        );
        // Niente è cambiato.
        let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
        let grafo = refs
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "refs/remotes/origin/feature/grafo")
            .unwrap();
        assert_eq!(grafo["target"], oid(9).to_string());
    }

    #[test]
    fn progress_goes_to_the_job_and_cancel_stops_the_fetch() {
        let (r, mut s) = sample();
        let seen: Arc<Mutex<Vec<Progress>>> = Arc::default();
        let sink = Arc::clone(&seen);
        let job = JobContext::new().with_progress(move |p| sink.lock().unwrap().push(p.clone()));
        r.run_with("remote.fetch", &mut s, json!({}), job).unwrap();
        let seen = seen.lock().unwrap();
        assert!(!seen.is_empty());
        let last = seen.last().unwrap();
        assert_eq!(last.phase, "Receiving objects");
        assert_eq!(last.percent, Some(100));
        assert!(last.done);
        // Dopo il comando la sessione torna a un contesto vuoto.
        assert!(!s.job().is_cancelled());

        let (r, mut s) = sample();
        let token = CancelToken::new();
        token.cancel();
        let err = r
            .run_with(
                "remote.fetch",
                &mut s,
                json!({}),
                JobContext::new().with_cancel(token),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::Cancelled { .. }), "{err}");
        let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
        assert!(
            refs.as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "refs/remotes/origin/feature/grafo"
                    && r["target"] == oid(9).to_string())
        );
    }

    #[test]
    fn needs_an_open_repo() {
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        let err = r.run("remote.fetch", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NoRepo), "{err}");
    }
}
