//! Gestione dei remoti (`remote.list`, `remote.add`, `remote.remove`, S27): tipi e controlli
//! comuni ai backend, e l'eliminazione/ripristino di un remoto usati dai comandi e dal diario
//! (`impl-docs/git/remoti.md`).
//!
//! Un remoto è fatto di tre parti, che [`RemoteSnapshot`] fotografa:
//!
//! - la sua configurazione nel file del repo (`remote.<nome>.*`: URL, refspec, ...);
//! - i suoi branch remoti (`refs/remotes/<nome>/...`, esclusi quelli di un altro remoto che si
//!   chiama `<nome>/qualcosa`) e il ref simbolico `refs/remotes/<nome>/HEAD`;
//! - le voci di altri che lo nominano ("dipendenti"): l'upstream dei branch locali
//!   (`branch.<b>.remote` e `branch.<b>.merge`), `branch.<b>.pushRemote` e
//!   `remote.pushDefault`, sempre del file del repo.
//!
//! Eliminare un remoto ([`remove_remote`]) toglie i branch remoti con una sola transazione
//! (`GitBackend::restore_refs`, con i valori attesi e i backup dei commit che non resterebbero
//! raggiungibili), poi la configurazione; ripristinarlo ([`restore_remote`]) fa il contrario,
//! solo se nel frattempo nessuno ha ricreato un remoto con quel nome.

use super::{ConfigEntry, GitBackend, HeadState, RefMove, RefRestore, RestoreOutcome};
use super::{check_no_operation, check_remote_name};
use crate::EngineError;
use project_domain::Oid;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeSet;

/// Prefisso dei branch remoti.
pub const REMOTES_PREFIX: &str = "refs/remotes/";

/// Un branch remoto di un remoto: nome completo e commit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RemoteRef {
    pub name: String,
    pub target: Oid,
}

/// Quello che `remote.remove` toglie (e il diario rimette): vedi il modulo.
///
/// JSON: `{"name", "config": [{"key", "value"}], "refs": [{"name", "target"}], "head": string |
/// null, "upstreams": [{"key", "value"}], "push_default": bool}`. Nel JSON i valori della
/// configurazione sono mascherati come in `remote.list` ([`mask_url`]): il diario in memoria
/// tiene i valori veri, che servono all'undo, ma `edit.history` non mostra password e token.
/// Per questo non si deserializza: un `RemoteSnapshot` letto dal JSON avrebbe gli URL
/// mascherati e non servirebbe a rimettere il remoto.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RemoteSnapshot {
    /// Il nome del remoto.
    pub name: String,
    /// `remote.<nome>.*` del file del repo, nell'ordine del file.
    #[serde(serialize_with = "masked_entries")]
    pub config: Vec<ConfigEntry>,
    /// I branch remoti non simbolici, in ordine di nome.
    pub refs: Vec<RemoteRef>,
    /// Dove punta `refs/remotes/<nome>/HEAD` (nome completo), se è un ref simbolico.
    pub head: Option<String>,
    /// `branch.<b>.remote` e `branch.<b>.merge` dei branch che hanno il remoto come upstream,
    /// e `branch.<b>.pushRemote` che lo nominano, nell'ordine del file.
    pub upstreams: Vec<ConfigEntry>,
    /// `remote.pushDefault` (file del repo) vale questo remoto.
    pub push_default: bool,
}

fn masked_entries<S: Serializer>(entries: &[ConfigEntry], s: S) -> Result<S::Ok, S::Error> {
    let masked: Vec<ConfigEntry> = entries.iter().map(masked_entry).collect();
    masked.serialize(s)
}

/// La voce con le credenziali mascherate ([`mask_url`]) nel valore: non solo `*.url` e
/// `*.pushurl`, qualunque valore che sia un URL con credenziali (es. `remote.x.proxy`).
pub fn masked_entry(entry: &ConfigEntry) -> ConfigEntry {
    ConfigEntry {
        key: entry.key.clone(),
        value: entry.value.as_deref().map(|v| mask_url(v).0),
    }
}

impl RemoteSnapshot {
    /// Vero se del remoto non c'è niente: né configurazione né branch remoti.
    pub fn is_absent(&self) -> bool {
        self.config.is_empty() && self.refs.is_empty() && self.head.is_none()
    }

    /// Le differenze fra `self` (lo stato atteso) e `now`, una per riga, per
    /// [`EngineError::UndoConflict`]; vuoto se sono uguali. Configurazione e dipendenti si
    /// confrontano come insiemi (l'ordine del file può cambiare riscrivendoli).
    pub fn differences(&self, now: &RemoteSnapshot) -> Vec<String> {
        let mut out = Vec::new();
        let sorted = |entries: &[ConfigEntry]| {
            let mut v: Vec<(String, String)> = entries
                .iter()
                // Una chiave senza valore si riscrive come `true`: è la stessa cosa per git.
                .map(|e| {
                    let value = e.value.clone().unwrap_or_else(|| "true".to_owned());
                    (e.key.to_ascii_lowercase(), value)
                })
                .collect();
            v.sort();
            v
        };
        if sorted(&self.config) != sorted(&now.config) {
            out.push(format!(
                "la configurazione `remote.{}.*` è cambiata",
                self.name
            ));
        }
        let (old, new): (BTreeSet<&RemoteRef>, BTreeSet<&RemoteRef>) =
            (self.refs.iter().collect(), now.refs.iter().collect());
        for r in old.difference(&new) {
            let current = now.refs.iter().find(|n| n.name == r.name);
            out.push(match current {
                Some(c) => format!(
                    "`{}`: doveva essere {}, ora è {}",
                    r.name, r.target, c.target
                ),
                None => format!("`{}`: doveva essere {}, ora non c'è", r.name, r.target),
            });
        }
        for r in new.difference(&old) {
            if !self.refs.iter().any(|o| o.name == r.name) {
                out.push(format!(
                    "`{}`: non doveva esserci, ora è {}",
                    r.name, r.target
                ));
            }
        }
        if self.head != now.head {
            out.push(format!(
                "`{REMOTES_PREFIX}{}/HEAD`: doveva puntare a {}, ora a {}",
                self.name,
                self.head.as_deref().unwrap_or("niente"),
                now.head.as_deref().unwrap_or("niente")
            ));
        }
        if sorted(&self.upstreams) != sorted(&now.upstreams) {
            out.push(format!(
                "l'upstream o il pushRemote dei branch che usano `{}` è cambiato",
                self.name
            ));
        }
        if self.push_default != now.push_default {
            out.push("`remote.pushDefault` è cambiato".to_owned());
        }
        out
    }

    /// I branch locali (nomi brevi) che hanno il remoto come upstream (`branch.<b>.remote`), in
    /// ordine di nome.
    pub fn upstream_branches(&self) -> Vec<String> {
        self.dependent_branches("remote")
    }

    /// I branch locali (nomi brevi) con `branch.<b>.pushRemote` uguale al remoto, in ordine di
    /// nome.
    pub fn push_remote_branches(&self) -> Vec<String> {
        self.dependent_branches("pushremote")
    }

    fn dependent_branches(&self, var: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .upstreams
            .iter()
            .filter_map(|e| split_branch_key(&e.key))
            .filter(|(_, v)| v == var)
            .map(|(b, _)| b.to_owned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// L'URL (`remote.<nome>.url`) com'è scritto, se c'è.
    pub fn url(&self) -> Option<&str> {
        self.config
            .iter()
            .find(|e| e.key.to_ascii_lowercase().ends_with(".url"))
            .and_then(|e| e.value.as_deref())
    }

    /// Lo stesso remoto senza i dipendenti (per l'undo e il redo di `remote.add`, che non li
    /// toccano).
    fn without_dependents(&self) -> RemoteSnapshot {
        RemoteSnapshot {
            upstreams: Vec::new(),
            push_default: false,
            ..self.clone()
        }
    }
}

/// `branch.<b>.<var>` → (`b`, `var` in minuscolo); `None` per un'altra chiave.
pub fn split_branch_key(key: &str) -> Option<(&str, String)> {
    let rest = key.strip_prefix("branch.")?;
    let (branch, var) = rest.rsplit_once('.')?;
    (!branch.is_empty()).then(|| (branch, var.to_ascii_lowercase()))
}

/// Fra le voci `branch.<b>.(remote|merge|pushRemote)` della configurazione (`entries`), quelle
/// che dipendono dal remoto `name`: `remote` e `merge` dei branch con `branch.<b>.remote =
/// <name>`, e i `pushRemote` uguali a `name`. Nell'ordine di `entries`.
pub fn remote_dependents(name: &str, entries: &[ConfigEntry]) -> Vec<ConfigEntry> {
    let tracking: BTreeSet<&str> = entries
        .iter()
        .filter_map(|e| {
            let (branch, var) = split_branch_key(&e.key)?;
            (var == "remote" && e.value.as_deref() == Some(name)).then_some(branch)
        })
        .collect();
    entries
        .iter()
        .filter(|e| match split_branch_key(&e.key) {
            Some((branch, var)) if var == "remote" || var == "merge" => tracking.contains(branch),
            Some((_, var)) if var == "pushremote" => e.value.as_deref() == Some(name),
            _ => false,
        })
        .cloned()
        .collect()
}

/// Vero se il ref `refname` è un branch remoto del remoto `remote`: sta sotto
/// `refs/remotes/<remote>/` e non sotto `refs/remotes/<altro>/` di un altro remoto configurato
/// che si chiama `<remote>/...` (i nomi dei remoti possono contenere `/`).
pub fn owned_by(remote: &str, remotes: &[String], refname: &str) -> bool {
    let prefix = format!("{REMOTES_PREFIX}{remote}/");
    if !refname.starts_with(&prefix) {
        return false;
    }
    !remotes.iter().any(|other| {
        other.len() > remote.len()
            && other.starts_with(&format!("{remote}/"))
            && refname.starts_with(&format!("{REMOTES_PREFIX}{other}/"))
    })
}

/// Il nome di un remoto **nuovo** (`remote.add`): come [`check_remote_name`] e in più non
/// inizia con `-` (git lo accetterebbe dopo `--end-of-options`, ma poi ogni comando scritto a
/// mano lo prenderebbe per un'opzione). Altrimenti [`EngineError::InvalidParams`].
pub fn check_new_remote_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    check_remote_name(id, name)?;
    if name.starts_with('-') {
        return Err(EngineError::InvalidParams {
            id,
            reason: format!(
                "un nome di remoto non può iniziare con `-`: `{}`",
                name.escape_debug()
            ),
        });
    }
    Ok(())
}

/// L'URL di un remoto nuovo (`remote.add`). Politica (prima di passarlo a git, dopo
/// `--end-of-options`):
///
/// - non vuoto, senza spazi in testa o in coda e senza caratteri di controllo (a capo, NUL);
/// - non inizia con `-` (un `--upload-pack=...` o `-oProxyCommand=...` non deve mai poter
///   diventare un'opzione di git o di ssh);
/// - niente trasporti che eseguono comandi o usano descrittori: `ext::`/`ext://` e
///   `fd::`/`fd://`, in qualsiasi maiuscolo (git li permette se `protocol.ext.allow` lo dice;
///   da qui mai). Gli altri helper (`<trasporto>::<indirizzo>`) restano permessi;
/// - un host o un utente che inizia con `-` (`ssh://-oProxyCommand=x/r`, `ssh://-oX@h/r`,
///   `utente@-x:r`, `ssh://[-x]/r`) è rifiutato.
///
/// Altrimenti [`EngineError::InvalidParams`]. Le credenziali nell'URL sono accettate (le
/// salva git nella configurazione, in chiaro): il comando lo dice in un avviso.
pub fn check_remote_url(id: &'static str, url: &str) -> Result<(), EngineError> {
    let reason = if url.trim().is_empty() {
        Some("l'URL del remoto è vuoto".to_owned())
    } else if url.chars().any(char::is_control) {
        Some("l'URL contiene caratteri di controllo".to_owned())
    } else if url.trim() != url {
        Some("l'URL inizia o finisce con spazi".to_owned())
    } else if url.starts_with('-') {
        Some("un URL non può iniziare con `-` (sembrerebbe un'opzione)".to_owned())
    } else if let Some(transport) = forbidden_transport(url) {
        Some(format!(
            "il trasporto `{transport}` esegue comandi o usa descrittori aperti: non permesso"
        ))
    } else if dash_in_authority(url) {
        Some("l'utente o l'host dell'URL non può iniziare con `-`".to_owned())
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: format!("{reason}: `{}`", mask_url(url).0.escape_debug()),
        }),
        None => Ok(()),
    }
}

/// `ext` o `fd` se l'URL usa uno di quei trasporti (`ext::...`, `ext://...`), anche dentro un
/// altro helper.
fn forbidden_transport(url: &str) -> Option<&'static str> {
    let lower = url.to_ascii_lowercase();
    for transport in ["ext", "fd"] {
        if lower.starts_with(&format!("{transport}::"))
            || lower.starts_with(&format!("{transport}://"))
        {
            return Some(transport);
        }
    }
    // `<helper>::<indirizzo>`: si controlla anche l'indirizzo.
    let (scheme, rest) = url.split_once("::")?;
    scheme_like(scheme)
        .then(|| forbidden_transport(rest))
        .flatten()
}

/// Vero se `s` può essere uno schema di URL (`[A-Za-z][A-Za-z0-9+.-]*`).
fn scheme_like(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
}

/// Utente e host di un URL `schema://[utente@]host...` o di un indirizzo alla scp
/// `[utente@]host:...` (senza `/` prima dei `:`), con le `[]` di un host IPv6 tolte; `None` per
/// un percorso.
fn split_user(authority: &str) -> (Option<&str>, &str) {
    match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    }
}

fn authority_of(url: &str) -> Option<(Option<&str>, &str)> {
    let url = match url.split_once("::") {
        Some((scheme, rest)) if scheme_like(scheme) => rest,
        _ => url,
    };
    let (user, host) = if let Some((scheme, rest)) = url.split_once("://")
        && scheme_like(scheme)
    {
        split_user(rest.split(['/', '?', '#']).next().unwrap_or_default())
    } else {
        let colon = url.find(':')?;
        if url[..colon].contains('/') {
            return None;
        }
        split_user(&url[..colon])
    };
    Some((user, host.trim_start_matches('[')))
}

/// Vero se l'utente o l'host dell'URL inizia con `-` (diventerebbe un'opzione di ssh).
fn dash_in_authority(url: &str) -> bool {
    authority_of(url).is_some_and(|(user, host)| {
        host.starts_with('-') || user.is_some_and(|u| u.starts_with('-'))
    })
}

/// L'URL senza credenziali, da mostrare, e se ne aveva. Politica: in un URL
/// `schema://utente:password@host/...` la password diventa `***` (`https://nicola:***@host/r`);
/// in un URL `http(s)://` o `ftp(s)://` con solo l'utente anche l'utente diventa `***`
/// (`https://***@github.com/r`: lì l'"utente" è spesso un token). Un utente senza password in
/// `ssh://`, `git://` o alla scp (`git@host:r`) resta: non è un segreto. Vale anche dentro un
/// helper (`<trasporto>::<url>`). Tutto il resto resta com'è.
pub fn mask_url(url: &str) -> (String, bool) {
    if let Some((scheme, rest)) = url.split_once("::")
        && scheme_like(scheme)
    {
        let (inner, masked) = mask_url(rest);
        return (format!("{scheme}::{inner}"), masked);
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_owned(), false);
    };
    if !scheme_like(scheme) {
        return (url.to_owned(), false);
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return (url.to_owned(), false);
    };
    let web = matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "ftps"
    );
    let shown = match userinfo.split_once(':') {
        Some((user, _)) => format!("{user}:***"),
        None if web => "***".to_owned(),
        None => return (url.to_owned(), false),
    };
    (format!("{scheme}://{shown}@{host}{tail}"), true)
}

/// Toglie il remoto fotografato in `snapshot` (`remote.remove`, undo di `remote.add`, redo di
/// `remote.remove`), solo se adesso è esattamente così ([`RemoteSnapshot::differences`]:
/// altrimenti [`EngineError::UndoConflict`] per `edit.*`, [`EngineError::NotAllowed`] per gli
/// altri comandi, e niente cambia). Con `dependents` toglie anche upstream, `pushRemote` e
/// `remote.pushDefault` salvati (per `remote.add` no: non li ha creati lui).
///
/// I branch remoti si tolgono con [`GitBackend::restore_refs`] (una transazione con i valori
/// attesi; i commit che non resterebbero raggiungibili da altri ref vanno prima in
/// `refs/project/backup/<remote|undo|redo>-...`, nel risultato), poi la configurazione: un
/// problema lì è un avviso, perché i ref sono già tolti.
pub fn remove_remote(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    command: &'static str,
    dependents: bool,
    reflog: String,
) -> Result<RestoreOutcome, EngineError> {
    let status = repo.status()?;
    check_no_operation(command, "rimuoverebbe un remoto", status.operation)?;
    let now = repo.remote_snapshot(&snapshot.name)?;
    let differences = snapshot.differences(&now);
    if !differences.is_empty() {
        return Err(changed(command, &snapshot.name, differences));
    }
    let mut outcome = RestoreOutcome::default();
    if !snapshot.refs.is_empty() {
        let head = HeadState::from_head(&status.head);
        outcome = repo.restore_refs(&RefRestore {
            command,
            head_expected: head.clone(),
            head_target: head,
            refs: snapshot
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: Some(r.target),
                    target: None,
                })
                .collect(),
            reflog,
        })?;
    }
    let target = if dependents {
        snapshot.clone()
    } else {
        snapshot.without_dependents()
    };
    outcome.warnings.extend(repo.remove_remote_config(&target));
    Ok(outcome)
}

/// Rimette il remoto fotografato in `snapshot` (undo di `remote.remove`, redo di
/// `remote.add`), solo se adesso del remoto non c'è niente (né fra i remoti configurati né
/// configurazione né branch remoti: altrimenti [`EngineError::UndoConflict`] e niente cambia).
/// Prima i branch remoti (una transazione che li crea solo se non esistono), poi la
/// configurazione. Con `dependents` anche upstream, `pushRemote` e `remote.pushDefault`, ma
/// solo dove il repo non ne ha uno nuovo e il branch esiste ancora: gli altri restano come sono,
/// con un avviso.
pub fn restore_remote(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    command: &'static str,
    dependents: bool,
    reflog: String,
) -> Result<(RestoreOutcome, RemoteSnapshot), EngineError> {
    let status = repo.status()?;
    check_no_operation(command, "rimetterebbe un remoto", status.operation)?;
    let now = repo.remote_snapshot(&snapshot.name)?;
    let configured = repo.remotes()?.contains(&snapshot.name);
    if configured || !now.is_absent() {
        let mut refs = vec![format!(
            "il remoto `{}` esiste di nuovo (configurazione o branch remoti)",
            snapshot.name
        )];
        refs.extend(now.refs.iter().map(|r| format!("`{}` c'è già", r.name)));
        return Err(changed(command, &snapshot.name, refs));
    }
    let (outcome, written, _) = put_back(
        repo,
        snapshot,
        &status.head,
        command,
        dependents,
        reflog,
        true,
    )?;
    Ok((outcome, written))
}

/// Rimette branch remoti e configurazione di `snapshot` senza controllare prima che il remoto
/// non ci sia (i ref si creano comunque solo se non esistono). Restituisce l'esito e quello
/// che è stato davvero riscritto (i dipendenti saltati non ci sono). Con `strict` un errore sui
/// ref ferma tutto prima di scrivere (undo e redo: niente cambia); senza (il ripristino
/// d'emergenza di [`ensure_remote_gone`]) diventa un avviso e la configurazione si scrive
/// comunque.
fn put_back(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    head: &project_domain::Head,
    command: &'static str,
    dependents: bool,
    reflog: String,
    strict: bool,
) -> Result<(RestoreOutcome, RemoteSnapshot, bool), EngineError> {
    let mut outcome = RestoreOutcome::default();
    let mut complete = true;
    if !snapshot.refs.is_empty() {
        let head = HeadState::from_head(head);
        let restored = repo.restore_refs(&RefRestore {
            command,
            head_expected: head.clone(),
            head_target: head,
            refs: snapshot
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: None,
                    target: Some(r.target),
                })
                .collect(),
            reflog,
        });
        match restored {
            Ok(o) => outcome = o,
            Err(e) if strict => return Err(e),
            Err(e) => {
                complete = false;
                outcome.warnings.push(format!(
                    "i branch remoti di `{}` non sono stati rimessi ({e}): i commit sono nei \
                 backup e nella voce del diario",
                    snapshot.name
                ));
            }
        }
    }
    let mut target = snapshot.without_dependents();
    if dependents {
        match free_dependents(repo, snapshot) {
            Ok((upstreams, push_default, warnings)) => {
                target.upstreams = upstreams;
                target.push_default = push_default;
                outcome.warnings.extend(warnings);
            }
            Err(e) if strict => return Err(e),
            Err(e) => {
                complete = false;
                outcome
                    .warnings
                    .push(format!("upstream e pushRemote non rimessi: {e}"));
            }
        }
    }
    outcome.warnings.extend(repo.write_remote_config(&target));
    Ok((outcome, target, complete))
}

/// Esito di [`ensure_remote_gone`] quando git vede ancora il remoto dopo la rimozione.
#[derive(Debug)]
pub struct RemoteStillListed {
    /// L'errore da restituire ([`EngineError::NotAllowed`]).
    pub error: EngineError,
    /// Vero se il remoto è stato rimesso tutto (configurazione, branch remoti, upstream): la
    /// rimozione non va nel diario. Falso se il ripristino non è riuscito o non si sa se
    /// git vede il remoto: la rimozione va nel diario, così `edit.undo` e i backup restano.
    pub put_back: bool,
}

/// Dopo `remote.remove`: se git vede ancora il remoto (definito anche fuori dal file del repo,
/// scoperto troppo tardi) lo si rimette subito com'era; `None` se è sparito davvero.
pub fn ensure_remote_gone(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    command: &'static str,
) -> Option<RemoteStillListed> {
    let name = snapshot.name.escape_debug().to_string();
    match repo.remotes() {
        Ok(remotes) if !remotes.contains(&snapshot.name) => return None,
        Ok(_) => {}
        Err(e) => {
            return Some(RemoteStillListed {
                error: EngineError::NotAllowed(format!(
                    "il remoto `{name}` è stato rimosso, ma l'elenco dei remoti non si rilegge \
                     ({e}): la rimozione è nel diario (`edit.undo` la annulla) e i commit dei \
                     branch remoti nei backup"
                )),
                put_back: false,
            });
        }
    }
    let reflog = format!(
        "project: {command} annullato, ripristino di {}",
        snapshot.name
    );
    let restored = repo
        .head()
        .and_then(|head| put_back(repo, snapshot, &head, command, true, reflog, false));
    let note = "la rimozione è nel diario (`edit.undo` la annulla) e i commit dei branch \
                remoti nei backup";
    let (how, put_back) = match restored {
        Ok((_, _, true)) => ("rimesso com'era".to_owned(), true),
        Ok((outcome, _, false)) => (
            format!(
                "rimesso solo in parte ({}): {note}",
                outcome.warnings.join("; ")
            ),
            false,
        ),
        Err(e) => (format!("NON rimesso ({e}): {note}"), false),
    };
    Some(RemoteStillListed {
        error: EngineError::NotAllowed(format!(
            "dopo la rimozione git vede ancora il remoto `{name}` (è definito anche fuori dal \
             file del repo): {how}. Toglierlo da dove è definito"
        )),
        put_back,
    })
}

/// I dipendenti di `snapshot` che si possono riscrivere: quelli dei branch che esistono ancora
/// e non hanno nel frattempo un altro upstream / pushRemote, e `remote.pushDefault` se ora non
/// c'è. Gli altri, un avviso ciascuno.
fn free_dependents(
    repo: &dyn GitBackend,
    snapshot: &RemoteSnapshot,
) -> Result<(Vec<ConfigEntry>, bool, Vec<String>), EngineError> {
    let refs = repo.refs()?;
    let mut warnings = Vec::new();
    let mut skipped: BTreeSet<(String, bool)> = BTreeSet::new();
    let mut upstreams = Vec::new();
    for entry in &snapshot.upstreams {
        let Some((branch, var)) = split_branch_key(&entry.key) else {
            continue;
        };
        let push = var == "pushremote";
        if skipped.contains(&(branch.to_owned(), push)) {
            continue;
        }
        let exists = refs.iter().any(|r| r.name == super::branch_ref(branch));
        let taken = if push {
            !repo
                .config_values(&format!("branch.{branch}.pushRemote"))?
                .is_empty()
        } else {
            repo.upstream(branch)?.is_some()
        };
        if !exists || taken {
            let what = if push { "il pushRemote" } else { "l'upstream" };
            warnings.push(if exists {
                format!(
                    "il branch `{branch}` ha già {what} nuovo: {what} verso `{}` non è stato \
                     rimesso",
                    snapshot.name
                )
            } else {
                format!(
                    "il branch `{branch}` non c'è più: {what} verso `{}` non è stato rimesso",
                    snapshot.name
                )
            });
            skipped.insert((branch.to_owned(), push));
            continue;
        }
        upstreams.push(entry.clone());
    }
    let mut push_default = snapshot.push_default;
    if push_default && !repo.config_values("remote.pushDefault")?.is_empty() {
        warnings.push(format!(
            "`remote.pushDefault` ha già un altro valore: non è stato rimesso a `{}`",
            snapshot.name
        ));
        push_default = false;
    }
    Ok((upstreams, push_default, warnings))
}

fn changed(command: &'static str, name: &str, refs: Vec<String>) -> EngineError {
    if command.starts_with("edit.") {
        EngineError::UndoConflict { command, refs }
    } else {
        EngineError::NotAllowed(format!(
            "il remoto `{}` è cambiato mentre `{command}` lo leggeva ({}): niente modificato, \
             riprovare",
            name.escape_debug(),
            refs.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, value: &str) -> ConfigEntry {
        ConfigEntry {
            key: key.to_owned(),
            value: Some(value.to_owned()),
        }
    }

    #[test]
    fn masking() {
        let cases = [
            (
                "https://nicola:tok@host/r.git",
                "https://nicola:***@host/r.git",
                true,
            ),
            (
                "https://ghp_x@github.com/r",
                "https://***@github.com/r",
                true,
            ),
            ("http://u:p@h:8080/x?y@z", "http://u:***@h:8080/x?y@z", true),
            ("ssh://git@host/r", "ssh://git@host/r", false),
            ("ssh://git:pw@host/r", "ssh://git:***@host/r", true),
            ("git@github.com:a/b.git", "git@github.com:a/b.git", false),
            ("/srv/r.git", "/srv/r.git", false),
            ("https://host/path@x", "https://host/path@x", false),
            (
                "persistent-https::https://u:p@h/r",
                "persistent-https::https://u:***@h/r",
                true,
            ),
            ("https://a:b:c@h@x/r", "https://a:***@x/r", true),
            ("http://u:pw@proxy:3128", "http://u:***@proxy:3128", true),
        ];
        for (url, shown, masked) in cases {
            assert_eq!(mask_url(url), (shown.to_owned(), masked), "{url}");
        }
    }

    #[test]
    fn urls() {
        for ok in [
            "https://example.com/r.git",
            "git@github.com:a/b.git",
            "../r.git",
            "/srv/con spazio/r.git",
            "file:///srv/r",
            "ssh://git@host:22/r",
            "persistent-https::https://h/r",
        ] {
            check_remote_url("remote.add", ok).unwrap();
        }
        for bad in [
            "",
            "  ",
            " https://h/r",
            "-x",
            "--upload-pack=touch /tmp/x",
            "ext::sh -c touch% /tmp/x",
            "EXT::sh",
            "ext://sh",
            "fd::3",
            "x::ext::sh",
            "ssh://-oProxyCommand=x/r",
            "git@-oProxyCommand=x:r",
            "ssh://-oProxyCommand=x@h/r",
            "ssh://[-oProxyCommand=x]/r",
            "https://h/r\n",
            "a\0b",
        ] {
            let err = check_remote_url("remote.add", bad).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{bad:?}");
        }
        // L'errore non mostra la password.
        let err = check_remote_url("remote.add", "ext::https://u:segreto@h").unwrap_err();
        assert!(!err.to_string().contains("segreto"), "{err}");
    }

    #[test]
    fn new_names() {
        check_new_remote_name("remote.add", "fork/a").unwrap();
        for bad in ["-x", "--upload-pack=x", "", "a b", "a..b"] {
            assert!(check_new_remote_name("remote.add", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dependents_and_ownership() {
        let entries = [
            entry("branch.main.remote", "origin"),
            entry("branch.main.merge", "refs/heads/main"),
            entry("branch.a.b.remote", "fork"),
            entry("branch.a.b.merge", "refs/heads/x"),
            entry("branch.a.b.pushremote", "origin"),
            entry("branch.c.pushremote", "fork"),
        ];
        let deps = remote_dependents("origin", &entries);
        assert_eq!(
            deps,
            [entries[0].clone(), entries[1].clone(), entries[4].clone()]
        );
        let snapshot = RemoteSnapshot {
            name: "origin".into(),
            upstreams: deps,
            ..RemoteSnapshot::default()
        };
        assert_eq!(snapshot.upstream_branches(), ["main"]);
        assert_eq!(snapshot.push_remote_branches(), ["a.b"]);

        let remotes = vec!["fork".to_owned(), "fork/a".to_owned()];
        assert!(owned_by("fork", &remotes, "refs/remotes/fork/main"));
        assert!(!owned_by("fork", &remotes, "refs/remotes/fork/a/main"));
        assert!(owned_by("fork/a", &remotes, "refs/remotes/fork/a/main"));
        assert!(!owned_by("fork", &remotes, "refs/remotes/forked/main"));
    }

    #[test]
    fn differences_and_masked_json() {
        let a = RemoteSnapshot {
            name: "o".into(),
            config: vec![entry("remote.o.url", "https://u:p@h/r")],
            refs: vec![RemoteRef {
                name: "refs/remotes/o/main".into(),
                target: Oid::from_bytes([1; Oid::LEN]),
            }],
            ..RemoteSnapshot::default()
        };
        assert!(a.differences(&a).is_empty());
        let mut b = a.clone();
        b.refs[0].target = Oid::from_bytes([2; Oid::LEN]);
        b.push_default = true;
        assert_eq!(a.differences(&b).len(), 2, "{:?}", a.differences(&b));
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["config"][0]["value"], "https://u:***@h/r");
        assert_eq!(a.url(), Some("https://u:p@h/r"));
    }
}
