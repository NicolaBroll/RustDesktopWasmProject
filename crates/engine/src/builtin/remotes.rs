//! Gestione dei remoti (S27): `remote.list`, `remote.add`, `remote.remove`. Le parti comuni
//! ai backend (controlli dei nomi e degli URL, mascheratura delle credenziali, eliminazione e
//! ripristino di un remoto) sono in `git/remote_admin.rs`; qui i comandi.

use super::remote::{changes, unexpected};
use super::{Out, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, FetchRequest, GitBackend, HeadState, JobContext, JournalEntry, RemoteSnapshot,
    Session, check_new_remote_name, check_no_operation, check_remote_name, check_remote_url,
    default_remote, ensure_remote_gone, mask_url, owned_by, remove_remote,
};
use project_domain::{RefInfo, RefKind};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {}

/// Parametri di `remote.add`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddParams {
    name: String,
    url: String,
    #[serde(default)]
    fetch: bool,
}

/// Parametri di `remote.remove`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveParams {
    name: String,
    #[serde(default)]
    confirm: bool,
}

/// Vero per un valore booleano di git (`true`, `yes`, `on`, `1`).
fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "true" | "yes" | "on" | "1" | ""
        )
    })
}

/// I remoti configurati come li dà `remote.list` (vedi lì).
fn remote_items(repo: &dyn GitBackend) -> Result<Value, EngineError> {
    let remotes = repo.remotes()?;
    let refs = repo.refs()?;
    let default = repo
        .status()
        .ok()
        .and_then(|status| default_remote("remote.list", &status, &remotes).ok());
    // Come git: l'ultimo valore vince.
    let push_default = repo.config_values("remote.pushDefault")?.pop();
    let mut items = Vec::new();
    for name in &remotes {
        let urls = repo.config_values(&format!("remote.{name}.url"))?;
        let push = repo.config_values(&format!("remote.{name}.pushurl"))?;
        let mut credentials = false;
        let mut show = |url: &String| {
            let (shown, masked) = mask_url(url);
            credentials |= masked;
            shown
        };
        let url = urls.first().map(&mut show);
        // Senza `pushurl` il push va a tutti gli `url`: si elencano solo se sono più di uno.
        let push_urls: Vec<String> = if !push.is_empty() {
            push.iter().map(&mut show).collect()
        } else if urls.len() > 1 {
            urls.iter().map(&mut show).collect()
        } else {
            Vec::new()
        };
        let fetch = repo.config_values(&format!("remote.{name}.fetch"))?;
        let mirror = truthy(repo.config_values(&format!("remote.{name}.mirror"))?.last());
        let branches = refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch && owned_by(name, &remotes, &r.name))
            .count();
        items.push(json!({
            "name": name,
            "url": url,
            "push_urls": push_urls,
            "fetch": fetch,
            "mirror": mirror,
            "branches": branches,
            "default": default.as_ref() == Some(name),
            "push_default": push_default.as_ref() == Some(name),
            "credentials": credentials,
        }));
    }
    Ok(json!({
        "remotes": items,
        "default": default,
        "push_default": push_default,
    }))
}

fn with_refs(out: &mut Value, refs: Result<Vec<RefInfo>, EngineError>) {
    match refs {
        Ok(refs) => out["refs"] = to_json(refs),
        Err(e) => {
            out["refs"] = Value::Null;
            out["refs_error"] = Value::String(e.to_string());
        }
    }
}

fn with_remotes(out: &mut Value, repo: &dyn GitBackend) {
    match remote_items(repo) {
        Ok(list) => out["remotes"] = list["remotes"].clone(),
        Err(e) => {
            out["remotes"] = Value::Null;
            out["remotes_error"] = Value::String(e.to_string());
        }
    }
}

/// I remoti configurati, in ordine di nome (sola lettura).
///
/// Risultato: `{"remotes": [{"name", "url": string | null, "push_urls": [string], "fetch":
/// [string], "mirror": bool, "branches": n, "default": bool, "push_default": bool,
/// "credentials": bool}], "default": string | null, "push_default": string | null}`.
///
/// - `url`: il primo `remote.<nome>.url` (quello del fetch), com'è scritto nella configurazione
///   (senza applicare `url.<base>.insteadOf`); `null` per un remoto senza URL.
/// - `push_urls`: dove va il push se non è `url` (i `remote.<nome>.pushurl`, oppure tutti gli
///   `url` se sono più di uno); vuoto = il push va a `url`.
/// - Le credenziali negli URL **non si mostrano mai**: `https://utente:token@host` diventa
///   `https://utente:***@host`, e in un URL `http(s)`/`ftp(s)` con solo l'utente
///   (`https://token@host`) anche l'utente diventa `***`; `credentials` dice che l'URL ne aveva
///   (vedi [`mask_url`]).
/// - `fetch`: i refspec `remote.<nome>.fetch`; `mirror`: `remote.<nome>.mirror`.
/// - `branches`: quanti branch remoti `refs/remotes/<nome>/...` ci sono (senza quelli di un
///   remoto `<nome>/...`).
/// - `default`: il remoto che `remote.fetch` usa senza `remote` (upstream del branch attivo,
///   `origin`, l'unico remoto; `null` se ambiguo); `push_default`: `remote.pushDefault`
///   (l'ultimo valore, anche se non è un remoto configurato).
pub(super) fn remote_list(s: &mut Session, p: Value) -> Out {
    let ListParams {} = parse_params("remote.list", p)?;
    remote_items(s.repo()?)
}

/// Il tipo di errore di un fetch per `fetch_error.kind`.
fn error_kind(e: &EngineError) -> &'static str {
    match e {
        EngineError::Auth { .. } => "auth",
        EngineError::RemoteUnreachable { .. } => "unreachable",
        EngineError::Cancelled { .. } => "cancelled",
        _ => "other",
    }
}

/// Fetch di tutto il remoto `remote` (come `remote.fetch`): `{"updated", "messages",
/// "warnings"}`.
fn fetch_all(
    repo: &mut dyn GitBackend,
    remote: &str,
    job: &JobContext,
    command: &'static str,
) -> Result<Value, EngineError> {
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command });
    }
    let before = repo.refs()?;
    let outcome = repo
        .fetch(
            &FetchRequest {
                remote: remote.to_owned(),
                branch: None,
            },
            job,
        )
        .map_err(|e| match e {
            EngineError::Cancelled { .. } => EngineError::Cancelled { command },
            e => e,
        })?;
    let after = repo.refs()?;
    let updated = changes(&before, &after);
    let warnings: Vec<String> = updated.iter().filter_map(unexpected).collect();
    Ok(json!({
        "updated": to_json(updated),
        "messages": outcome.messages,
        "warnings": warnings,
    }))
}

/// Aggiunge il remoto `name` con `url` (vedi
/// [`GitBackend::create_remote`](crate::GitBackend::create_remote): `git remote add`, refspec
/// di default) e, con `"fetch": true`, poi scarica da lì come `remote.fetch` (lento: avanzamento
/// e annullamento dal [`JobContext`]).
///
/// Controlli prima di scrivere: nome nuovo ([`check_new_remote_name`]: niente `-` iniziale) e
/// URL ([`check_remote_url`]: niente `-` iniziale, niente `ext::`/`fd::`, niente host che
/// inizia con `-`, niente caratteri di controllo) → [`EngineError::InvalidParams`]; un remoto
/// con quel nome già configurato, o branch remoti `refs/remotes/<name>/...` rimasti da prima
/// (l'undo li cancellerebbe) → [`EngineError::NotAllowed`]. Un URL con credenziali si accetta
/// (git lo salva in chiaro nella configurazione del repo: lo dice `warnings`).
///
/// Va nel diario: `edit.undo` toglie il remoto (configurazione e branch remoti scaricati) solo
/// se è ancora com'era subito dopo; `edit.redo` lo rimette. Un fetch fallito o annullato non
/// annulla l'aggiunta: il remoto resta (ed è nel diario) e l'errore è in `fetch_error`.
///
/// Risultato: `{"remote", "url" (mascherato), "credentials": bool, "fetched": bool, "fetch":
/// {"updated", "messages", "warnings"} | null, "fetch_error": {"kind": "auth" | "unreachable" |
/// "cancelled" | "other", "message"} | null, "warnings": [string], "remotes": [come
/// remote.list], "refs": [RefInfo], "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn remote_add(s: &mut Session, p: Value) -> Out {
    const ID: &str = "remote.add";
    let AddParams { name, url, fetch } = parse_params(ID, p)?;
    check_new_remote_name(ID, &name)?;
    check_remote_url(ID, &url)?;
    let job = s.job().clone();
    let (repo, journal) = s.repo_and_journal()?;
    if repo.remotes()?.contains(&name) {
        return Err(EngineError::NotAllowed(format!(
            "esiste già un remoto `{}`, niente modificato",
            name.escape_debug()
        )));
    }
    let before = repo.remote_snapshot(&name)?;
    if !before.config.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "la configurazione del repo ha già voci `remote.{}.*`: niente modificato (scegliere \
             un altro nome o toglierle)",
            name.escape_debug()
        )));
    }
    if !before.refs.is_empty() || before.head.is_some() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono già branch remoti `refs/remotes/{}/...` ({}) rimasti da un remoto tolto a \
             mano: niente modificato (scegliere un altro nome)",
            name.escape_debug(),
            before.refs.len()
        )));
    }
    let head = HeadState::from_head(&repo.head()?);
    repo.create_remote(&name, &url)?;
    let (shown, credentials) = mask_url(&url);
    let mut warnings = Vec::new();
    if credentials {
        warnings.push(format!(
            "l'URL contiene credenziali: git le ha salvate in chiaro nella configurazione del \
             repo (`remote.{name}.url`); meglio un credential helper"
        ));
    }
    let (fetch_out, fetch_error) = if fetch {
        match fetch_all(repo, &name, &job, ID) {
            Ok(out) => (out, Value::Null),
            Err(e) => (
                Value::Null,
                json!({ "kind": error_kind(&e), "message": e.to_string() }),
            ),
        }
    } else {
        (Value::Null, Value::Null)
    };
    let fetched = !fetch_out.is_null();
    let after = repo.remote_snapshot(&name)?;
    journal.record(JournalEntry::remote_add(after, head, fetched));
    let mut out = journal_state(journal);
    out["remote"] = Value::String(name);
    out["url"] = Value::String(shown);
    out["credentials"] = Value::Bool(credentials);
    out["fetched"] = Value::Bool(fetched);
    out["fetch"] = fetch_out;
    out["fetch_error"] = fetch_error;
    out["warnings"] = to_json(warnings);
    with_remotes(&mut out, repo);
    with_refs(&mut out, repo.refs());
    Ok(out)
}

/// `a`, `b` per i messaggi.
fn names(list: &[String]) -> String {
    list.iter()
        .map(|b| format!("`{}`", b.escape_debug()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Cosa toglierebbe la rimozione, per la conferma.
fn removal_summary(snapshot: &RemoteSnapshot) -> String {
    let mut parts = vec![format!(
        "la configurazione `remote.{}.*` e {} branch remoti (`refs/remotes/{}/...`)",
        snapshot.name,
        snapshot.refs.len(),
        snapshot.name
    )];
    let upstreams = snapshot.upstream_branches();
    if !upstreams.is_empty() {
        parts.push(format!(
            "l'upstream dei branch {} (`branch.<b>.remote`/`merge`)",
            names(&upstreams)
        ));
    }
    let push = snapshot.push_remote_branches();
    if !push.is_empty() {
        parts.push(format!("il `pushRemote` dei branch {}", names(&push)));
    }
    if snapshot.push_default {
        parts.push("`remote.pushDefault`".to_owned());
    }
    parts.join(", ")
}

/// Rimuove il remoto configurato `name`: la sezione `remote.<name>` del file del repo, i branch
/// remoti `refs/remotes/<name>/...` (e `refs/remotes/<name>/HEAD`), l'upstream dei branch
/// locali che lo usano (`branch.<b>.remote`/`merge`), i `branch.<b>.pushRemote` che lo nominano
/// e `remote.pushDefault` se vale `name` (come `git remote remove`). Sul remoto vero non cambia
/// niente; i tag scaricati restano.
///
/// Chiede sempre `"confirm": true`: senza, [`EngineError::ConfirmRequired`] che elenca i branch
/// che perderebbero l'upstream e il numero di branch remoti, e niente cambia. Rifiuti
/// ([`EngineError::NotAllowed`]): un remoto che non è configurato; uno definito, anche solo in
/// parte, fuori dal file del repo (configurazione globale o di sistema, `include`,
/// `config.worktree`, `.git/remotes/`: l'errore dice dove); il remoto promisor di un clone
/// parziale (`remote.<name>.promisor`, `extensions.partialClone`: senza, gli oggetti mancanti
/// non si troverebbero più); un'operazione in corso. Se dopo la rimozione git vede ancora il
/// remoto, tutto si rimette subito com'era e il risultato è [`EngineError::NotAllowed`].
///
/// Prima di scrivere, la voce del diario salva tutto ([`RemoteSnapshot`]: configurazione,
/// branch remoti con i commit, upstream) e i commit che resterebbero solo nei branch remoti
/// (non raggiungibili da branch locali, tag, altri ref o backup) vanno in ref di backup
/// `refs/project/backup/remote-...` (uno per commit di punta, in `backups`), così l'undo li
/// ritrova anche dopo un `gc`. I branch remoti si tolgono con una transazione (`update-ref
/// --stdin`, con i valori letti); poi la configurazione (un problema lì va in `warnings`). A
/// differenza di `git remote remove`, i ref fuori da `refs/remotes/<name>/` scritti da un
/// refspec personalizzato (es. `refs/pull/*`) restano.
/// `edit.undo` rimette branch remoti, configurazione e upstream, solo se nel frattempo nessuno
/// ha ricreato un remoto con quel nome (altrimenti [`EngineError::UndoConflict`]); un upstream
/// che nel frattempo è stato reimpostato altrove resta, con un avviso.
///
/// Risultato: `{"remote", "removed": RemoteSnapshot (URL mascherati), "upstreams_removed":
/// [branch], "push_remotes_removed": [branch], "push_default_removed": bool, "backups":
/// [string], "warnings": [string], "remotes": [come remote.list], "refs": [RefInfo],
/// "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn remote_remove(s: &mut Session, p: Value) -> Out {
    const ID: &str = "remote.remove";
    let RemoveParams { name, confirm } = parse_params(ID, p)?;
    check_remote_name(ID, &name)?;
    let (repo, journal) = s.repo_and_journal()?;
    let remotes = repo.remotes()?;
    if !remotes.contains(&name) {
        let known = if remotes.is_empty() {
            "il repository non ha remoti configurati".to_owned()
        } else {
            format!("remoti configurati: {}", names(&remotes))
        };
        return Err(EngineError::NotAllowed(format!(
            "non esiste un remoto `{}` ({known}); niente modificato",
            name.escape_debug()
        )));
    }
    let status = repo.status()?;
    check_no_operation(ID, "rimuoverebbe un remoto", status.operation)?;
    let snapshot = repo.remote_snapshot(&name)?;
    if snapshot.config.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "il remoto `{}` è configurato fuori dal file del repo (configurazione globale, \
             `include` o `.git/remotes/`): niente modificato, toglierlo da dove è definito",
            name.escape_debug()
        )));
    }
    let elsewhere = repo.remote_config_elsewhere(&name)?;
    if !elsewhere.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "il remoto `{}` è definito anche fuori dal file del repo: {}. Rimuovendolo da qui ne \
             resterebbe una parte (git lo vedrebbe ancora): niente modificato, togliere prima \
             quelle voci",
            name.escape_debug(),
            elsewhere.join("; ")
        )));
    }
    let promisor = truthy(
        repo.config_values(&format!("remote.{name}.promisor"))?
            .last(),
    );
    let partial = repo
        .config_values("extensions.partialClone")?
        .contains(&name);
    if promisor || partial {
        return Err(EngineError::NotAllowed(format!(
            "`{}` è il remoto promisor di un clone parziale (`remote.{}.promisor` o \
             `extensions.partialClone`): gli oggetti che mancano in locale si scaricano da lì, \
             e senza il remoto il repo non li troverebbe più (checkout, log e diff fallirebbero). \
             Niente modificato",
            name.escape_debug(),
            name
        )));
    }
    if !confirm {
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: format!(
                "rimuovere il remoto `{}` toglierebbe {}. Il remoto vero non cambia; i commit \
                 che restano solo nei branch remoti si salvano prima in \
                 `refs/project/backup/remote-...`, e `edit.undo` rimette tutto",
                name.escape_debug(),
                removal_summary(&snapshot)
            ),
            expected: None,
        });
    }
    let head = HeadState::from_head(&status.head);
    let reflog = format!("project: remote.remove {name}");
    let outcome = remove_remote(repo, &snapshot, ID, true, reflog)?;
    let entry = JournalEntry::remote_remove(snapshot.clone(), head, &outcome.backups);
    // Una parte definita altrove e sfuggita al controllo: si rimette subito tutto. Se non si
    // riesce, la rimozione va comunque nel diario (l'errore lo dice).
    if let Some(still) = ensure_remote_gone(repo, &snapshot, ID) {
        if !still.put_back {
            journal.record(entry);
        }
        return Err(still.error);
    }
    let upstreams = snapshot.upstream_branches();
    let push_remotes = snapshot.push_remote_branches();
    let push_default = snapshot.push_default;
    journal.record(entry);
    let mut out = journal_state(journal);
    out["remote"] = Value::String(name);
    out["removed"] = to_json(&snapshot);
    out["upstreams_removed"] = to_json(upstreams);
    out["push_remotes_removed"] = to_json(push_remotes);
    out["push_default_removed"] = Value::Bool(push_default);
    out["backups"] = to_json(outcome.backups);
    out["warnings"] = to_json(outcome.warnings);
    with_remotes(&mut out, repo);
    with_refs(&mut out, repo.refs());
    Ok(out)
}

#[cfg(test)]
#[path = "remotes_tests.rs"]
mod tests;
