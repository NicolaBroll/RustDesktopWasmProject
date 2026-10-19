//! L0 — tipi di base condivisi da tutti i livelli: errori e parsing dell'input.
//!
//! Questo crate non dipende da nessun altro crate del workspace.

mod invocation;

pub use invocation::{Invocation, is_valid_command_id};

/// Errori di livello L0.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("id di comando non valido: `{0}` (ammessi: a-z, 0-9, `.`, `_`, `-`)")]
    InvalidCommandId(String),
    #[error("parametri JSON non validi per `{id}`: {reason}")]
    InvalidParams { id: String, reason: String },
}
