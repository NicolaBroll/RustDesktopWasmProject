//! L2 — modello dei dati e logica di dominio.
//!
//! Il documento di esempio è una lista di voci con titolo. Nessuna dipendenza
//! da UI, piattaforma o I/O: il formato di file lavora su byte in memoria.

mod document;
mod format;

pub use document::{Document, Item};
pub use format::{FORMAT_ID, FORMAT_VERSION};

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
}
