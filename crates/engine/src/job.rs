//! Contesto di un comando lento (`impl-docs/ARCHITETTURA-git.md` §6): avanzamento e
//! annullamento.
//!
//! [`Registry::run`](crate::Registry::run) resta sincrono. Chi esegue un comando lento fuori dal
//! thread della UI lo lancia con [`Registry::run_with`](crate::Registry::run_with) e un
//! [`JobContext`]: il comando riceve il contesto dalla [`Session`](crate::Session)
//! ([`Session::job`](crate::Session::job)) e lo passa al backend, che segnala l'avanzamento con
//! [`JobContext::report`] (da qualunque thread) e controlla [`JobContext::is_cancelled`]. CLI,
//! MCP e test che usano `Registry::run` ricevono un contesto vuoto: nessun avanzamento, mai
//! annullato.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Un passo di avanzamento di un comando lento, es. una riga `Receiving objects:  45% (9/20)`
/// di `git fetch --progress`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// La fase, senza il prefisso `remote: `: `Counting objects`, `Compressing objects`,
    /// `Receiving objects`, `Resolving deltas`, ...
    pub phase: String,
    /// Vero se la fase è del remoto (riga `remote: ...` di git).
    pub remote: bool,
    /// Percentuale della fase (0–100), se git la dà.
    pub percent: Option<u8>,
    /// Elementi fatti, se git li conta (`9` in `(9/20)`, o `123` in `Counting objects: 123`).
    pub current: Option<u64>,
    /// Elementi totali, se noti (`20` in `(9/20)`).
    pub total: Option<u64>,
    /// Vero sull'ultima riga della fase (`..., done.`).
    pub done: bool,
    /// La riga com'è (senza `\r`/`\n`), es. per mostrarla così com'è o per i dettagli che il
    /// parser non estrae (velocità di trasferimento).
    pub text: String,
}

/// Annullamento cooperativo di un comando: chi l'ha lanciato chiama
/// [`CancelToken::cancel`] (da qualunque thread), il backend lo controlla mentre lavora. Le
/// copie condividono lo stato.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Chiede di annullare il comando. Un comando che lo vede smette appena può e restituisce
    /// [`EngineError::Cancelled`](crate::EngineError::Cancelled); uno già finito non cambia.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Chi riceve l'avanzamento: chiamato dal thread del comando (o da un thread del backend).
pub type ProgressSink = Arc<dyn Fn(&Progress) + Send + Sync>;

/// Avanzamento e annullamento di un comando, passati con
/// [`Registry::run_with`](crate::Registry::run_with). Il default non riceve niente e non è mai
/// annullato.
#[derive(Clone, Default)]
pub struct JobContext {
    sink: Option<ProgressSink>,
    cancel: CancelToken,
}

impl JobContext {
    /// Contesto vuoto: nessun avanzamento, mai annullato (quello di `Registry::run`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Manda l'avanzamento a `sink`.
    pub fn with_progress(mut self, sink: impl Fn(&Progress) + Send + Sync + 'static) -> Self {
        self.sink = Some(Arc::new(sink));
        self
    }

    /// Il comando si annulla con `cancel` (una copia del token di chi lo lancia).
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Segnala un passo di avanzamento.
    pub fn report(&self, progress: &Progress) {
        if let Some(sink) = &self.sink {
            sink(progress);
        }
    }

    /// Vero se chi ha lanciato il comando ha chiesto di annullarlo.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Il token di annullamento.
    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }
}

impl fmt::Debug for JobContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobContext")
            .field("progress", &self.sink.is_some())
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn reports_to_the_sink_and_shares_cancellation() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let token = CancelToken::new();
        let job = JobContext::new()
            .with_progress(move |p| sink.lock().unwrap().push(p.phase.clone()))
            .with_cancel(token.clone());
        let progress = Progress {
            phase: "Receiving objects".into(),
            remote: false,
            percent: Some(50),
            current: Some(1),
            total: Some(2),
            done: false,
            text: "Receiving objects:  50% (1/2)".into(),
        };
        job.report(&progress);
        assert_eq!(*seen.lock().unwrap(), ["Receiving objects"]);
        assert!(!job.is_cancelled());
        token.cancel();
        assert!(job.is_cancelled());
        assert!(job.clone().is_cancelled());

        // Il contesto vuoto non riceve niente e non si annulla.
        let empty = JobContext::new();
        empty.report(&progress);
        assert!(!empty.is_cancelled());
    }
}
