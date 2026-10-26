//! Running commands off the UI thread (`impl-docs/ARCHITECTURE-git.md` §6).
//!
//! The UI does not call [`Registry::run`] directly: it sends the command to the [`Executor`],
//! which runs it with [`Platform::spawn`] on the shared session and returns the result over a
//! channel. The UI reads results with [`Executor::poll`] on every frame and never touches the
//! session lock, so it does not block while a slow command holds it.
//!
//! Every command starts with [`Registry::run_with`] and a [`JobContext`] for its ticket: the
//! last [`Progress`] received is read with [`Executor::progress`], and [`Executor::cancel`]
//! cancels it (S25, `remote.fetch`). Commands that are not slow ignore both.

use project_engine::{
    CancelToken, EngineError, JobContext, Platform, Progress, PullOperation, PushRejection,
    Registry, Session,
};
use project_i18n::tr;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Identifies a command sent to the [`Executor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket(u64);

impl Ticket {
    #[cfg(test)]
    pub(crate) fn for_test(n: u64) -> Self {
        Ticket(n)
    }
}

/// A command in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub ticket: Ticket,
    /// Command id, e.g. `repo.open`.
    pub command: String,
    /// True if the command modifies the repository ([`CommandSpec::writes`](project_engine::CommandSpec::writes)).
    pub writes: bool,
}

/// A finished command.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub ticket: Ticket,
    pub command: String,
    /// The command result, or the error message.
    pub result: Result<Value, String>,
    /// The error kind, if the command failed: the UI reacts differently, for
    /// example it re-reads the diff after [`ErrorKind::StaleDiff`].
    pub error_kind: Option<ErrorKind>,
    /// The structured error detail, if the kind has one: the reason of
    /// [`ErrorKind::ConfirmRequired`], the full git output (stderr) for [`ErrorKind::Git`].
    pub error_detail: Option<String>,
    /// The full id to send back as `"expected"` with `"confirm": true`, if the command
    /// gave it with [`ErrorKind::ConfirmRequired`] (the force of `remote.push`).
    pub confirm_expected: Option<String>,
}

/// The kind of an engine error, for UI reactions that do not depend on the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// [`EngineError::StaleDiff`]: the diff seen is no longer the current one.
    StaleDiff,
    /// [`EngineError::NotAllowed`]: operation refused in the repo's current state.
    NotAllowed,
    /// [`EngineError::InvalidParams`].
    InvalidParams,
    /// [`EngineError::IndexLocked`].
    IndexLocked,
    /// [`EngineError::UnknownRev`]: the rev matches no ref or commit.
    UnknownRev,
    /// [`EngineError::ConfirmRequired`]: `"confirm": true` is needed; the reason is in
    /// [`Done::error_detail`].
    ConfirmRequired,
    /// [`EngineError::NothingToCommit`].
    NothingToCommit,
    /// [`EngineError::Git`]: git ended with an error (e.g. a failing hook); its
    /// stderr is in [`Done::error_detail`].
    Git,
    /// [`EngineError::PostCommit`]: the commit succeeded, but afterwards something is off (e.g. HEAD
    /// moved by a `post-commit` hook); do not repeat it, re-read status and log.
    PostCommit,
    /// [`EngineError::CheckoutConflict`]: the checkout would overwrite local changes and
    /// nothing changed; the files git named are in [`Done::error_detail`], one per line.
    CheckoutConflict,
    /// [`EngineError::Auth`]: the remote rejected the credentials or they would have to be asked for;
    /// git's text is in [`Done::error_detail`].
    Auth,
    /// [`EngineError::RemoteUnreachable`]: the remote cannot be reached; git's text is in
    /// [`Done::error_detail`].
    RemoteUnreachable,
    /// [`EngineError::Cancelled`]: cancelled with [`Executor::cancel`].
    Cancelled,
    /// [`EngineError::PullConflict`]: the merge or rebase of the pull stopped on a conflict
    /// and was aborted (the repo is as before, the fetch stays done); the conflicted files
    /// are in [`Done::error_detail`], one per line.
    PullConflict(PullOperation),
    /// [`EngineError::PushRejected`]: the push was rejected and nothing changed; git's text
    /// (including a hook's output) is in [`Done::error_detail`].
    PushRejected(PushRejection),
    /// [`EngineError::RepoNotFound`]: there is no repository at the path given to `repo.open`.
    RepoNotFound,
    /// All other errors.
    Other,
}

impl ErrorKind {
    pub fn of(error: &EngineError) -> Self {
        match error {
            EngineError::StaleDiff { .. } => ErrorKind::StaleDiff,
            EngineError::NotAllowed(_) => ErrorKind::NotAllowed,
            EngineError::RepoNotFound { .. } => ErrorKind::RepoNotFound,
            EngineError::InvalidParams { .. } => ErrorKind::InvalidParams,
            EngineError::IndexLocked { .. } => ErrorKind::IndexLocked,
            EngineError::UnknownRev(_) => ErrorKind::UnknownRev,
            EngineError::ConfirmRequired { .. } => ErrorKind::ConfirmRequired,
            EngineError::NothingToCommit => ErrorKind::NothingToCommit,
            EngineError::Git { .. } | EngineError::GitWithReason { .. } => ErrorKind::Git,
            EngineError::PostCommit { .. } => ErrorKind::PostCommit,
            EngineError::CheckoutConflict { .. } => ErrorKind::CheckoutConflict,
            EngineError::Auth { .. } => ErrorKind::Auth,
            EngineError::RemoteUnreachable { .. } => ErrorKind::RemoteUnreachable,
            EngineError::Cancelled { .. } => ErrorKind::Cancelled,
            EngineError::PullConflict { operation, .. } => ErrorKind::PullConflict(*operation),
            EngineError::PushRejected { reason, .. } => ErrorKind::PushRejected(*reason),
            _ => ErrorKind::Other,
        }
    }

    /// The id to put in [`Done::confirm_expected`]: the `expected` of a confirmation.
    pub fn expected(error: &EngineError) -> Option<String> {
        match error {
            EngineError::ConfirmRequired { expected, .. } => expected.clone(),
            _ => None,
        }
    }

    /// The detail to put in [`Done::error_detail`]: the reason of a confirmation, git's stderr,
    /// the files a checkout would overwrite (one per line), git's text for a remote that
    /// rejects the credentials or cannot be reached, the conflicted files of a pull (one per
    /// line), git's text of a rejected push.
    pub fn detail(error: &EngineError) -> Option<String> {
        match error {
            EngineError::ConfirmRequired { reason, .. }
            | EngineError::GitWithReason { reason, .. } => Some(reason.text()),
            EngineError::Git { stderr, .. } => Some(stderr.clone()),
            EngineError::Auth { message, .. }
            | EngineError::RemoteUnreachable { message, .. }
            | EngineError::PushRejected { message, .. } => Some(message.clone()),
            EngineError::PullConflict { files, .. }
            | EngineError::CheckoutConflict { files, .. } => Some(files.join("\n")),
            _ => None,
        }
    }
}

/// A command error, as it comes from the job.
struct Failure {
    kind: ErrorKind,
    detail: Option<String>,
    message: String,
    expected: Option<String>,
}

impl Failure {
    fn of(error: &EngineError) -> Self {
        Self {
            kind: ErrorKind::of(error),
            detail: ErrorKind::detail(error),
            // In the UI language; `Display` is the English of CLI and MCP.
            message: error.text(),
            expected: ErrorKind::expected(error),
        }
    }
}

/// Called when a result arrives, from any thread: in the UI it requests a new frame.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The latest progress of every command in progress (`None` until one arrives). The job's
/// sink only updates the tickets present: one already collected does not come back.
type ProgressMap = Arc<Mutex<HashMap<Ticket, Option<Progress>>>>;

/// Sends commands to the shared session and collects their results.
pub struct Executor {
    session: Arc<Mutex<Session>>,
    registry: Arc<Registry>,
    platform: Arc<dyn Platform>,
    wake: Wake,
    sender: Sender<Done>,
    receiver: Receiver<Done>,
    next: u64,
    running: Vec<Job>,
    progress: ProgressMap,
    cancel: HashMap<Ticket, CancelToken>,
}

impl Executor {
    pub fn new(
        session: Session,
        registry: Registry,
        platform: Arc<dyn Platform>,
        wake: Wake,
    ) -> Self {
        let (sender, receiver) = channel();
        Self {
            session: Arc::new(Mutex::new(session)),
            registry: Arc::new(registry),
            platform,
            wake,
            sender,
            receiver,
            next: 0,
            running: Vec::new(),
            progress: Arc::default(),
            cancel: HashMap::new(),
        }
    }

    /// Sends the command `command` with `params` and returns at once (after running it, if the
    /// platform runs jobs inside `spawn`). The result arrives from [`Executor::poll`].
    pub fn submit(&mut self, command: &str, params: Value) -> Ticket {
        let ticket = Ticket(self.next);
        self.next += 1;
        let writes = self.registry.get(command).is_some_and(|spec| spec.writes);
        log::debug!("submit {command}");
        self.running.push(Job {
            ticket,
            command: command.to_owned(),
            writes,
        });

        // Before starting the job: a platform may run the command (and send progress) inside
        // `spawn`.
        lock(&self.progress).insert(ticket, None);
        let token = CancelToken::new();
        self.cancel.insert(ticket, token.clone());
        let progress = Arc::clone(&self.progress);
        let wake = Arc::clone(&self.wake);
        let job = JobContext::new()
            .with_cancel(token)
            .with_progress(move |p: &Progress| {
                if let Some(slot) = lock(&progress).get_mut(&ticket) {
                    *slot = Some(p.clone());
                }
                wake();
            });

        let session = Arc::clone(&self.session);
        let registry = Arc::clone(&self.registry);
        let reply = Reply {
            ticket,
            command: command.to_owned(),
            sender: self.sender.clone(),
            wake: Arc::clone(&self.wake),
            sent: false,
        };
        self.platform.spawn(Box::new(move || {
            // A job that ended badly poisons the lock, but the session stays valid: every command
            // leaves it consistent even when it fails.
            let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
            let result = registry
                .run_with(&reply.command, &mut session, params, job)
                .map_err(|e| Failure::of(&e));
            drop(session);
            reply.send(result);
        }));
        ticket
    }

    /// The commands finished since the last call, in the order they finished. Does not block.
    pub fn poll(&mut self) -> Vec<Done> {
        let done: Vec<Done> = self.receiver.try_iter().collect();
        self.running
            .retain(|job| done.iter().all(|d| d.ticket != job.ticket));
        if !done.is_empty() {
            let mut progress = lock(&self.progress);
            for d in &done {
                progress.remove(&d.ticket);
                self.cancel.remove(&d.ticket);
            }
        }
        done
    }

    /// The latest progress of command `ticket`, if it is still running and has sent one.
    pub fn progress(&self, ticket: Ticket) -> Option<Progress> {
        lock(&self.progress).get(&ticket).cloned().flatten()
    }

    /// Asks to cancel command `ticket`: a slow command stops as soon as it can and ends
    /// with [`ErrorKind::Cancelled`]; the others finish normally. False if the command is no
    /// longer running.
    pub fn cancel(&self, ticket: Ticket) -> bool {
        match self.cancel.get(&ticket) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// The commands sent and not yet collected by [`Executor::poll`], oldest first.
    pub fn running(&self) -> &[Job] {
        &self.running
    }

    /// True if at least one command is in progress.
    pub fn is_busy(&self) -> bool {
        !self.running.is_empty()
    }

    /// The write in progress, if any: until it is collected by [`Executor::poll`] the UI does not
    /// send another and disables actions that modify the repo.
    pub fn writing(&self) -> Option<&Job> {
        self.running.iter().find(|job| job.writes)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sends a job's result. If the job ends without sending it (panic in the command, or the
/// platform failing to start it and discarding it) it sends an error, so the command does not
/// stay "in progress" forever.
struct Reply {
    ticket: Ticket,
    command: String,
    sender: Sender<Done>,
    wake: Wake,
    sent: bool,
}

impl Reply {
    fn send(mut self, result: Result<Value, Failure>) {
        self.deliver(result);
    }

    fn deliver(&mut self, result: Result<Value, Failure>) {
        self.sent = true;
        let (error_kind, error_detail, confirm_expected) = match &result {
            Ok(_) => (None, None, None),
            Err(f) => (Some(f.kind), f.detail.clone(), f.expected.clone()),
        };
        // If the UI is already closed nobody is waiting for the result.
        let _ = self.sender.send(Done {
            ticket: self.ticket,
            command: self.command.clone(),
            result: result.map_err(|f| f.message),
            error_kind,
            error_detail,
            confirm_expected,
        });
        (self.wake)();
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if !self.sent {
            self.deliver(Err(Failure {
                kind: ErrorKind::Other,
                detail: None,
                message: tr!("app-no-result", command = &self.command),
                expected: None,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{Capabilities, CommandSpec, EngineError, MemoryOpener, Platform};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    const SLOW: Duration = Duration::from_millis(300);

    /// Test platform: real threads as on desktop, or immediate execution inside `spawn`,
    /// or discarded jobs (a thread that does not start).
    enum TestPlatform {
        Threads,
        Inline,
        Drop,
    }

    impl Platform for TestPlatform {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
            on_done(None);
        }
        fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
            match self {
                TestPlatform::Threads => drop(std::thread::spawn(job)),
                TestPlatform::Inline => job(),
                TestPlatform::Drop => drop(job),
            }
        }
    }

    fn slow_write(_: &mut Session, _: Value) -> Result<Value, EngineError> {
        std::thread::sleep(SLOW);
        Ok(json!({}))
    }

    fn slow(_: &mut Session, params: Value) -> Result<Value, EngineError> {
        std::thread::sleep(SLOW);
        Ok(json!({ "echo": params }))
    }

    /// Sends progress until cancelled (at most 5 seconds).
    fn until_cancelled(s: &mut Session, _: Value) -> Result<Value, EngineError> {
        let start = Instant::now();
        let mut n = 0;
        while start.elapsed() < Duration::from_secs(5) {
            if s.job().is_cancelled() {
                return Err(EngineError::Cancelled {
                    command: "test.cancel",
                });
            }
            n += 1;
            s.job().report(&Progress {
                phase: "Receiving objects".into(),
                remote: false,
                percent: None,
                current: Some(n),
                total: None,
                done: false,
                text: format!("Receiving objects: {n}"),
            });
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(json!({}))
    }

    fn panics(_: &mut Session, _: Value) -> Result<Value, EngineError> {
        panic!("fake command that panics");
    }

    /// The real catalog plus `test.slow` (sleeps [`SLOW`]), `test.panic` and `test.write` (a
    /// write that sleeps [`SLOW`]).
    fn registry() -> Registry {
        let mut registry = Registry::builtin();
        let spec = |id| CommandSpec {
            id,
            title: "test",
            params_help: "{}",
            writes: false,
        };
        registry.register(spec("test.slow"), slow);
        registry.register(spec("test.panic"), panics);
        registry.register(spec("test.cancel"), until_cancelled);
        registry.register(
            CommandSpec {
                writes: true,
                ..spec("test.write")
            },
            slow_write,
        );
        registry
    }

    fn executor(platform: TestPlatform) -> (Executor, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let wake: Wake = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let session = Session::new(Arc::new(MemoryOpener::sample()));
        let executor = Executor::new(session, registry(), Arc::new(platform), wake);
        (executor, wakes)
    }

    /// Calls `poll` until `n` results arrive (at most 5 seconds).
    fn wait(executor: &mut Executor, n: usize) -> Vec<Done> {
        let start = Instant::now();
        let mut done = Vec::new();
        while done.len() < n {
            assert!(start.elapsed() < Duration::from_secs(5), "missing results");
            done.extend(executor.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        done
    }

    #[test]
    fn slow_command_does_not_block_the_caller() {
        let (mut ex, wakes) = executor(TestPlatform::Threads);
        let start = Instant::now();
        let ticket = ex.submit("test.slow", json!({ "n": 1 }));
        // The UI keeps drawing frames while the command works.
        assert!(start.elapsed() < SLOW / 2, "submit waited for the command");
        assert!(ex.is_busy());
        assert_eq!(ex.running()[0].command, "test.slow");
        assert!(ex.poll().is_empty());

        let done = wait(&mut ex, 1);
        assert!(start.elapsed() >= SLOW);
        assert_eq!(done[0].ticket, ticket);
        assert_eq!(done[0].result, Ok(json!({ "echo": { "n": 1 } })));
        assert!(!ex.is_busy());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn commands_share_the_session() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        ex.submit("test.slow", json!({}));
        let open = ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        // The session lock queues the commands: the status sees the repo opened first.
        let done = wait(&mut ex, 2);
        let status = ex.submit("repo.status", json!({}));
        let after = wait(&mut ex, 1);
        assert!(done.iter().any(|d| d.ticket == open && d.result.is_ok()));
        assert_eq!(after[0].ticket, status);
        assert!(after[0].result.is_ok(), "{:?}", after[0].result);
    }

    #[test]
    fn inline_platform_delivers_before_returning() {
        let (mut ex, _) = executor(TestPlatform::Inline);
        ex.submit("repo.status", json!({}));
        let done = ex.poll();
        assert_eq!(done.len(), 1);
        assert!(done[0].result.as_ref().unwrap_err().contains("repo.open"));
        assert_eq!(done[0].error_kind, Some(ErrorKind::Other));
        assert!(!ex.is_busy());
    }

    #[test]
    fn errors_carry_their_kind() {
        let (mut ex, _) = executor(TestPlatform::Inline);
        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        assert_eq!(ex.poll()[0].error_kind, None);
        ex.submit("index.stage", json!({ "paths": [] }));
        assert_eq!(ex.poll()[0].error_kind, Some(ErrorKind::InvalidParams));
        let stale =
            json!({ "path": "src/lib.rs", "lines": [{ "new": 1 }], "digest": "0".repeat(32) });
        ex.submit("index.stage_lines", stale);
        assert_eq!(ex.poll()[0].error_kind, Some(ErrorKind::StaleDiff));

        ex.submit("commit.create", json!({ "message": "first" }));
        let done = ex.poll().remove(0);
        assert!(done.result.is_ok(), "{:?}", done.result);
        assert_eq!(done.error_detail, None);
        ex.submit("commit.create", json!({ "message": "again" }));
        let done = ex.poll().remove(0);
        assert_eq!(done.error_kind, Some(ErrorKind::NothingToCommit));
        assert_eq!(done.error_detail, None);
    }

    #[test]
    fn confirm_and_git_errors_carry_their_detail() {
        let confirm = EngineError::ConfirmRequired {
            command: "commit.create",
            reason: project_engine::msg!(
                "engine-other",
                text = "the commit is on refs/remotes/origin/main"
            ),
            expected: None,
        };
        assert_eq!(ErrorKind::of(&confirm), ErrorKind::ConfirmRequired);
        assert_eq!(
            ErrorKind::detail(&confirm).as_deref(),
            Some("the commit is on refs/remotes/origin/main")
        );
        assert_eq!(ErrorKind::expected(&confirm), None);
        let force = EngineError::ConfirmRequired {
            command: "remote.push",
            reason: project_engine::msg!("engine-other", text = "the remote would lose 1 commit"),
            expected: Some("a".repeat(40)),
        };
        assert_eq!(ErrorKind::expected(&force), Some("a".repeat(40)));
        let git = EngineError::Git {
            command: "commit".to_owned(),
            stderr: "pre-commit: line 1\nline 2".to_owned(),
        };
        assert_eq!(ErrorKind::of(&git), ErrorKind::Git);
        assert_eq!(
            ErrorKind::detail(&git).as_deref(),
            Some("pre-commit: line 1\nline 2")
        );
        assert_eq!(ErrorKind::detail(&EngineError::NoRepo), None);
        let post = EngineError::PostCommit {
            commit: None,
            backup: None,
            reason: project_engine::msg!("engine-other", text = "HEAD changed"),
        };
        assert_eq!(ErrorKind::of(&post), ErrorKind::PostCommit);
        let checkout = EngineError::CheckoutConflict {
            target: "feature/x".to_owned(),
            files: vec!["README.md".to_owned(), "src/lib.rs".to_owned()],
            stderr: "error: ...".to_owned(),
        };
        assert_eq!(ErrorKind::of(&checkout), ErrorKind::CheckoutConflict);
        assert_eq!(
            ErrorKind::detail(&checkout).as_deref(),
            Some("README.md\nsrc/lib.rs")
        );
    }

    #[test]
    fn progress_is_kept_per_ticket_and_cancel_stops_the_job() {
        let (mut ex, wakes) = executor(TestPlatform::Threads);
        let ticket = ex.submit("test.cancel", json!({}));
        let start = Instant::now();
        while ex.progress(ticket).is_none() {
            assert!(start.elapsed() < Duration::from_secs(5), "no progress");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            wakes.load(Ordering::SeqCst) >= 1,
            "progress requests a frame"
        );
        assert_eq!(ex.progress(ticket).unwrap().phase, "Receiving objects");
        assert!(ex.is_busy());
        assert!(ex.cancel(ticket));
        let done = wait(&mut ex, 1).remove(0);
        assert_eq!(done.ticket, ticket);
        assert_eq!(done.error_kind, Some(ErrorKind::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(4), "did not stop");
        assert_eq!(ex.progress(ticket), None);
        assert!(!ex.cancel(ticket), "already finished");
    }

    #[test]
    fn a_fetch_cancelled_while_waiting_does_not_start() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        wait(&mut ex, 1);
        // The fetch waits for the session lock behind a slow command; cancelled before
        // starting it downloads nothing.
        ex.submit("test.slow", json!({}));
        let fetch = ex.submit("remote.fetch", json!({}));
        assert!(ex.writing().is_some_and(|j| j.ticket == fetch));
        assert!(ex.cancel(fetch));
        let done = wait(&mut ex, 2);
        let fetched = done.iter().find(|d| d.ticket == fetch).unwrap();
        assert_eq!(fetched.error_kind, Some(ErrorKind::Cancelled));
        ex.submit("remote.fetch", json!({}));
        let again = wait(&mut ex, 1).remove(0);
        let updated = &again.result.unwrap()["updated"];
        assert_eq!(updated.as_array().map(Vec::len), Some(1), "{updated}");
    }

    #[test]
    fn remote_errors_carry_the_git_message() {
        let auth = EngineError::Auth {
            remote: "origin".into(),
            message: "fatal: Authentication failed".into(),
        };
        assert_eq!(ErrorKind::of(&auth), ErrorKind::Auth);
        assert_eq!(
            ErrorKind::detail(&auth).as_deref(),
            Some("fatal: Authentication failed")
        );
        let net = EngineError::RemoteUnreachable {
            remote: "origin".into(),
            message: "fatal: Could not resolve host".into(),
        };
        assert_eq!(ErrorKind::of(&net), ErrorKind::RemoteUnreachable);
        assert_eq!(
            ErrorKind::detail(&net).as_deref(),
            Some("fatal: Could not resolve host")
        );
        let cancelled = EngineError::Cancelled {
            command: "remote.fetch",
        };
        assert_eq!(ErrorKind::of(&cancelled), ErrorKind::Cancelled);
        assert_eq!(ErrorKind::detail(&cancelled), None);
        let conflict = EngineError::PullConflict {
            branch: "main".into(),
            upstream: "origin/main".into(),
            operation: PullOperation::Rebase,
            files: vec!["a.txt".into(), "b/c.rs".into()],
            message: "CONFLICT (content): ...".into(),
        };
        assert_eq!(
            ErrorKind::of(&conflict),
            ErrorKind::PullConflict(PullOperation::Rebase)
        );
        assert_eq!(
            ErrorKind::detail(&conflict).as_deref(),
            Some("a.txt\nb/c.rs")
        );
        let rejected = EngineError::PushRejected {
            remote: "origin".into(),
            branch: "main".into(),
            reason: PushRejection::Hook,
            message: "pre-push: no".into(),
        };
        assert_eq!(
            ErrorKind::of(&rejected),
            ErrorKind::PushRejected(PushRejection::Hook)
        );
        assert_eq!(
            ErrorKind::detail(&rejected).as_deref(),
            Some("pre-push: no")
        );
    }

    #[test]
    fn a_panicking_command_reports_an_error_and_keeps_the_session() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        ex.submit("test.panic", json!({}));
        let done = wait(&mut ex, 1);
        assert!(done[0].result.is_err());
        assert!(!ex.is_busy());

        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        assert!(wait(&mut ex, 1)[0].result.is_ok());
    }

    #[test]
    fn a_dropped_job_reports_an_error() {
        let (mut ex, _) = executor(TestPlatform::Drop);
        ex.submit("repo.status", json!({}));
        let done = ex.poll();
        assert_eq!(done.len(), 1);
        assert!(
            done[0]
                .result
                .as_ref()
                .unwrap_err()
                .contains("without a result")
        );
        assert!(!ex.is_busy());
    }

    #[test]
    fn write_jobs_are_tracked_until_collected() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        let read = ex.submit("test.slow", json!({}));
        assert!(ex.writing().is_none(), "a read is not a write");
        let write = ex.submit("test.write", json!({}));
        let job = ex.writing().expect("write in progress");
        assert_eq!(job.ticket, write);
        assert!(job.writes);
        assert!(
            !ex.running()
                .iter()
                .find(|j| j.ticket == read)
                .unwrap()
                .writes
        );

        let done = wait(&mut ex, 2);
        assert!(done.iter().all(|d| d.result.is_ok()));
        assert!(ex.writing().is_none());
    }

    #[test]
    fn builtin_write_commands_are_marked() {
        let (mut ex, _) = executor(TestPlatform::Drop);
        ex.submit("index.stage", json!({ "paths": ["a"] }));
        assert!(ex.writing().is_some());
        ex.poll();
        assert!(ex.writing().is_none());
        ex.submit("repo.status", json!({}));
        assert!(ex.writing().is_none());
        ex.submit("command.nonexistent", json!({}));
        assert!(ex.writing().is_none());
    }
}
