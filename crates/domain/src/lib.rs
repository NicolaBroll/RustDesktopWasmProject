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
    Commit, GraphLayout, GraphRow, GraphSegment, Head, Oid, RefInfo, RefKind, Signature, Upstream,
    layout_graph, layout_graph_nodes,
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
}
