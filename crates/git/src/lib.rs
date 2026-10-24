//! L6 — git backend that runs the system `git` command.
//!
//! [`CliOpener`] opens a repository and returns a [`CliBackend`], which implements
//! [`project_engine::GitBackend`] reading git's output with the domain parsers. The rules
//! for every call to git are in `impl-docs/ARCHITECTURE-git.md` §5: never a shell,
//! `-C <repo>`, `--end-of-options` before revs, `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`.
//!
//! No other L6 crate imports this: the L7 apps wire it in.

// `msg!` (error texts, key + parameters) in all modules.
#[macro_use]
extern crate project_engine;

mod backend;
mod command;
mod progress;
mod version;

pub use backend::{CliBackend, CliOpener};
pub use version::GitVersion;
