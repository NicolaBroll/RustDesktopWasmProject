//! Remoti (`remote.fetch`, S25): tipi e controlli comuni ai backend
//! (`impl-docs/git/sync.md`).
//!
//! Un remoto si indica sempre con il **nome** di un remoto configurato (`origin`), mai con un
//! URL o un percorso: `git fetch <x>` con un `x` che non è un remoto lo userebbe come URL, quindi
//! il comando controlla che il nome sia fra quelli di [`GitBackend::remotes`](super::GitBackend)
//! prima di chiamare il backend.

use super::valid_ref_name;
use crate::EngineError;
use project_domain::Status;
use serde::{Deserialize, Serialize};

/// Cosa scaricare con [`GitBackend::fetch`](super::GitBackend::fetch). Nomi già controllati dal
/// comando: `remote` è un remoto configurato, `branch` un nome breve di branch valido.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchRequest {
    /// Il nome del remoto configurato.
    pub remote: String,
    /// Solo questo branch del remoto (nome breve, es. `main`); `None`: quello che dice la
    /// configurazione del remoto (`remote.<nome>.fetch`, di solito tutti i branch).
    pub branch: Option<String>,
}

/// Il risultato di [`GitBackend::fetch`](super::GitBackend::fetch). I ref aggiornati li calcola
/// il comando confrontando i ref prima e dopo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FetchOutcome {
    /// Le righe di git che non sono avanzamento (`From <url>`, ` * [new branch] main ->
    /// origin/main`, avvisi), in ordine.
    pub messages: Vec<String>,
}

/// Il nome di un remoto passato a `remote.*`: non vuoto, senza caratteri di controllo, e tale
/// che `refs/remotes/<nome>/x` sia un nome di ref valido (lo stesso controllo di git per i nomi
/// dei remoti). Un nome che inizia con `-` è accettato (git lo permette): ai backend arriva
/// sempre dopo `--end-of-options`. Altrimenti [`EngineError::InvalidParams`].
pub fn check_remote_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    let reason = if name.is_empty() {
        Some("il nome del remoto è vuoto")
    } else if name.chars().any(char::is_control) {
        Some("il nome del remoto contiene caratteri di controllo")
    } else if !valid_ref_name(&format!("refs/remotes/{name}/x")) {
        Some("non è un nome di remoto valido")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: format!("{reason}: `{}`", name.escape_debug()),
        }),
        None => Ok(()),
    }
}

/// Il remoto da usare quando `remote` manca, come `git fetch` senza argomenti: quello
/// dell'upstream del branch attivo (`origin/main` → `origin`; con più remoti che potrebbero
/// corrispondere, il nome più lungo), altrimenti `origin`, altrimenti l'unico remoto
/// configurato. Nessuno di questi: [`EngineError::NotAllowed`] che chiede di indicarlo.
pub fn default_remote(
    id: &'static str,
    status: &Status,
    remotes: &[String],
) -> Result<String, EngineError> {
    let from_upstream = status.upstream.as_ref().and_then(|upstream| {
        remotes
            .iter()
            .filter(|r| {
                upstream
                    .name
                    .strip_prefix(r.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|r| r.len())
    });
    if let Some(remote) = from_upstream {
        return Ok(remote.clone());
    }
    if remotes.iter().any(|r| r == "origin") {
        return Ok("origin".to_owned());
    }
    match remotes {
        [only] => Ok(only.clone()),
        [] => Err(EngineError::NotAllowed(format!(
            "`{id}`: il repository non ha remoti configurati"
        ))),
        _ => Err(EngineError::NotAllowed(format!(
            "`{id}`: il branch attivo non ha un upstream e non c'è un remoto `origin`: indicare \
             `remote` fra {}",
            names(remotes)
        ))),
    }
}

/// [`EngineError::NotAllowed`] per un remoto che non è configurato: niente scaricato.
pub fn unknown_remote(remote: &str, remotes: &[String]) -> EngineError {
    let known = if remotes.is_empty() {
        "il repository non ha remoti configurati".to_owned()
    } else {
        format!("remoti configurati: {}", names(remotes))
    };
    EngineError::NotAllowed(format!(
        "non esiste un remoto `{}` ({known}); niente scaricato",
        remote.escape_debug()
    ))
}

fn names(remotes: &[String]) -> String {
    remotes
        .iter()
        .map(|r| format!("`{}`", r.escape_debug()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Head, Upstream};

    fn status(upstream: Option<&str>) -> Status {
        Status {
            head: Head::Branch {
                name: "main".into(),
                target: None,
            },
            upstream: upstream.map(|name| Upstream {
                name: name.into(),
                ahead: Some(0),
                behind: Some(0),
            }),
            entries: Vec::new(),
            operation: None,
        }
    }

    fn list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn remote_names() {
        for ok in ["origin", "-x", "--upload-pack=evil", "a/b", "fork.mio"] {
            check_remote_name("remote.fetch", ok).unwrap();
        }
        for bad in [
            "", "a..b", "a b", "x\n", "a:b", "x~1", "/x", "x/", "@{u}", "a\0b",
        ] {
            let err = check_remote_name("remote.fetch", bad).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{bad:?}");
        }
    }

    #[test]
    fn default_remote_follows_the_upstream_then_origin() {
        let id = "remote.fetch";
        let remotes = list(&["fork", "fork/a", "origin"]);
        assert_eq!(
            default_remote(id, &status(Some("fork/main")), &remotes).unwrap(),
            "fork"
        );
        // Il nome più lungo vince: `fork/a/main` è del remoto `fork/a`.
        assert_eq!(
            default_remote(id, &status(Some("fork/a/main")), &remotes).unwrap(),
            "fork/a"
        );
        // Upstream locale (`branch.<b>.remote = .`) o assente: `origin`.
        assert_eq!(
            default_remote(id, &status(Some("altro")), &remotes).unwrap(),
            "origin"
        );
        assert_eq!(
            default_remote(id, &status(None), &remotes).unwrap(),
            "origin"
        );
        // Senza `origin`: l'unico remoto, o errore se ce ne sono di più o nessuno.
        assert_eq!(
            default_remote(id, &status(None), &list(&["solo"])).unwrap(),
            "solo"
        );
        let err = default_remote(id, &status(None), &list(&["a", "b"])).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = default_remote(id, &status(None), &[]).unwrap_err();
        assert!(err.to_string().contains("non ha remoti"), "{err}");
    }

    #[test]
    fn unknown_remote_lists_the_configured_ones() {
        let err = unknown_remote("x", &list(&["origin"]));
        assert!(err.to_string().contains("`origin`"), "{err}");
        assert!(matches!(err, EngineError::NotAllowed(_)));
    }
}
