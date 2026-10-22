//! L5 — motore indipendente dalla UI.
//!
//! Ogni operazione sul repository passa da [`Registry::run`], usato allo stesso modo da UI,
//! CLI, server MCP e test. I repository git si leggono tramite i trait [`GitBackend`] e
//! [`RepoOpener`]; la [`Session`] tiene il repo aperto.

mod builtin;
mod commands;
mod git;
mod platform;
mod session;

pub use builtin::DEFAULT_LOG_LIMIT;
pub use commands::{CommandSpec, Handler, Registry};
pub use git::{
    BACKUP_PREFIX, DiffSpec, GitBackend, LinePatch, LineSelection, LogQuery, MemoryBackend,
    MemoryOpener, RepoOpener, StagedVersion, apply_patch, backup_message, file_digest, line_patch,
    path_matches, versions_at_risk, versions_at_risk_hidden,
};
pub use platform::{Capabilities, Picked, Platform};
pub use project_core::Invocation;
pub use session::Session;

use project_core::CoreError;
use project_domain::DomainError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("comando sconosciuto: `{0}`")]
    UnknownCommand(String),
    #[error("parametri non validi per `{id}`: {reason}")]
    InvalidParams { id: &'static str, reason: String },
    /// Al percorso non c'è un repository git (o non si riesce ad aprirlo).
    #[error("`{path}` non è un repository git: {reason}")]
    RepoNotFound { path: String, reason: String },
    /// Il comando ha bisogno di un repo, ma la sessione non ne ha uno aperto.
    #[error("nessun repository aperto: aprirne uno con `repo.open`")]
    NoRepo,
    /// La rev non corrisponde a nessun ref né commit (o l'id abbreviato è ambiguo).
    #[error("revisione sconosciuta: `{0}`")]
    UnknownRev(String),
    /// Il programma `git` non si avvia (non installato o non nel `PATH`).
    #[error("impossibile eseguire `git`: {0}")]
    GitNotFound(String),
    /// Il `git` installato è più vecchio della versione minima supportata.
    #[error("git {found} non è supportato: serve almeno la versione {required}")]
    GitTooOld { found: String, required: String },
    /// Un comando git è terminato con errore; `stderr` è il suo messaggio.
    #[error("`git {command}` non riuscito: {stderr}")]
    Git {
        /// Il sottocomando git, es. `status`.
        command: String,
        stderr: String,
    },
    /// L'indice è bloccato da un altro processo git: il file `lock` (di solito
    /// `.git/index.lock`) esiste. Non si riprova e non si cancella il lock: se nessun git è in
    /// esecuzione è rimasto da un crash e va tolto a mano.
    #[error(
        "l'indice è bloccato: `{lock}` esiste. Un altro processo git sta lavorando sul repository; \
         riprovare quando ha finito. Se nessun git è in esecuzione il file è rimasto da un crash \
         e va rimosso a mano"
    )]
    IndexLocked { lock: String },
    /// L'operazione è rifiutata nello stato attuale del repo (es. stage di un file ignorato,
    /// unstage di un file in conflitto); niente è stato modificato.
    #[error("operazione rifiutata: {0}")]
    NotAllowed(String),
    /// Il diff da cui vengono le righe scelte (`index.stage_lines`, `index.unstage_lines`) non
    /// è più quello attuale: il file è cambiato da quando il diff è stato letto (o non ha più
    /// modifiche). Niente è stato modificato; va riletto il diff e rifatta la selezione.
    #[error(
        "il diff di `{path}` non è più quello letto ({reason}): niente modificato, rileggere il \
         diff e rifare la selezione"
    )]
    StaleDiff { path: String, reason: String },
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Domain(#[from] DomainError),
}

impl EngineError {
    /// [`EngineError::NotAllowed`] per lo stage di file ignorati chiesti per nome: niente è
    /// stato messo in stage.
    pub fn ignored(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(format!(
            "file ignorati da .gitignore (o da un altro file di esclusione), niente messo in \
             stage: {}",
            list(paths)
        ))
    }

    /// [`EngineError::NotAllowed`] per l'unstage di file in conflitto: niente è stato tolto
    /// dallo stage.
    pub fn unmerged(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(format!(
            "file in conflitto, niente tolto dallo stage (si perderebbero le versioni del \
             conflitto; per risolverlo va messo in stage): {}",
            list(paths)
        ))
    }
}

impl EngineError {
    /// [`EngineError::NotAllowed`] per lo stage di una cartella con file in conflitto: un
    /// conflitto si risolve solo chiedendo il file per nome. Niente è stato messo in stage.
    pub fn unmerged_in_folder(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(format!(
            "la cartella contiene file in conflitto, niente messo in stage (metterli in stage \
             li segnerebbe come risolti: vanno chiesti per nome): {}",
            list(paths)
        ))
    }
}

fn list(paths: &[impl std::fmt::Display]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
