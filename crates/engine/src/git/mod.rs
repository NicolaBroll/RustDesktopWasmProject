//! Accesso a un repository git tramite trait (vedi `impl-docs/ARCHITETTURA-git.md` §4).
//!
//! Il motore non esegue `git` e non tocca il file system: legge il repo tramite
//! [`GitBackend`], aperto da un [`RepoOpener`] iniettato dall'app. Le implementazioni sono
//! [`MemoryBackend`] (qui, per i test e la versione web) e `CliBackend` (in `project-git`).

mod memory;
mod sample;

#[cfg(test)]
mod memory_tests;

pub use memory::{MemoryBackend, MemoryOpener};

use crate::EngineError;
use project_domain::{Commit, Head, RefInfo, Status};
use serde::{Deserialize, Serialize};

/// Un repository aperto, in sola lettura (le scritture arrivano con gli step successivi).
///
/// `Send` perché la sessione può spostarlo su un thread di lavoro per i comandi lenti (§6).
pub trait GitBackend: Send {
    /// Dove punta HEAD.
    fn head(&self) -> Result<Head, EngineError>;

    /// Branch locali, branch remoti, tag e stash, ordinati per nome completo (byte per byte),
    /// come `git for-each-ref`. Per i branch locali `upstream` riporta il nome completo del ref
    /// remoto (es. `refs/remotes/origin/main`) e ahead/behind, `None` se il ref non esiste più.
    fn refs(&self) -> Result<Vec<RefInfo>, EngineError>;

    /// Commit raggiungibili da `query.rev` (HEAD se manca), figli sempre prima dei genitori e,
    /// a parità, il più recente per data del committer prima. Su un branch senza commit e senza
    /// `rev` il risultato è vuoto; una `rev` che non si risolve dà
    /// [`EngineError::UnknownRev`].
    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError>;

    /// Stato del worktree e dell'indice, con HEAD e upstream del branch corrente.
    fn status(&self) -> Result<Status, EngineError>;
}

/// Apre un repository dato il percorso. L'app lo inietta nella sessione: desktop e CLI usano
/// `CliOpener` di `project-git`, la versione web [`MemoryOpener`].
pub trait RepoOpener: Send + Sync {
    /// [`EngineError::RepoNotFound`] se al percorso non c'è un repository.
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError>;
}

/// Una pagina del log: parametri di `repo.log` (§4), `{"skip"?, "limit"?, "rev"?}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogQuery {
    /// Commit da saltare dall'inizio del log.
    pub skip: usize,
    /// Numero massimo di commit; `None` = tutti.
    pub limit: Option<usize>,
    /// Da dove partire: nome di un ref (`main`, `origin/main`, `v1.0`, `refs/heads/main`),
    /// `HEAD` o un id (anche abbreviato, almeno 4 cifre). `None` = HEAD.
    pub rev: Option<String>,
}

impl LogQuery {
    /// Tutto il log da HEAD.
    pub fn all() -> Self {
        Self::default()
    }

    /// `limit` commit dopo i primi `skip`, da HEAD.
    pub fn page(skip: usize, limit: usize) -> Self {
        Self {
            skip,
            limit: Some(limit),
            rev: None,
        }
    }

    /// Stessa pagina, a partire da `rev`.
    pub fn from_rev(mut self, rev: impl Into<String>) -> Self {
        self.rev = Some(rev.into());
        self
    }
}
