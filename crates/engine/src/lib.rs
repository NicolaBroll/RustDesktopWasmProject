//! L5 — motore indipendente dalla UI.
//!
//! Tutte le modifiche al documento passano da [`Registry::run`], usato allo
//! stesso modo da UI, CLI, server MCP e test. I repository git si leggono tramite i trait
//! [`GitBackend`] e [`RepoOpener`].

mod builtin;
mod commands;
mod git;
mod platform;
mod session;

pub use commands::{CommandSpec, Handler, Registry};
pub use git::{GitBackend, LogQuery, MemoryBackend, MemoryOpener, RepoOpener};
pub use platform::{Capabilities, Picked, Platform};
pub use project_core::Invocation;
pub use project_domain::{Document, Item};
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
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Domain(#[from] DomainError),
}
