//! Credential prompts (S52): git and ssh ask for a user name, password, passphrase or host key
//! confirmation during fetch, pull, push and clone, and the app asks the user.
//!
//! [`CredentialDialog`] is the engine side: installed on the `CliOpener` by `apps/project`, its
//! `ask` runs on the worker thread of the command, sends the question to the UI with a one-shot
//! reply channel, wakes the UI and blocks until the answer. [`CredentialBox`] is the UI side, held
//! by the `App`: one question at a time in a modal, the others wait in the channel. Data only, no
//! egui drawing: the modal is `view::credential_dialog`.
//!
//! The answer is a secret: it lives only in [`Prompt`] while the modal is open, is never put in
//! an `Action`, a log or `Debug` output, and is overwritten before being dropped when the modal
//! closes without sending it.

use project_engine::{CredentialKind, CredentialPrompter, CredentialRequest};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Mutex, MutexGuard};

/// The actions of the modal, applied by the `App`. The typed text is not here: the field edits
/// [`Prompt::answer`] in place, so the secret never travels in an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// "OK" (or Enter), "Trust and continue" for a host key: sends the answer.
    Answer,
    /// "Cancel" (or Escape): git gets no answer and the command fails with `AuthCancelled`.
    Cancel,
    /// Shows or hides the secret typed in the field.
    ToggleShown,
}

/// A question on its way to the UI, with the channel for the answer (`None` = cancelled).
struct Question {
    request: CredentialRequest,
    reply: Sender<Option<String>>,
    /// The commands that may have asked (see [`Job`]).
    owners: Vec<u64>,
}

/// The desktop app's [`CredentialPrompter`], installed on the `CliOpener` by `apps/project`.
///
/// Built before the window exists: [`CredentialDialog::connect`] hands it the `egui::Context`
/// (to wake the UI) and gives the UI the receiving end of the questions. Until then, or once the
/// UI is gone, every question is cancelled.
pub struct CredentialDialog {
    ctx: Mutex<Option<egui::Context>>,
    sender: Sender<Question>,
    /// The receiving end, until [`CredentialDialog::connect`] takes it.
    receiver: Mutex<Option<Receiver<Question>>>,
}

impl std::fmt::Debug for CredentialDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialDialog")
            .field("connected", &lock(&self.ctx).is_some())
            .finish_non_exhaustive()
    }
}

impl Default for CredentialDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl CredentialDialog {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            ctx: Mutex::new(None),
            sender,
            receiver: Mutex::new(Some(receiver)),
        }
    }

    /// Connects the UI: `ctx` is woken at every question. The first call gets the questions,
    /// later ones `None`.
    pub fn connect(&self, ctx: &egui::Context) -> Option<CredentialBox> {
        let receiver = lock(&self.receiver).take()?;
        *lock(&self.ctx) = Some(ctx.clone());
        Some(CredentialBox {
            queue: Some(receiver),
            open: None,
        })
    }
}

impl CredentialPrompter for CredentialDialog {
    fn ask(&self, request: CredentialRequest) -> Option<String> {
        let ctx = lock(&self.ctx).clone()?;
        let (reply, answer) = mpsc::channel();
        // Fails once the UI (the receiver) is gone.
        // `ask` runs on the bridge's thread, not the command's: the owner is among the commands
        // that started a bridge and are still running.
        let owners = lock(&ASKING).iter().copied().collect();
        self.sender
            .send(Question {
                request,
                reply,
                owners,
            })
            .ok()?;
        ctx.request_repaint();
        // Fails if the UI drops the question without answering (closed, or the app exits).
        answer.recv().ok().flatten()
    }

    fn asks_now(&self) -> bool {
        let (id, quiet) = JOB.get();
        if quiet {
            return false;
        }
        if let Some(id) = id {
            lock(&ASKING).insert(id);
        }
        true
    }
}

thread_local! {
    /// The command running on this thread (see [`Job`]): its id and whether it must not ask.
    static JOB: std::cell::Cell<(Option<u64>, bool)> = const { std::cell::Cell::new((None, false)) };
}

/// The commands that started a credential bridge and are still running.
static ASKING: Mutex<BTreeSet<u64>> = Mutex::new(BTreeSet::new());

/// The id of the next [`Job`].
static NEXT_JOB: AtomicU64 = AtomicU64::new(1);

/// Held by the executor on the thread of every command, for its whole run: with `asks` false the
/// command gets no credential dialog ([`CredentialPrompter::asks_now`] is false: the automatic
/// fetch); otherwise a question it asks can be told apart from a stale one after it ends.
pub(crate) struct Job {
    id: u64,
    previous: (Option<u64>, bool),
}

impl Job {
    pub(crate) fn start(asks: bool) -> Self {
        let id = NEXT_JOB.fetch_add(1, Ordering::Relaxed);
        Self {
            id,
            previous: JOB.replace((Some(id), !asks)),
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        lock(&ASKING).remove(&self.id);
        JOB.set(self.previous);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// The question shown in the modal.
pub(crate) struct Prompt {
    pub request: CredentialRequest,
    /// What the user typed: a secret for [`CredentialKind::is_secret`].
    pub answer: String,
    /// The secret is shown in clear ("Show").
    pub shown: bool,
    /// The field has not received the focus yet.
    pub focus: bool,
    reply: Sender<Option<String>>,
    /// The commands that may have asked; empty: unknown (asked outside the executor).
    owners: Vec<u64>,
}

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Prompt")
            .field("request", &self.request)
            .field("shown", &self.shown)
            .finish_non_exhaustive()
    }
}

impl Prompt {
    /// The answer to send: `yes` for a host key ("Trust and continue"), the field otherwise.
    fn take_answer(&mut self) -> String {
        if self.request.kind == CredentialKind::HostKey {
            wipe(&mut self.answer);
            "yes".to_owned()
        } else {
            std::mem::take(&mut self.answer)
        }
    }
}

impl Drop for Prompt {
    fn drop(&mut self) {
        wipe(&mut self.answer);
    }
}

/// Overwrites the text with zeros, then drops it.
fn wipe(text: &mut String) {
    let mut bytes = std::mem::take(text).into_bytes();
    bytes.fill(0);
    // Keeps the compiler from dropping the writes as dead stores.
    std::hint::black_box(&bytes);
}

/// The UI side: the questions waiting and the one in the modal.
#[derive(Debug, Default)]
pub struct CredentialBox {
    queue: Option<Receiver<Question>>,
    open: Option<Prompt>,
}

impl std::fmt::Debug for Question {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Question")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

impl CredentialBox {
    /// Opens the next waiting question if none is open. Returns true if one opened.
    pub(crate) fn receive(&mut self) -> bool {
        if self.open.is_some() {
            return false;
        }
        let Some(queue) = &self.queue else {
            return false;
        };
        match queue.try_recv() {
            Ok(Question {
                request,
                reply,
                owners,
            }) => {
                self.open = Some(Prompt {
                    request,
                    answer: String::new(),
                    shown: false,
                    focus: true,
                    reply,
                    owners,
                });
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.queue = None;
                false
            }
        }
    }

    /// The question in the modal is stale: every command that may have asked it has ended (git
    /// was stopped while it asked). Never for a question of unknown origin.
    pub(crate) fn stale(&self) -> bool {
        self.open.as_ref().is_some_and(|prompt| {
            let asking = lock(&ASKING);
            !prompt.owners.is_empty() && prompt.owners.iter().all(|id| !asking.contains(id))
        })
    }

    /// The question in the modal, if any.
    pub(crate) fn open(&self) -> Option<&Prompt> {
        self.open.as_ref()
    }

    /// The question in the modal, for the field to edit its answer.
    pub(crate) fn open_mut(&mut self) -> Option<&mut Prompt> {
        self.open.as_mut()
    }

    /// Applies an action of the modal. Returns true if the modal closed (the caller drops the
    /// field's egui state, which may hold copies of the text).
    pub(crate) fn apply(&mut self, action: Action) -> bool {
        match action {
            Action::ToggleShown => {
                if let Some(prompt) = &mut self.open {
                    prompt.shown = !prompt.shown;
                }
                false
            }
            Action::Answer => {
                let Some(mut prompt) = self.open.take() else {
                    return false;
                };
                let answer = prompt.take_answer();
                // The command may have ended meanwhile: nobody waits, nothing to do.
                let _ = prompt.reply.send(Some(answer));
                true
            }
            Action::Cancel => self.cancel(),
        }
    }

    /// Closes the modal with no answer (the job it came from is no longer running). Returns true
    /// if one was open.
    pub(crate) fn cancel(&mut self) -> bool {
        let Some(prompt) = self.open.take() else {
            return false;
        };
        let _ = prompt.reply.send(None);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::{Duration, Instant};

    fn request(prompt: &str) -> CredentialRequest {
        CredentialRequest::new(prompt, Some("origin".to_owned()))
    }

    /// Waits until the question asked on another thread reaches the box.
    fn wait_open(boxed: &mut CredentialBox) {
        let start = Instant::now();
        while !boxed.receive() {
            assert!(start.elapsed() < Duration::from_secs(5), "no question");
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn the_answer_goes_back_to_the_asking_thread() {
        let dialog = std::sync::Arc::new(CredentialDialog::new());
        let mut boxed = dialog.connect(&egui::Context::default()).unwrap();
        assert!(dialog.connect(&egui::Context::default()).is_none());
        let asker =
            { thread::spawn(move || dialog.ask(request("Password for 'https://ada@host': "))) };
        wait_open(&mut boxed);
        let prompt = boxed.open_mut().unwrap();
        assert_eq!(prompt.request.kind, CredentialKind::Password);
        assert!(prompt.focus);
        prompt.answer.push_str("s3cret");
        assert!(!format!("{boxed:?}").contains("s3cret"));
        assert!(boxed.apply(Action::Answer));
        assert!(boxed.open().is_none());
        assert_eq!(asker.join().unwrap().as_deref(), Some("s3cret"));
    }

    #[test]
    fn cancel_answers_none() {
        let dialog = std::sync::Arc::new(CredentialDialog::new());
        let mut boxed = dialog.connect(&egui::Context::default()).unwrap();
        let asker = { thread::spawn(move || dialog.ask(request("Username for 'https://host': "))) };
        wait_open(&mut boxed);
        boxed.open_mut().unwrap().answer.push_str("ada");
        assert!(boxed.apply(Action::Cancel));
        assert_eq!(asker.join().unwrap(), None);
    }

    #[test]
    fn host_key_trust_answers_yes() {
        let dialog = std::sync::Arc::new(CredentialDialog::new());
        let mut boxed = dialog.connect(&egui::Context::default()).unwrap();
        let asker = {
            thread::spawn(move || {
                dialog.ask(request(
                    "Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
                ))
            })
        };
        wait_open(&mut boxed);
        assert_eq!(boxed.open().unwrap().request.kind, CredentialKind::HostKey);
        boxed.apply(Action::Answer);
        assert_eq!(asker.join().unwrap().as_deref(), Some("yes"));
    }

    #[test]
    fn questions_wait_their_turn() {
        let dialog = std::sync::Arc::new(CredentialDialog::new());
        let mut boxed = dialog.connect(&egui::Context::default()).unwrap();
        let askers: Vec<_> = ["Username for 'https://a': ", "Username for 'https://b': "]
            .into_iter()
            .map(|p| {
                let dialog = dialog.clone();
                thread::spawn(move || dialog.ask(request(p)))
            })
            .collect();
        wait_open(&mut boxed);
        // The second stays in the channel while the first is open.
        thread::sleep(Duration::from_millis(20));
        assert!(!boxed.receive());
        boxed.open_mut().unwrap().answer.push_str("one");
        boxed.apply(Action::Answer);
        wait_open(&mut boxed);
        boxed.open_mut().unwrap().answer.push_str("two");
        boxed.apply(Action::Answer);
        let mut answers: Vec<_> = askers.into_iter().map(|a| a.join().unwrap()).collect();
        answers.sort();
        assert_eq!(answers, [Some("one".to_owned()), Some("two".to_owned())]);
    }

    #[test]
    fn no_ui_or_closed_ui_answers_none() {
        let dialog = CredentialDialog::new();
        // Not connected yet.
        assert_eq!(dialog.ask(request("Password: ")), None);
        let boxed = dialog.connect(&egui::Context::default()).unwrap();
        drop(boxed);
        assert_eq!(dialog.ask(request("Password: ")), None);
    }

    #[test]
    fn a_question_dropped_unanswered_answers_none() {
        let dialog = std::sync::Arc::new(CredentialDialog::new());
        let mut boxed = dialog.connect(&egui::Context::default()).unwrap();
        let asker = { thread::spawn(move || dialog.ask(request("Password: "))) };
        wait_open(&mut boxed);
        drop(boxed);
        assert_eq!(asker.join().unwrap(), None);
    }

    #[test]
    fn quiet_commands_do_not_ask() {
        let dialog = CredentialDialog::new();
        assert!(dialog.asks_now());
        {
            let _quiet = Job::start(false);
            assert!(!dialog.asks_now());
            let _asks = Job::start(true);
            assert!(dialog.asks_now());
        }
        assert!(dialog.asks_now());
    }

    #[test]
    fn a_question_is_stale_once_its_command_ended() {
        let dialog = std::sync::Arc::new(CredentialDialog::new());
        let mut boxed = dialog.connect(&egui::Context::default()).unwrap();
        let (go, wait) = mpsc::channel::<()>();
        // The command's thread starts the bridge, whose own thread asks.
        let command = thread::spawn(move || {
            let job = Job::start(true);
            assert!(dialog.asks_now());
            let asker = thread::spawn(move || dialog.ask(request("Password: ")));
            wait.recv().unwrap();
            drop(job);
            asker
        });
        wait_open(&mut boxed);
        assert!(!boxed.stale());
        go.send(()).unwrap();
        let asker = command.join().unwrap();
        assert!(boxed.stale());
        assert!(boxed.cancel());
        assert_eq!(asker.join().unwrap(), None);
    }

    #[test]
    fn toggle_and_wipe() {
        let mut text = String::from("s3cret");
        wipe(&mut text);
        assert!(text.is_empty());
        let (reply, answer) = mpsc::channel();
        let mut boxed = CredentialBox {
            queue: None,
            open: Some(Prompt {
                request: request("Enter passphrase for key '/k': "),
                answer: "s3cret".to_owned(),
                shown: false,
                focus: false,
                reply,
                owners: Vec::new(),
            }),
        };
        assert!(!boxed.stale(), "unknown origin: never stale");
        assert!(!boxed.apply(Action::ToggleShown));
        assert!(boxed.open().unwrap().shown);
        assert!(boxed.cancel());
        assert!(boxed.open().is_none());
        assert_eq!(answer.recv().unwrap(), None);
        assert!(!boxed.cancel());
    }
}
