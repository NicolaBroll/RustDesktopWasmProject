//! L5 — motore indipendente dalla UI.
//!
//! Tutte le modifiche al documento passano da [`Registry::run`], usato allo
//! stesso modo da UI, CLI, server MCP e test.

mod builtin;
mod commands;
mod platform;
mod session;

pub use commands::{CommandSpec, Handler, Registry};
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
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Domain(#[from] DomainError),
}
