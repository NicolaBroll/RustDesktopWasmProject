//! L2 — modello dei dati e logica di dominio.
//!
//! Il modulo [`git`] contiene i tipi del client git e i parser dell'output di git (vedi
//! `impl-docs/ARCHITETTURA-git.md` §3), esportati anche dalla radice del crate. Nessuna
//! dipendenza da UI, piattaforma o I/O: i parser lavorano su byte in memoria.

pub mod git;

pub use git::{
    Change, Commit, Conflict, EntryKind, GraphLayout, GraphRow, GraphSegment, Head, LOG_ARGS,
    LOG_FIELDS, LOG_FORMAT, Oid, Operation, REFS_ARGS, REFS_FIELDS, REFS_FORMAT, RefInfo, RefKind,
    RepoPath, STATUS_ARGS, Signature, Status, StatusEntry, SubmoduleStatus, TrackedInfo,
    UnmergedInfo, Upstream, layout_graph, layout_graph_nodes, parse_log, parse_refs, parse_status,
};

/// Errori di dominio.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
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
