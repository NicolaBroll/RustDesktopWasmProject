//! L0 — base types shared by all layers: errors and input parsing.
//!
//! This crate does not depend on any other crate in the workspace.

mod invocation;

pub use invocation::{Invocation, is_valid_command_id};

/// L0 errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoreError {
    #[error("invalid command id: `{0}` (allowed: a-z, 0-9, `.`, `_`, `-`)")]
    InvalidCommandId(String),
    #[error("invalid JSON parameters for `{id}`: {reason}")]
    InvalidParams { id: String, reason: String },
}
