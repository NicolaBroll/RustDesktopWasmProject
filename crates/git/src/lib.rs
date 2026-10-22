//! L6 — backend git che esegue il comando `git` di sistema.
//!
//! [`CliOpener`] apre un repository e restituisce un [`CliBackend`], che implementa
//! [`project_engine::GitBackend`] leggendo l'output di git con i parser del dominio. Le regole
//! per ogni chiamata a git sono in `impl-docs/ARCHITETTURA-git.md` §5: mai una shell,
//! `-C <repo>`, `--end-of-options` prima delle rev, `GIT_TERMINAL_PROMPT=0`, `LC_ALL=C`.
//!
//! Nessun altro crate L6 importa questo: lo collegano le app L7.

mod backend;
mod command;
mod progress;
mod version;

pub use backend::{CliBackend, CliOpener};
pub use version::GitVersion;
