//! Context of a slow command (`impl-docs/ARCHITECTURE-git.md` §6): progress and
//! cancellation.
//!
//! [`Registry::run`](crate::Registry::run) stays synchronous. Whoever runs a slow command off the
//! UI thread launches it with [`Registry::run_with`](crate::Registry::run_with) and a
//! [`JobContext`]: the command receives the context from the [`Session`](crate::Session)
//! ([`Session::job`](crate::Session::job)) and passes it to the backend, which reports progress with
//! [`JobContext::report`] (from any thread) and checks [`JobContext::is_cancelled`]. CLI,
//! MCP and tests that use `Registry::run` receive an empty context: no progress, never
//! cancelled.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A progress step of a slow command, e.g. a line `Receiving objects:  45% (9/20)`
/// from `git fetch --progress`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The phase, without the `remote: ` prefix: `Counting objects`, `Compressing objects`,
    /// `Receiving objects`, `Resolving deltas`, ...
    pub phase: String,
    /// True if the phase is on the remote (git's `remote: ...` line).
    pub remote: bool,
    /// Percentage of the phase (0–100), if git gives it.
    pub percent: Option<u8>,
    /// Items done, if git counts them (`9` in `(9/20)`, or `123` in `Counting objects: 123`).
    pub current: Option<u64>,
    /// Total items, if known (`20` in `(9/20)`).
    pub total: Option<u64>,
    /// True on the last line of the phase (`..., done.`).
    pub done: bool,
    /// The line as is (without `\r`/`\n`), e.g. to show it verbatim or for details the
    /// parser does not extract (transfer speed).
    pub text: String,
}

/// Cooperative cancellation of a command: whoever launched it calls
/// [`CancelToken::cancel`] (from any thread), the backend checks it while working. Copies
/// share the state.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks to cancel the command. A command that sees it stops as soon as it can and returns
    /// [`EngineError::Cancelled`](crate::EngineError::Cancelled); one already finished is unchanged.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Receiver of progress: called from the command's thread (or from a backend thread).
pub type ProgressSink = Arc<dyn Fn(&Progress) + Send + Sync>;

/// Progress and cancellation of a command, passed with
/// [`Registry::run_with`](crate::Registry::run_with). The default receives nothing and is never
/// cancelled.
#[derive(Clone, Default)]
pub struct JobContext {
    sink: Option<ProgressSink>,
    cancel: CancelToken,
}

impl JobContext {
    /// Empty context: no progress, never cancelled (the one of `Registry::run`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Sends progress to `sink`.
    pub fn with_progress(mut self, sink: impl Fn(&Progress) + Send + Sync + 'static) -> Self {
        self.sink = Some(Arc::new(sink));
        self
    }

    /// The command is cancelled with `cancel` (a copy of the token of whoever launched it).
    pub fn with_cancel(mut self, cancel: CancelToken) -> Self {
        self.cancel = cancel;
        self
    }

    /// Reports a progress step.
    pub fn report(&self, progress: &Progress) {
        if let Some(sink) = &self.sink {
            sink(progress);
        }
    }

    /// True if whoever launched the command asked to cancel it.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
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
        assert!(job.is_cancelled());

        // The empty context receives nothing and cannot be cancelled.
        let empty = JobContext::new();
        empty.report(&progress);
        assert!(!empty.is_cancelled());
    }
}
