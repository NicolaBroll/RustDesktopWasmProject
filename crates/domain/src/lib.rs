//! L2 — modello dei dati e logica di dominio.
//!
//! Il documento di esempio è una lista di voci con titolo. Nessuna dipendenza
//! da UI, piattaforma o I/O: il formato di file lavora su byte in memoria.
//!
//! Il modulo [`git`] contiene i tipi del client git (vedi `impl-docs/ARCHITETTURA-git.md` §3),
//! esportati anche dalla radice del crate.

mod document;
mod format;
pub mod git;

pub use document::{Document, Item};
pub use format::{FORMAT_ID, FORMAT_VERSION};
pub use git::{
    Change, Commit, Conflict, EntryKind, Head, LOG_ARGS, LOG_FIELDS, LOG_FORMAT, Oid, Operation,
    RefInfo, RefKind, RepoPath, STATUS_ARGS, Signature, Status, StatusEntry, SubmoduleStatus,
    TrackedInfo, UnmergedInfo, Upstream, parse_log, parse_status,
};

/// Errori di dominio.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("il testo non può essere vuoto")]
    EmptyText,
    #[error("indice {index} fuori intervallo (voci: {len})")]
    IndexOutOfRange { index: usize, len: usize },
    #[error("file non valido: {0}")]
    InvalidFile(String),
    #[error("formato `{format}` versione {version} non supportato")]
    UnsupportedFormat { format: String, version: u32 },
    #[error("identificatore git `{input}` non valido: {reason}")]
    InvalidOid { input: String, reason: String },
    /// L'output di un comando git non ha il formato atteso dal parser.
    #[error("output di `git {command}` non valido: {reason}")]
    InvalidGitOutput {
        /// Il sottocomando git, es. `log`.
        command: &'static str,
        reason: String,
    },
}
