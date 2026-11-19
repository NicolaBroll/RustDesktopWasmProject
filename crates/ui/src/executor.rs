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
//!
//! A result that is slow to turn into what the UI shows (a large diff) is prepared in the job
//! too, after the session is released: [`Executor::submit_prepared`].
//!
//! A command that only reads the repository ([`CommandSpec::reader`]) does not wait for a write
//! in progress (a fetch, a commit with a slow hook): it runs on a second session
//! ([`Session::reader`]). An answer read before a write ended never reaches the UI after that
//! write's own: see [`Reading`].

#[cfg(doc)]
use project_engine::CommandSpec;
use project_engine::{
    CancelToken, EngineError, JobContext, Message, MessageArg, Platform, Progress, PushRejection,
    Registry, Session,
};
use project_i18n::tr;
use serde_json::Value;
use std::any::Any;
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
    /// When it was sent: the status bar shows a read only once it takes a while.
    pub started: std::time::Instant,
    /// False if git's credential questions get no dialog (the automatic fetch).
    pub asks: bool,
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
    /// The `bytes` number of a [`ErrorKind::ConfirmRequired`] reason, if it has one: the total
    /// size of the files `worktree.clean` would delete.
    pub confirm_bytes: Option<u64>,
}

/// The kind of an engine error, for UI reactions that do not depend on the text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// [`EngineError::StaleDiff`]: the diff seen is no longer the current one.
    StaleDiff,
    /// [`EngineError::NotAllowed`]: operation refused in the repo's current state.
    NotAllowed,
    /// [`EngineError::NotAllowed`] because of uncommitted changes to tracked files (merge,
    /// cherry-pick, revert, rebase, pull: keys `engine-*-local-changes`); the files are in
    /// [`Done::error_detail`]. Nothing changed: the UI offers to stash them.
    LocalChanges,
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
    /// [`EngineError::AuthCancelled`]: the user cancelled a credential question; nothing
    /// changed. The remote is in [`Done::error_detail`]. Shown as a notice, not as an error.
    AuthCancelled,
    /// [`EngineError::RemoteUnreachable`]: the remote cannot be reached; git's text is in
    /// [`Done::error_detail`].
    RemoteUnreachable,
    /// [`EngineError::Cancelled`]: cancelled with [`Executor::cancel`].
    Cancelled,
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
            EngineError::NotAllowed(m) if local_changes_files(m).is_some() => {
                ErrorKind::LocalChanges
            }
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
            EngineError::AuthCancelled { .. } => ErrorKind::AuthCancelled,
            EngineError::RemoteUnreachable { .. } => ErrorKind::RemoteUnreachable,
            EngineError::Cancelled { .. } => ErrorKind::Cancelled,
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

    /// The number to put in [`Done::confirm_bytes`]: the `bytes` argument of a confirmation.
    pub fn bytes(error: &EngineError) -> Option<u64> {
        let EngineError::ConfirmRequired { reason, .. } = error else {
            return None;
        };
        reason.args().iter().find_map(|(name, arg)| match arg {
            MessageArg::Number(n) if name == "bytes" && *n >= 0.0 => Some(*n as u64),
            _ => None,
        })
    }

    /// The detail to put in [`Done::error_detail`]: the reason of a confirmation, git's stderr,
    /// the files a checkout would overwrite (one per line), git's text for a remote that
    /// rejects the credentials or cannot be reached, git's text of a rejected push.
    pub fn detail(error: &EngineError) -> Option<String> {
        match error {
            EngineError::ConfirmRequired { reason, .. }
            | EngineError::GitWithReason { reason, .. } => Some(reason.text()),
            EngineError::Git { stderr, .. } => Some(stderr.clone()),
            EngineError::Auth { message, .. }
            | EngineError::RemoteUnreachable { message, .. }
            | EngineError::PushRejected { message, .. } => Some(message.clone()),
            EngineError::AuthCancelled { remote } => Some(remote.clone()),
            EngineError::CheckoutConflict { files, .. } => Some(files.join("\n")),
            EngineError::NotAllowed(m) => local_changes_files(m),
            _ => None,
        }
    }
}

/// The refusals for uncommitted local changes, which the UI explains with its own text.
const LOCAL_CHANGES_KEYS: [&str; 4] = [
    "engine-merge-local-changes",
    "engine-pick-local-changes",
    "engine-pull-local-changes",
    "engine-rebase-local-changes",
];

/// The files of a refusal for local changes (`a.rs, b.rs`, without the engine's backticks),
/// `None` for other messages.
fn local_changes_files(message: &Message) -> Option<String> {
    if !LOCAL_CHANGES_KEYS.contains(&message.key()) {
        return None;
    }
    message.args().iter().find_map(|(name, arg)| match arg {
        MessageArg::Text(files) if name == "files" => Some(files.replace('`', "")),
        _ => None,
    })
}

/// A command error, as it comes from the job.
struct Failure {
    kind: ErrorKind,
    detail: Option<String>,
    message: String,
    expected: Option<String>,
    bytes: Option<u64>,
}

impl Failure {
    fn of(error: &EngineError) -> Self {
        Self {
            kind: ErrorKind::of(error),
            detail: ErrorKind::detail(error),
            // In the UI language; `Display` is the English of CLI and MCP.
            message: error.text(),
            expected: ErrorKind::expected(error),
            bytes: ErrorKind::bytes(error),
        }
    }
}

/// Called when a result arrives, from any thread: in the UI it requests a new frame.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// The latest progress of every command in progress (`None` until one arrives). The job's
/// sink only updates the tickets present: one already collected does not come back.
type ProgressMap = Arc<Mutex<HashMap<Ticket, Option<Progress>>>>;

/// What [`Executor::submit_prepared`] made of a command's result, off the UI thread.
pub type Prepared = Box<dyn Any + Send>;

/// Turns a command's result into what the UI shows (see [`Executor::submit_prepared`]). It may
/// run more than once: a read overtaken by a write is read again.
type Prepare = Box<dyn Fn(Value) -> Prepared + Send>;

/// The session of the reads, and how many commands changed the repo so far.
///
/// A read takes both when it starts and, when it has its answer, sends it only if `changes` is
/// still the same, checking and sending under this lock; otherwise it reads again. A command
/// that writes or opens a repo replaces `session` and adds to `changes` before it sends its own
/// answer. So what the UI receives after the answer of a write was read after that write: the
/// state the write's answer brings is never overwritten by an older one.
#[derive(Default)]
struct Reading {
    /// [`Session::reader`] of the repo open in the session; `None` with no repo open (the reads
    /// then run on the session itself). A read keeps the one it started with.
    session: Option<Arc<Mutex<Session>>>,
    changes: u64,
    /// [`Session::opens`]: an answer about a repo that is no longer the open one is not read
    /// again (the UI drops it).
    opens: u64,
}

impl Reading {
    fn of(session: &Session) -> Option<Arc<Mutex<Session>>> {
        session.reader().ok().map(|s| Arc::new(Mutex::new(s)))
    }
}

/// Around a command on the session: when it ends, also with a panic, a command that writes or
/// opens leaves the reads a new reader and counts as a change.
struct Changing<'a> {
    reading: &'a Mutex<Reading>,
    session: &'a mut Session,
    writes: bool,
}

impl Drop for Changing<'_> {
    fn drop(&mut self) {
        let mut reading = lock(self.reading);
        if self.writes || self.session.opens() != reading.opens {
            reading.session = Reading::of(self.session);
            reading.opens = self.session.opens();
            reading.changes += 1;
        }
    }
}

/// Sends commands to the shared session and collects their results.
pub struct Executor {
    session: Arc<Mutex<Session>>,
    reading: Arc<Mutex<Reading>>,
    registry: Arc<Registry>,
    platform: Arc<dyn Platform>,
    wake: Wake,
    sender: Sender<(Done, Option<Prepared>)>,
    receiver: Receiver<(Done, Option<Prepared>)>,
    next: u64,
    running: Vec<Job>,
    progress: ProgressMap,
    cancel: HashMap<Ticket, CancelToken>,
    /// What the commands collected by the last [`Executor::poll`] prepared.
    prepared: HashMap<Ticket, Prepared>,
}

impl Executor {
    pub fn new(
        session: Session,
        registry: Registry,
        platform: Arc<dyn Platform>,
        wake: Wake,
    ) -> Self {
        let (sender, receiver) = channel();
        let reading = Reading {
            session: Reading::of(&session),
            changes: 0,
            opens: session.opens(),
        };
        Self {
            reading: Arc::new(Mutex::new(reading)),
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
            prepared: HashMap::new(),
        }
    }

    /// Sends the command `command` with `params` and returns at once (after running it, if the
    /// platform runs jobs inside `spawn`). The result arrives from [`Executor::poll`].
    pub fn submit(&mut self, command: &str, params: Value) -> Ticket {
        self.submit_with(command, params, true)
    }

    /// Like [`Executor::submit`]; with `asks` false git's credential questions get no dialog
    /// (the automatic fetch): a missing credential is an authentication error.
    pub fn submit_with(&mut self, command: &str, params: Value, asks: bool) -> Ticket {
        self.start(command, params, asks, None)
    }

    /// Like [`Executor::submit`], and the job goes on to turn the result into what the UI shows
    /// with `prepare`, off the UI thread and with the session already released: decoding a large
    /// result and building its view would freeze the window. The [`Done`] of the command then
    /// carries no value (`Ok(Value::Null)`): what `prepare` returned is collected with
    /// [`Executor::take_prepared`]. A failed command prepares nothing.
    pub fn submit_prepared<T: Send + 'static>(
        &mut self,
        command: &str,
        params: Value,
        prepare: impl Fn(Value) -> T + Send + 'static,
    ) -> Ticket {
        let prepare: Prepare = Box::new(move |value| Box::new(prepare(value)));
        self.start(command, params, true, Some(prepare))
    }

    /// What the command `ticket`, sent with [`Executor::submit_prepared`], prepared. To be taken
    /// while handling its [`Done`]: the next [`Executor::poll`] drops what nobody took (the
    /// answers that arrive too late to be shown).
    pub fn take_prepared<T: 'static>(&mut self, ticket: Ticket) -> Option<T> {
        let prepared = self.prepared.remove(&ticket)?;
        prepared.downcast().ok().map(|boxed| *boxed)
    }

    fn start(
        &mut self,
        command: &str,
        params: Value,
        asks: bool,
        prepare: Option<Prepare>,
    ) -> Ticket {
        let ticket = Ticket(self.next);
        self.next += 1;
        let spec = self.registry.get(command);
        let writes = spec.is_some_and(|spec| spec.writes);
        let reads = spec.is_some_and(|spec| spec.reader);
        log::debug!("submit {command}");
        self.running.push(Job {
            ticket,
            command: command.to_owned(),
            writes,
            started: std::time::Instant::now(),
            asks,
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
        let reading = Arc::clone(&self.reading);
        let registry = Arc::clone(&self.registry);
        let reply = Reply {
            ticket,
            command: command.to_owned(),
            sender: self.sender.clone(),
            wake: Arc::clone(&self.wake),
            sent: false,
        };
        self.platform.spawn(Box::new(move || {
            let _job = crate::credentials::Job::start(asks);
            // With no session locked.
            let answer = |result: Result<Value, EngineError>| match (result, &prepare) {
                (Ok(value), Some(prepare)) => (Ok(Value::Null), Some(prepare(value))),
                (result, _) => (result.map_err(|e| Failure::of(&e)), None),
            };
            if reads {
                loop {
                    let (reader, seen, repo) = {
                        let reading = lock(&reading);
                        (reading.session.clone(), reading.changes, reading.opens)
                    };
                    // No repo open: nothing to read beside the session.
                    let Some(reader) = reader else { break };
                    let result = registry.run_with(
                        &reply.command,
                        &mut lock(&reader),
                        params.clone(),
                        job.clone(),
                    );
                    // git failed: perhaps on what a write in progress is changing (a file or
                    // a ref gone for a moment). Asked again on the session, after that write.
                    if matches!(
                        result,
                        Err(EngineError::Git { .. } | EngineError::GitWithReason { .. })
                    ) {
                        break;
                    }
                    let (result, prepared) = answer(result);
                    let reading = lock(&reading);
                    if reading.changes == seen || reading.opens != repo {
                        reply.send(result, prepared);
                        return;
                    }
                    // A write ended meanwhile and its answer is on its way: this one may be older.
                }
            }
            let mut session = lock(&session);
            let result = {
                let changing = Changing {
                    reading: &reading,
                    session: &mut session,
                    writes,
                };
                registry.run_with(&reply.command, &mut *changing.session, params, job)
            };
            drop(session);
            let (result, prepared) = answer(result);
            reply.send(result, prepared);
        }));
        ticket
    }

    /// The commands finished since the last call, in the order they finished. Does not block.
    pub fn poll(&mut self) -> Vec<Done> {
        self.prepared.clear();
        let mut done = Vec::new();
        for (finished, prepared) in self.receiver.try_iter() {
            if let Some(prepared) = prepared {
                self.prepared.insert(finished.ticket, prepared);
            }
            done.push(finished);
        }
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

/// A job that ended badly poisons the lock, but what it holds stays valid: every command leaves
/// the session consistent even when it fails.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sends a job's result. If the job ends without sending it (panic in the command, or the
/// platform failing to start it and discarding it) it sends an error, so the command does not
/// stay "in progress" forever.
struct Reply {
    ticket: Ticket,
    command: String,
    sender: Sender<(Done, Option<Prepared>)>,
    wake: Wake,
    sent: bool,
}

impl Reply {
    fn send(mut self, result: Result<Value, Failure>, prepared: Option<Prepared>) {
        self.deliver(result, prepared);
    }

    fn deliver(&mut self, result: Result<Value, Failure>, prepared: Option<Prepared>) {
        self.sent = true;
        let (error_kind, error_detail, confirm_expected, confirm_bytes) = match &result {
            Ok(_) => (None, None, None, None),
            Err(f) => (Some(f.kind), f.detail.clone(), f.expected.clone(), f.bytes),
        };
        // If the UI is already closed nobody is waiting for the result.
        let done = Done {
            ticket: self.ticket,
            command: self.command.clone(),
            result: result.map_err(|f| f.message),
            error_kind,
            error_detail,
            confirm_expected,
            confirm_bytes,
        };
        let _ = self.sender.send((done, prepared));
        (self.wake)();
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if !self.sent {
            let failure = Failure {
                kind: ErrorKind::Other,
                detail: None,
                message: tr!("app-no-result", command = &self.command),
                expected: None,
                bytes: None,
            };
            self.deliver(Err(failure), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{
        Access, Capabilities, EngineError, MemoryOpener, Platform, PullOperation,
    };
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

    /// A write that changes the repo and then panics.
    fn commit_then_panic(s: &mut Session, _: Value) -> Result<Value, EngineError> {
        s.repo_mut()?
            .commit(&project_engine::CommitRequest::new("before the panic"))?;
        panic!("fake write that panics");
    }

    /// A read on which git fails beside the session, as on a file a write is replacing.
    fn fails_on_the_reader(s: &mut Session, _: Value) -> Result<Value, EngineError> {
        if s.is_reader() {
            return Err(EngineError::Git {
                command: "diff".into(),
                stderr: "fatal: stat 'a.txt': No such file or directory".into(),
            });
        }
        Ok(json!({ "read": "on the session" }))
    }

    /// How many times [`head_then_slow`] ran.
    static HEAD_READS: AtomicUsize = AtomicUsize::new(0);

    /// A read that takes a while after looking: HEAD as it was [`SLOW`] ago.
    fn head_then_slow(s: &mut Session, _: Value) -> Result<Value, EngineError> {
        let head = s.repo()?.head()?.target().map(|id| id.to_string());
        HEAD_READS.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(SLOW);
        Ok(json!({ "head": head }))
    }

    /// The real catalog plus `test.slow` (sleeps [`SLOW`]), `test.panic` and `test.write` (a
    /// write that sleeps [`SLOW`]).
    fn registry() -> Registry {
        let mut registry = Registry::builtin();
        registry.add("test.slow", "test", "{}", Access::Read, slow);
        registry.add("test.panic", "test", "{}", Access::Read, panics);
        registry.add("test.cancel", "test", "{}", Access::Read, until_cancelled);
        registry.add("test.write", "test", "{}", Access::Write, slow_write);
        registry.add("test.head", "test", "{}", Access::Read, head_then_slow);
        registry.add(
            "test.flaky",
            "test",
            "{}",
            Access::Read,
            fails_on_the_reader,
        );
        registry.add("test.wreck", "test", "{}", Access::Write, commit_then_panic);
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
        // With no repo open the read runs on the session, and its lock queues the commands.
        let done = wait(&mut ex, 2);
        let status = ex.submit("repo.status", json!({}));
        let after = wait(&mut ex, 1);
        assert!(done.iter().any(|d| d.ticket == open && d.result.is_ok()));
        assert_eq!(after[0].ticket, status);
        assert!(after[0].result.is_ok(), "{:?}", after[0].result);
    }

    fn opened(platform: TestPlatform) -> Executor {
        let (mut ex, _) = executor(platform);
        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        assert!(wait(&mut ex, 1)[0].result.is_ok());
        ex
    }

    #[test]
    fn a_read_does_not_wait_for_a_write_in_progress() {
        let mut ex = opened(TestPlatform::Threads);
        let start = Instant::now();
        let write = ex.submit("test.write", json!({}));
        let status = ex.submit("repo.status", json!({}));
        let log = ex.submit("repo.log", json!({ "limit": 3 }));

        let reads = wait(&mut ex, 2);
        assert!(start.elapsed() < SLOW / 2, "the reads waited for the write");
        assert!(reads.iter().all(|d| d.result.is_ok()), "{reads:?}");
        assert!(reads.iter().any(|d| d.ticket == status));
        assert!(reads.iter().any(|d| d.ticket == log));
        assert_eq!(ex.writing().map(|job| job.ticket), Some(write));

        // What reads the session itself (the journal) still waits for it.
        let history = ex.submit("edit.history", json!({}));
        let rest = wait(&mut ex, 2);
        assert!(start.elapsed() >= SLOW);
        assert!(rest.iter().any(|d| d.ticket == history && d.result.is_ok()));
    }

    #[test]
    fn a_read_overtaken_by_a_write_is_read_again_and_answers_after_it() {
        let mut ex = opened(TestPlatform::Threads);
        let head = |done: &Done| done.result.clone().unwrap()["head"].clone();
        ex.submit("test.head", json!({}));
        let before = head(&wait(&mut ex, 1)[0]);
        let ran = HEAD_READS.load(Ordering::SeqCst);

        // The read looks at HEAD, then a commit moves it and answers while the read still works.
        let read = ex.submit("test.head", json!({}));
        let start = Instant::now();
        while HEAD_READS.load(Ordering::SeqCst) == ran {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "the read did not start"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        let commit = ex.submit("commit.create", json!({ "message": "x" }));
        let done = wait(&mut ex, 2);
        assert_eq!(
            done[0].ticket, commit,
            "the answer of the write comes first"
        );
        let created = done[0].result.clone().unwrap()["commit"]["id"].clone();
        assert_eq!(done[1].ticket, read);
        // Not the HEAD it saw first: that answer would have put the old state back in the view.
        assert_eq!(head(&done[1]), created);
        assert_ne!(head(&done[1]), before);
        assert_eq!(HEAD_READS.load(Ordering::SeqCst), ran + 2);
    }

    #[test]
    fn a_read_git_fails_beside_a_write_is_asked_again_after_it() {
        let mut ex = opened(TestPlatform::Threads);
        let start = Instant::now();
        let write = ex.submit("test.write", json!({}));
        let read = ex.submit("test.flaky", json!({}));
        let done = wait(&mut ex, 2);
        assert!(start.elapsed() >= SLOW);
        assert_eq!(done[0].ticket, write);
        assert_eq!(done[1].ticket, read);
        assert_eq!(done[1].result, Ok(json!({ "read": "on the session" })));
    }

    #[test]
    fn an_answer_about_a_repo_no_longer_open_is_not_read_again() {
        let mut ex = opened(TestPlatform::Threads);
        let start = Instant::now();
        let read = ex.submit("test.slow", json!({}));
        let open = ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        let done = wait(&mut ex, 2);
        assert_eq!(done[0].ticket, open);
        assert_eq!(done[1].ticket, read);
        assert!(done[1].result.is_ok());
        assert!(start.elapsed() < SLOW * 2, "the read ran twice");
    }

    #[test]
    fn a_write_that_panics_still_leaves_the_reads_what_it_wrote() {
        let mut ex = opened(TestPlatform::Threads);
        let wreck = ex.submit("test.wreck", json!({}));
        let done = wait(&mut ex, 1);
        assert_eq!(done[0].ticket, wreck);
        assert!(done[0].result.is_err());

        ex.submit("repo.log", json!({ "limit": 1 }));
        let log = wait(&mut ex, 1)[0].result.clone().unwrap();
        assert_eq!(log["commits"][0]["summary"], "before the panic");
    }

    #[test]
    fn the_reads_follow_the_repo_the_session_opens() {
        let (mut ex, _) = executor(TestPlatform::Inline);
        // No repo: the read answers as the session does.
        ex.submit("repo.status", json!({}));
        let done = ex.poll();
        assert!(
            done[0]
                .result
                .as_ref()
                .is_err_and(|e| e.contains("repo.open"))
        );

        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        ex.submit("repo.refs", json!({}));
        let refs = ex.poll()[1].result.clone().unwrap();

        // What a write changed is there for the next read.
        ex.submit("branch.create", json!({ "name": "beside" }));
        ex.submit("repo.refs", json!({}));
        let after = ex.poll()[1].result.clone().unwrap();
        assert_ne!(after, refs);
        assert!(after.to_string().contains("refs/heads/beside"));

        // A failed open leaves the repo open, and the reads on it.
        ex.submit("repo.open", json!({ "path": "elsewhere" }));
        ex.submit("repo.refs", json!({}));
        let done = ex.poll();
        assert!(done[0].result.is_err());
        assert_eq!(done[1].result.clone().unwrap(), after);
    }

    #[test]
    fn a_result_is_prepared_in_the_job_off_the_callers_thread() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        let caller = std::thread::current().id();
        let ticket = ex.submit_prepared("test.slow", json!({ "n": 7 }), move |value| {
            (value["echo"]["n"].as_u64(), std::thread::current().id())
        });
        assert_eq!(ex.take_prepared::<()>(ticket), None, "not done yet");
        let done = wait(&mut ex, 1);
        // The value went to `prepare`: the UI thread has nothing left to decode.
        assert_eq!(done[0].result, Ok(Value::Null));
        // Asked with another type: nothing, and what was prepared is gone with it.
        let (n, thread) = ex
            .take_prepared::<(Option<u64>, std::thread::ThreadId)>(ticket)
            .expect("prepared");
        assert_eq!(n, Some(7));
        assert_ne!(thread, caller);
        assert_eq!(
            ex.take_prepared::<(Option<u64>, std::thread::ThreadId)>(ticket),
            None
        );
    }

    #[test]
    fn a_prepared_result_nobody_takes_is_dropped_and_a_failure_prepares_nothing() {
        let (mut ex, _) = executor(TestPlatform::Inline);
        let dropped = Arc::new(());
        let held = Arc::clone(&dropped);
        let open = json!({ "path": MemoryOpener::SAMPLE_PATH });
        let ticket = ex.submit_prepared("repo.open", open, move |_| Arc::clone(&held));
        assert_eq!(ex.poll().len(), 1);
        assert_eq!(Arc::strong_count(&dropped), 2, "kept until the next poll");
        assert!(ex.poll().is_empty());
        assert_eq!(Arc::strong_count(&dropped), 1);
        assert_eq!(ex.take_prepared::<Arc<()>>(ticket), None);

        // A failed command: its error as usual, `prepare` is not called.
        let ticket = ex.submit_prepared("test.missing", json!({}), |_| -> u8 {
            panic!("nothing to prepare")
        });
        let done = ex.poll();
        assert!(done[0].result.is_err());
        assert_eq!(ex.take_prepared::<u8>(ticket), None);

        // A `prepare` that panics (its thread ends) is a command without a result, not one in
        // progress forever.
        let (mut ex, _) = executor(TestPlatform::Threads);
        let ticket = ex.submit_prepared("test.slow", json!({}), |_| -> u8 { panic!("boom") });
        let done = wait(&mut ex, 1);
        assert_eq!(done[0].ticket, ticket);
        assert!(done[0].result.is_err());
        assert_eq!(ex.take_prepared::<u8>(ticket), None);
        assert!(!ex.is_busy());
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
        let cancelled = EngineError::AuthCancelled {
            remote: "origin".into(),
        };
        assert_eq!(ErrorKind::of(&cancelled), ErrorKind::AuthCancelled);
        assert_eq!(ErrorKind::detail(&cancelled).as_deref(), Some("origin"));
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
        // No backend produces it since S32: it has no kind of its own, the engine's text (which
        // names the files) goes in the generic failure dialog.
        assert_eq!(ErrorKind::of(&conflict), ErrorKind::Other);
        assert_eq!(ErrorKind::detail(&conflict), None);
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
