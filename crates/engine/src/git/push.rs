//! Push (`remote.push`, S26): tipi comuni ai backend (`impl-docs/git/sync.md`).
//!
//! Il comando decide remoto, branch di destinazione e upstream; il backend pubblica **solo**
//! `refs/heads/<branch>:refs/heads/<remote_branch>` (refspec esplicito: niente tag, niente altri
//! branch, qualunque sia `push.default`, `remote.<nome>.push` o `push.followTags`). Il force è
//! sempre e solo `--force-with-lease=refs/heads/<remote_branch>:<oid atteso>`, con l'oid letto
//! dal branch remoto locale prima del push.

use project_domain::Oid;
use serde::{Deserialize, Serialize};

/// Cosa pubblicare con [`GitBackend::push`](super::GitBackend::push). Nomi già controllati dal
/// comando.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRequest {
    /// Il remoto configurato.
    pub remote: String,
    /// Il branch locale (nome breve).
    pub branch: String,
    /// Il branch sul remoto (nome breve).
    pub remote_branch: String,
    /// Imposta l'upstream del branch (`--set-upstream`) se il push riesce: solo al primo push,
    /// quando il branch non ne ha uno.
    pub set_upstream: bool,
    /// `Some(atteso)`: force con `--force-with-lease=refs/heads/<remote_branch>:<atteso>`, cioè
    /// il remoto si sovrascrive solo se il suo branch vale ancora `atteso` (`None`: solo se non
    /// esiste). `None`: push normale, solo fast-forward.
    pub lease: Option<Option<Oid>>,
    /// Il commit da salvare in un ref di backup prima di un force (`refs/project/backup/push-...`):
    /// il valore del branch remoto che il force sovrascriverebbe, se ha commit che il branch
    /// locale non ha.
    pub backup: Option<Oid>,
}

/// Come è stato aggiornato il branch sul remoto.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    /// Il branch non c'era: creato.
    New,
    FastForward,
    /// Sovrascritto (force-with-lease).
    Forced,
    /// Il remoto aveva già quel commit: niente pubblicato.
    UpToDate,
}

/// Esito di [`GitBackend::push`](super::GitBackend::push).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushOutcome {
    pub kind: PushKind,
    /// Il riassunto di git (`a1b2c3d..e4f5a6b`, `[new branch]`, ...).
    pub summary: String,
    /// Le righe di git che non sono avanzamento (`To <url>`, avvisi, output degli hook).
    pub messages: Vec<String>,
    /// Il ref di backup creato prima di un force (vedi [`PushRequest::backup`]).
    pub backup: Option<String>,
}

/// Perché il push è stato rifiutato ([`EngineError::PushRejected`](crate::EngineError)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushRejection {
    /// Il remoto ha commit che il branch locale non ha (`non-fast-forward`, `fetch first`).
    NonFastForward,
    /// Force-with-lease: il branch sul remoto non vale più quello atteso (`stale info`).
    StaleLease,
    /// Il remoto ha rifiutato l'aggiornamento (hook `pre-receive`/`update`, branch protetto).
    RemoteRejected,
    /// Un hook `pre-push` locale (o un altro controllo locale, come i sottomoduli non
    /// pubblicati) ha fermato il push prima che partisse.
    Hook,
    /// Altro rifiuto di git.
    Other,
}

impl PushRejection {
    /// Cosa fare, per il messaggio dell'errore.
    pub fn advice(&self) -> &'static str {
        match self {
            PushRejection::NonFastForward => {
                "il remoto ha commit che il branch locale non ha: fare `remote.pull` (o fetch e \
                 merge/rebase) e ripetere il push"
            }
            PushRejection::StaleLease => {
                "il branch sul remoto si è spostato rispetto al branch remoto locale \
                 (force-with-lease): fare `remote.fetch`, controllare i commit arrivati e \
                 decidere di nuovo"
            }
            PushRejection::RemoteRejected => "il remoto ha rifiutato l'aggiornamento",
            PushRejection::Hook => {
                "un hook `pre-push` (o un controllo locale di git) ha fermato il push prima che \
                 partisse"
            }
            PushRejection::Other => "git ha rifiutato il push",
        }
    }
}

/// Il nome del branch remoto locale che rispecchia `remote_ref` (es. `refs/heads/main`)
/// secondo i refspec di fetch del remoto (`remote.<nome>.fetch`, es.
/// `+refs/heads/*:refs/remotes/origin/*`), come fa git per aggiornarlo dopo un push: il primo
/// refspec che corrisponde (con al più un `*`), `None` se nessuno, se il refspec non ha
/// destinazione o se un refspec negativo (`^...`) lo esclude.
pub fn tracking_ref(refspecs: &[String], remote_ref: &str) -> Option<String> {
    fn matches<'a>(pattern: &str, name: &'a str) -> Option<&'a str> {
        match pattern.split_once('*') {
            Some((prefix, suffix)) => name
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .filter(|middle| !middle.is_empty()),
            None => (pattern == name).then_some(""),
        }
    }
    let excluded = refspecs
        .iter()
        .filter_map(|r| r.strip_prefix('^'))
        .any(|pattern| matches(pattern, remote_ref).is_some());
    if excluded {
        return None;
    }
    refspecs
        .iter()
        .filter(|r| !r.starts_with('^'))
        .find_map(|spec| {
            let spec = spec.strip_prefix('+').unwrap_or(spec);
            let (src, dst) = spec.split_once(':')?;
            if dst.is_empty() {
                return None;
            }
            let middle = matches(src, remote_ref)?;
            Some(if src.contains('*') {
                dst.replacen('*', middle, 1)
            } else {
                dst.to_owned()
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_ref_from_refspecs() {
        let specs = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let default = specs(&["+refs/heads/*:refs/remotes/origin/*"]);
        assert_eq!(
            tracking_ref(&default, "refs/heads/feature/x").as_deref(),
            Some("refs/remotes/origin/feature/x")
        );
        assert_eq!(tracking_ref(&default, "refs/tags/v1"), None);
        assert_eq!(tracking_ref(&[], "refs/heads/main"), None);
        let custom = specs(&[
            "^refs/heads/tmp/*",
            "refs/heads/main:refs/remotes/up/principale",
            "+refs/heads/*:refs/remotes/up/b/*",
        ]);
        assert_eq!(
            tracking_ref(&custom, "refs/heads/main").as_deref(),
            Some("refs/remotes/up/principale")
        );
        assert_eq!(
            tracking_ref(&custom, "refs/heads/x").as_deref(),
            Some("refs/remotes/up/b/x")
        );
        assert_eq!(tracking_ref(&custom, "refs/heads/tmp/a"), None);
        assert_eq!(
            tracking_ref(&specs(&["refs/heads/main"]), "refs/heads/main"),
            None
        );
    }
}
