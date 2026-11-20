//! Pull and push (S26): `remote.pull` and `remote.push` in the background, with progress and
//! "Stop" like the fetch. Data only, no egui: tested with unit tests.
//!
//! One network operation at a time: fetch, pull and push are writes
//! ([`CommandSpec::writes`](project_engine::CommandSpec)), and the UI does not send one while
//! another is in progress. What needs explaining:
//!
//! - pull whose merge or rebase stopped on a conflict: not an error, the operation **stays in
//!   progress** as with `merge.start` (`in_progress` in the result; the rebase since S32). The
//!   re-read status makes the in-progress operation panel appear (Continue/Abort merge or rebase)
//!   and the stage area opens with the files;
//! - rejected push ([`ErrorKind::PushRejected`]): a situation that lasts, so a notice in the
//!   tab ([`SyncBox::rejected`]) and not a dialog: non-fast-forward proposes "Pull", stale lease
//!   "Fetch"; "Details…" opens the modal [`Window::Details`] with git's text (the hook output)
//!   and, as a second choice for a non-fast-forward, "Force (with lease)…". The notice blocks
//!   nothing and goes away by itself: see [`SyncBox::follow`];
//! - every other failure or refusal (credentials, network, local changes, detached HEAD, a git
//!   error): the modal failure dialog ([`Window::Failure`], shown one at a time with those of
//!   the other areas, see [`crate::failure`]). A pull aborted by the engine on a conflict
//!   (`EngineError::PullConflict`, which no backend produces since S32) is one of these.
//!
//! The force never starts from a bar button: only from the details dialog, first with
//! `"force": true` and without `confirm`. If the remote would lose commits the engine answers
//! `ConfirmRequired` with the commits, and the confirmation opens; `"confirm": true` only after the click.

use crate::ErrorKind;
use crate::discard::History;
use crate::fetch::{self, Failure};
use crate::running::Running;
use crate::stash_retry::{Retry, Route};
use crate::{Done, Tab, params};
use project_domain::remote_short;
use project_domain::{HEADS_PREFIX, Head, Oid, RefInfo, Status};
use project_engine::{
    PullApplied, PullStrategy, PushKind, PushRejection, PushRemoteSource, RemotePullParams,
    RemotePullResult, RemotePushParams, RemotePushResult, RemotePushTagParams, RemotePushTagResult,
    StrategySource, TagPushKind,
};
use project_i18n::tr;
use serde_json::{Value, json};

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Pull the given branch (`None`: HEAD's), with the given strategy (`None`:
    /// the configured one).
    Pull(Option<String>, Option<PullStrategy>),
    /// Push the given branch (`None`: HEAD's), never forced.
    Push(Option<String>),
    /// Push the tag (second value, short name) to the remote (first value), never forced.
    PushTag(String, String),
    /// "Force (with lease)…" in the details of the rejected push: `"force": true` without
    /// `confirm`.
    ForcePush,
    /// "Force the push" in the confirmation: `"force": true, "confirm": true`.
    ConfirmForcePush,
    /// "Stop" the pull or push in progress.
    Cancel,
    /// Closes the pull and push dialog (also the force confirmation, without forcing). The
    /// notice of the rejected push stays.
    Dismiss,
    /// "Details…" in the notice of the rejected push: git's text and the force.
    RejectedDetails,
    /// "×" in the notice of the rejected push (and "Fetch" for a stale lease): it goes away.
    DismissRejected,
}

/// The Registry command ids.
pub const PULL: &str = "remote.pull";
pub const PUSH: &str = "remote.push";
pub const PUSH_TAG: &str = "remote.push_tag";

/// Pull, push of a branch or push of a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Pull,
    Push,
    /// `remote.push_tag`: one tag to one remote, never forced.
    PushTag,
}

impl Op {
    pub fn command(self) -> &'static str {
        match self {
            Op::Pull => PULL,
            Op::Push => PUSH,
            Op::PushTag => PUSH_TAG,
        }
    }
}

/// What to ask for: pull or push, of which branch, with which options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub op: Op,
    /// The local branch (short name); `None` = HEAD's.
    pub branch: Option<String>,
    /// Pull only: the strategy instead of the configuration.
    pub strategy: Option<PullStrategy>,
    /// Push only: `"force": true` (always with lease in the engine).
    pub force: bool,
    /// Forced push only: `"confirm": true`, after the click in the confirmation.
    pub confirm: bool,
    /// Confirmed forced push only: the oid of the local remote-tracking branch seen in the confirmation
    /// (`"expected"`), if the engine gave it.
    pub expected: Option<Oid>,
    /// Push of a tag only: the remote and the tag (short name).
    pub tag: Option<(String, String)>,
}

impl Request {
    pub fn pull(branch: Option<String>, strategy: Option<PullStrategy>) -> Self {
        Self {
            op: Op::Pull,
            branch,
            strategy,
            force: false,
            confirm: false,
            expected: None,
            tag: None,
        }
    }

    pub fn push(branch: Option<String>) -> Self {
        Self {
            op: Op::Push,
            branch,
            strategy: None,
            force: false,
            confirm: false,
            expected: None,
            tag: None,
        }
    }

    /// Push of the tag `name` (short name) to `remote`: only `refs/tags/<name>`, never forced.
    pub fn push_tag(remote: String, name: String) -> Self {
        Self {
            op: Op::PushTag,
            tag: Some((remote, name)),
            ..Self::push(None)
        }
    }

    /// The command parameters.
    pub fn params(&self) -> Value {
        if let Some((remote, name)) = &self.tag {
            return params(RemotePushTagParams {
                remote: remote.clone(),
                name: name.clone(),
            });
        }
        match self.op {
            Op::Pull => params(RemotePullParams {
                remote: None,
                branch: self.branch.clone(),
                strategy: self.strategy,
            }),
            Op::Push | Op::PushTag => params(RemotePushParams {
                remote: None,
                branch: self.branch.clone(),
                force: self.force,
                confirm: self.confirm,
                expected: self.expected.map(|oid| oid.to_string()),
            }),
        }
    }

    /// The description for the bar: `Pull of «main»`, `Forced push of «main»`, `Pull`.
    pub fn label(&self) -> String {
        if let Some((remote, name)) = &self.tag {
            return tr!("sync-label-push-tag", tag = name, remote = remote);
        }
        match (self.op, self.force, &self.branch) {
            (Op::Pull, _, Some(branch)) => tr!("sync-label-pull-branch", branch = branch),
            (Op::Pull, _, None) => tr!("sync-label-pull"),
            (Op::Push, false, Some(branch)) => tr!("sync-label-push-branch", branch = branch),
            (Op::Push, false, None) | (Op::PushTag, ..) => tr!("sync-label-push"),
            (Op::Push, true, Some(branch)) => tr!("sync-label-force-branch", branch = branch),
            (Op::Push, true, None) => tr!("sync-label-force"),
        }
    }
}

/// A rejected push: nothing changed, neither on the remote nor locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// The push's branch (`None`: the push of a tag).
    pub branch: Option<String>,
    /// Push of a tag only: the remote and the tag (short name).
    pub tag: Option<(String, String)>,
    pub reason: PushRejection,
    /// What was rejected, for the notice: `Push of «main» rejected`.
    pub title: String,
    /// What to do, in one sentence, for the notice.
    pub summary: String,
    /// What happened and what to do, in full, for the details.
    pub explanation: String,
    /// Git's text (the hook output, the `[rejected]` line).
    pub git: Option<String>,
}

impl Rejected {
    /// True if the remote has commits the branch does not: Pull and the force are proposed.
    pub fn non_fast_forward(&self) -> bool {
        self.reason == PushRejection::NonFastForward
    }

    /// True if the remote moved since the last fetch: the fetch is proposed.
    pub fn stale(&self) -> bool {
        self.reason == PushRejection::StaleLease
    }

    /// True if the details offer "Force (with lease)…": a non-fast-forward of a branch.
    pub fn can_force(&self) -> bool {
        self.non_fast_forward() && self.branch.is_some()
    }
}

/// The rejected push shown as a notice in the tab, and what makes it go away.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Notice {
    rejected: Rejected,
    /// The current branch when the push was rejected (`None`: detached HEAD).
    on: Option<String>,
    /// The branch was seen behind its upstream since the rejection. A push can be rejected
    /// while the refs still say "not behind" (nobody fetched yet): only "behind, then no
    /// longer behind" says that the remote's commits were integrated.
    seen_behind: bool,
}

/// The short name of the current branch (`None`: detached HEAD).
fn current_branch(head: &Head) -> Option<&str> {
    match head {
        Head::Branch { name, .. } => Some(name),
        Head::Detached(_) => None,
    }
}

/// The confirmation of a forced push that would remove commits from the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmForce {
    pub branch: Option<String>,
    /// The engine's reason: the commits lost on the remote and the backup.
    pub reason: String,
    /// The oid of the local remote-tracking branch the confirmation shows: it goes back to the engine with
    /// `"confirm": true` (`"expected"`), so a different remote than the one seen is not confirmed.
    /// `None`: the engine did not give it (e.g. the local remote-tracking branch disappeared) and it cannot be
    /// confirmed.
    pub expected: Option<Oid>,
    /// True if the confirmation was sent but in the meantime the local remote-tracking branch changed:
    /// the dialog shows the new commits and must be confirmed again.
    pub changed: bool,
}

impl ConfirmForce {
    /// True if it can be confirmed: the seen oid to send back is there.
    pub fn can_confirm(&self) -> bool {
        self.expected.is_some()
    }
}

/// The dialog opened after a pull or push: all modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Window {
    /// Error to explain (credentials, network, rejection with a long message, git error): the
    /// modal failure dialog.
    Failure(Failure),
    /// "Details…" of the rejected push: the explanation, git's text and, for a
    /// non-fast-forward, "Force (with lease)…".
    Details(Rejected),
    /// The force confirmation.
    ConfirmForce(ConfirmForce),
}

/// Pull and push: the one in progress, the open dialog and the notice of a rejected push.
#[derive(Debug, Default)]
pub struct SyncBox {
    running: Option<Running<Request>>,
    window: Option<Window>,
    /// The last rejected push, while it is worth saying: one at a time, the newest.
    rejected: Option<Notice>,
}

/// What arrived from `remote.pull` or `remote.push`.
#[derive(Debug)]
pub enum Received {
    Done(Box<Outcome>),
    /// The engine asks for the force confirmation: its dialog is open.
    Confirm,
    /// Interrupted: with `reload` refs, status and log are re-read (the pull's fetch or the push
    /// may have updated something).
    Cancelled {
        notice: String,
        reload: bool,
    },
    /// Failed or rejected: the message for the status bar (the details, if needed, in the
    /// failure dialog or in the notice of the rejected push); with `reload` refs, status and
    /// log are re-read.
    Failed {
        message: String,
        reload: bool,
    },
}

/// The result of a successful pull or push, for the view.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    pub refs: Option<Vec<RefInfo>>,
    pub status: Option<Status>,
    /// The journal after the pull (Undo proposes it); `None` for the push, which is not in it.
    pub history: Option<History>,
    /// True if the active branch moved: after the log the new HEAD is selected.
    pub moved_head: bool,
    /// The pull's merge is stopped with conflicted files and stays in progress: the stage area opens
    /// with the files to resolve (the operation panel appears from the re-read status).
    pub show_changes: bool,
    /// A problem after the successful operation (refs or status not re-read).
    pub warning: Option<String>,
}

impl SyncBox {
    pub fn running(&self) -> Option<&Running<Request>> {
        self.running.as_ref()
    }

    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// True if the force confirmation or the details of a rejected push are open: while they
    /// are, no writes. A [`Window::Failure`] is modal too, counted with the other failures
    /// ([`crate::RepoView::failure`]). The notice of a rejected push is not a dialog: it blocks
    /// nothing.
    pub fn modal_open(&self) -> bool {
        matches!(
            self.window,
            Some(Window::ConfirmForce(_) | Window::Details(_))
        )
    }

    /// Closes the dialog; the notice of a rejected push stays.
    pub fn dismiss(&mut self) {
        self.window = None;
    }

    /// The failure dialog, to note HEAD in its retry or to take it ("Stash and …", U68).
    pub(crate) fn failure_mut(&mut self) -> Option<&mut Failure> {
        match &mut self.window {
            Some(Window::Failure(failure)) => Some(failure),
            _ => None,
        }
    }

    /// The rejected push to show as a notice in the tab, if any.
    pub fn rejected(&self) -> Option<&Rejected> {
        self.rejected.as_ref().map(|notice| &notice.rejected)
    }

    /// "Details…" in the notice: the modal dialog with git's text. False without a notice.
    pub fn open_details(&mut self) -> bool {
        let Some(notice) = &self.rejected else {
            return false;
        };
        self.window = Some(Window::Details(notice.rejected.clone()));
        true
    }

    /// "×" in the notice: it goes away.
    pub fn dismiss_rejected(&mut self) {
        self.rejected = None;
    }

    /// The repo as it is now, seen before drawing: the notice of a rejected push goes away
    /// when the current branch is no longer the one it was rejected on, when the pushed branch
    /// is gone and, for a non-fast-forward, when the branch was behind its upstream and no
    /// longer is (a fetch followed by a merge, a refresh after a pull from the terminal; a
    /// pull from the app removes it with its answer).
    pub fn follow(&mut self, head: &Head, refs: &[RefInfo]) {
        let Some(notice) = &mut self.rejected else {
            return;
        };
        if notice.on.as_deref() != current_branch(head) {
            self.rejected = None;
            return;
        }
        let Some(branch) = &notice.rejected.branch else {
            return;
        };
        let name = format!("{HEADS_PREFIX}{branch}");
        let Some(local) = refs.iter().find(|r| r.name == name) else {
            self.rejected = None;
            return;
        };
        if !notice.rejected.non_fast_forward() {
            return;
        }
        match local.upstream.as_ref().and_then(|u| u.behind) {
            Some(0) if notice.seen_behind => self.rejected = None,
            Some(0) | None => {}
            Some(_) => notice.seen_behind = true,
        }
    }

    /// A pull or push of `request` went well: the notice of the rejected push of the same
    /// branch (or tag) has nothing more to say. Also when the pull's merge or rebase stopped
    /// on a conflict: what the notice asked for was done, and the panel of the operation in
    /// progress says what is left to do.
    fn succeeded(&mut self, request: &Request) {
        let same = self.rejected.as_ref().is_some_and(|notice| {
            let rejected = &notice.rejected;
            match request.op {
                Op::Pull => rejected.non_fast_forward() && rejected.branch == request.branch,
                Op::Push | Op::PushTag => {
                    rejected.branch == request.branch && rejected.tag == request.tag
                }
            }
        });
        if same {
            self.rejected = None;
        }
    }

    /// The request, if there is not already a pull or push in progress, with the branch pinned: without
    /// `branch` (bar buttons) HEAD's branch as shown now (`head`) is used. This way the
    /// notice and dialogs that come out of it (rejection, force confirmation) always send the branch by name,
    /// even if HEAD meanwhile changes from the terminal. With a detached HEAD it stays `None` and the engine
    /// refuses.
    pub fn request(&self, mut request: Request, head: &Head) -> Option<Request> {
        if self.running.is_some() {
            return None;
        }
        if request.branch.is_none()
            && request.op != Op::PushTag
            && let Head::Branch { name, .. } = head
        {
            request.branch = Some(name.clone());
        }
        Some(request)
    }

    /// "Force (with lease)…" from the details of the push rejected as non-fast-forward: the
    /// push with `"force": true` and without `confirm`. From nowhere else.
    pub fn force(&self) -> Option<Request> {
        let Some(Window::Details(rejected)) = &self.window else {
            return None;
        };
        if !rejected.can_force() || self.running.is_some() {
            return None;
        }
        Some(Request {
            force: true,
            ..Request::push(rejected.branch.clone())
        })
    }

    /// The click on "Force the push" in the confirmation: `"force": true, "confirm": true`.
    pub fn confirm_force(&self) -> Option<Request> {
        let Some(Window::ConfirmForce(confirm)) = &self.window else {
            return None;
        };
        if self.running.is_some() || !confirm.can_confirm() || confirm.branch.is_none() {
            return None;
        }
        Some(Request {
            force: true,
            confirm: true,
            expected: confirm.expected,
            ..Request::push(confirm.branch.clone())
        })
    }

    /// The request started with `ticket`: the dialog closes (the notice of a rejected push
    /// stays until the answer says otherwise).
    pub fn started(&mut self, ticket: crate::Ticket, request: Request) {
        self.window = None;
        self.running = Some(Running::with(ticket, request));
    }

    /// "Stop": the ticket to cancel, only once.
    pub fn cancel(&mut self) -> Option<crate::Ticket> {
        self.running.as_mut()?.cancel()
    }

    /// The command's answer for `ticket`; `head`: HEAD as the tab shows it now.
    pub fn receive(
        &mut self,
        ticket: crate::Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
        expected: Option<String>,
        head: &Head,
    ) -> Received {
        let request = match self.running.take() {
            Some(running) if running.ticket == ticket => running.extra,
            other => {
                self.running = other;
                return Received::Failed {
                    message: tr!("sync-unexpected-response"),
                    reload: true,
                };
            }
        };
        let label = request.label();
        let command = request.op.command();
        let error = match result {
            Ok(value) => {
                let received = received_ok(request.op, command, value);
                if matches!(received, Received::Done(_)) {
                    self.succeeded(&request);
                }
                return received;
            }
            Err(error) => error,
        };
        let branch = request.branch.clone();
        match kind {
            Some(ErrorKind::Cancelled) => Received::Cancelled {
                notice: match request.op {
                    Op::Pull => tr!("sync-pull-cancelled", label = &label),
                    Op::Push | Op::PushTag => tr!("sync-push-cancelled", label = &label),
                },
                reload: true,
            },
            // Nothing was fetched or sent: a notice, not an error window.
            Some(ErrorKind::AuthCancelled) => Received::Cancelled {
                notice: fetch::auth_cancelled(detail.as_deref()),
                reload: false,
            },
            Some(ErrorKind::Auth) => {
                self.window = Some(Window::Failure(fetch::auth_failure(
                    &fetch::who(None),
                    detail,
                )));
                Received::Failed {
                    message: tr!("sync-failed-auth", label = &label),
                    reload: false,
                }
            }
            Some(ErrorKind::RemoteUnreachable) => {
                self.window = Some(Window::Failure(fetch::unreachable_failure(
                    &fetch::who(None),
                    detail,
                )));
                Received::Failed {
                    message: tr!("sync-failed-unreachable", label = &label),
                    reload: false,
                }
            }
            // A notice in the tab, not a dialog: it replaces the one of an earlier rejection.
            Some(ErrorKind::PushRejected(reason)) => {
                self.rejected = Some(Notice {
                    rejected: Rejected {
                        branch,
                        tag: request.tag.clone(),
                        reason,
                        title: tr!("sync-rejected-notice-title", label = &label),
                        summary: rejection_summary(reason),
                        explanation: rejection_text(reason),
                        git: detail,
                    },
                    on: current_branch(head).map(str::to_owned),
                    seen_behind: false,
                });
                Received::Failed {
                    message: tr!(
                        "sync-rejected",
                        label = &label,
                        reason = rejection_short(reason)
                    ),
                    reload: false,
                }
            }
            // The force asks for confirmation; if it was already the confirmation, the local remote-tracking branch
            // changed since it was seen: the dialog updates and is confirmed again,
            // nothing starts by itself.
            Some(ErrorKind::ConfirmRequired) if request.force => {
                self.window = Some(Window::ConfirmForce(ConfirmForce {
                    branch,
                    reason: detail.unwrap_or(error),
                    expected: expected.and_then(|e| e.parse().ok()),
                    changed: request.confirm,
                }));
                Received::Confirm
            }
            // Uncommitted changes: the UI's text with "Stash…"; the fetch may be done, re-read.
            Some(ErrorKind::LocalChanges) => {
                let mut failure = fetch::local_changes_failure(
                    tr!("sync-not-possible-title", label = &label),
                    detail.as_deref(),
                );
                // Only a pull is refused this way, and only a pull is ever sent again after a
                // stash: with the parameters of this request, as they were sent.
                if request.op == Op::Pull {
                    failure.retry = Some(Box::new(Retry::new(
                        request.params(),
                        label.clone(),
                        tr!("stash-retry-pull"),
                        Route::Pull(request.clone()),
                    )));
                }
                let message = tr!(
                    "sync-not-possible",
                    label = &label,
                    error = &failure.explanation
                );
                self.window = Some(Window::Failure(failure));
                Received::Failed {
                    message,
                    reload: request.op == Op::Pull,
                }
            }
            // Rejected before starting (detached HEAD, no upstream,
            // diverged branch with fast-forward only...): the reason is long, it goes in a window.
            // The pull may have already done the fetch: re-read.
            Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => {
                self.window = Some(Window::Failure(Failure {
                    title: tr!("sync-not-possible-title", label = &label),
                    explanation: error.clone(),
                    git: None,
                    stash: false,
                    retry: None,
                    identity: false,
                }));
                Received::Failed {
                    message: tr!("sync-not-possible", label = &label, error = error),
                    reload: request.op == Op::Pull,
                }
            }
            _ => {
                self.window = Some(Window::Failure(Failure {
                    title: tr!("sync-failed-title", label = &label),
                    explanation: error.clone(),
                    git: detail,
                    stash: false,
                    retry: None,
                    // A pull that merges or rebases creates commits.
                    identity: crate::identity::missing(&error),
                }));
                Received::Failed {
                    message: tr!("common-failed", label = &label, error = error),
                    reload: true,
                }
            }
        }
    }
}

/// What to explain in the details of a rejected push.
fn rejection_text(reason: PushRejection) -> String {
    match reason {
        PushRejection::NonFastForward => tr!("sync-rejection-non-fast-forward"),
        PushRejection::StaleLease => tr!("sync-rejection-stale-lease"),
        PushRejection::RemoteRejected => tr!("sync-rejection-remote"),
        PushRejection::Hook => tr!("sync-rejection-hook"),
        PushRejection::TagExists => tr!("sync-rejection-tag-exists"),
        PushRejection::Other => tr!("sync-rejection-other"),
    }
}

/// What to do about a rejected push, in one sentence: the notice in the tab.
fn rejection_summary(reason: PushRejection) -> String {
    match reason {
        PushRejection::NonFastForward => tr!("sync-rejection-notice-non-fast-forward"),
        PushRejection::StaleLease => tr!("sync-rejection-notice-stale-lease"),
        PushRejection::RemoteRejected => tr!("sync-rejection-notice-remote"),
        PushRejection::Hook => tr!("sync-rejection-notice-hook"),
        PushRejection::TagExists => tr!("sync-rejection-notice-tag-exists"),
        PushRejection::Other => tr!("sync-rejection-notice-other"),
    }
}

/// The rejection reason in short, for the status bar.
fn rejection_short(reason: PushRejection) -> String {
    match reason {
        PushRejection::NonFastForward => tr!("sync-rejection-short-non-fast-forward"),
        PushRejection::StaleLease => tr!("sync-rejection-short-stale-lease"),
        PushRejection::RemoteRejected => tr!("sync-rejection-short-remote"),
        PushRejection::Hook => tr!("sync-rejection-short-hook"),
        PushRejection::TagExists => tr!("sync-rejection-short-tag-exists"),
        PushRejection::Other => tr!("sync-rejection-short-other"),
    }
}

/// A successful answer of `command`: the outcome read from `value`.
fn received_ok(op: Op, command: &str, value: Value) -> Received {
    let outcome = match op {
        Op::Pull => serde_json::from_value::<RemotePullResult>(value).map(pull_outcome),
        Op::Push => serde_json::from_value::<RemotePushResult>(value).map(push_outcome),
        Op::PushTag => serde_json::from_value::<RemotePushTagResult>(value).map(push_tag_outcome),
    };
    match outcome {
        Ok(outcome) => Received::Done(Box::new(outcome)),
        Err(e) => Received::Failed {
            message: crate::unexpected_result(command, e),
            reload: true,
        },
    }
}

/// The outcome of `remote.pull`.
fn pull_outcome(r: RemotePullResult) -> Outcome {
    for line in &r.fetch.messages {
        log::info!("{PULL}: {line}");
    }
    let upstream = remote_short(&r.upstream);
    let what = tr!("sync-label-pull-branch", branch = &r.branch);
    let range = format!("{} → {}", r.before.short(), r.after.short());
    let mut notice = match r.strategy {
        // The merge or rebase of the pull stays in progress: the panel finishes or aborts it.
        PullApplied::Rebase if r.in_progress => match r.conflicts.len() {
            0 => tr!("sync-pull-stopped-rebase", what = what, upstream = upstream),
            n => tr!(
                "sync-pull-stopped-conflicts-rebase",
                what = what,
                upstream = upstream,
                n = n
            ),
        },
        _ if r.in_progress => match r.conflicts.len() {
            0 => tr!("sync-pull-stopped", what = what, upstream = upstream),
            n => tr!(
                "sync-pull-stopped-conflicts",
                what = what,
                upstream = upstream,
                n = n
            ),
        },
        PullApplied::UpToDate if r.ahead > 0 => tr!(
            "sync-pull-up-to-date-ahead",
            what = what,
            upstream = upstream,
            n = r.ahead
        ),
        PullApplied::UpToDate => tr!("sync-pull-up-to-date", what = what, upstream = upstream),
        PullApplied::FastForward => tr!(
            "sync-pull-fast-forward",
            what = what,
            upstream = upstream,
            range = range,
            n = r.behind
        ),
        PullApplied::Merge => tr!(
            "sync-pull-merge",
            what = what,
            upstream = upstream,
            range = range
        ),
        PullApplied::Rebase => tr!(
            "sync-pull-rebase",
            what = what,
            upstream = upstream,
            range = range,
            n = r.ahead
        ),
    };
    if r.strategy == PullApplied::Merge && r.strategy_source == StrategySource::Default {
        notice.push_str(&tr!("sync-pull-merge-default"));
    }
    match &r.backup {
        Some(_) if r.in_progress => notice.push_str(&tr!("sync-pull-backup-in-progress")),
        Some(_) => notice.push_str(&tr!("sync-pull-backup")),
        None => {}
    }
    notice.push_str(&crate::messages_text(&r.warnings));
    let notice = crate::status::with_backup(notice, r.backup.as_deref());
    let moved_head = r.after != r.before
        && r.status
            .as_ref()
            .is_some_and(|s| matches!(&s.head, Head::Branch { name, .. } if *name == r.branch));
    let warning = match (r.status_error, r.refs_error) {
        (Some(e), _) => Some(tr!("sync-pull-status-not-reread", error = e)),
        (None, Some(e)) => Some(tr!("sync-pull-refs-not-reread", error = e)),
        (None, None) => None,
    };
    Outcome {
        notice,
        refs: r.refs,
        status: r.status,
        history: Some(r.journal.into()),
        moved_head,
        show_changes: r.in_progress && !r.conflicts.is_empty(),
        warning,
    }
}

/// The outcome of `remote.push`.
fn push_outcome(r: RemotePushResult) -> Outcome {
    for line in &r.messages {
        log::info!("{PUSH}: {line}");
    }
    let target = format!("{}/{}", r.remote, r.remote_branch);
    let summary = if r.summary.is_empty() {
        String::new()
    } else {
        format!(" ({})", r.summary)
    };
    let branch = &r.branch;
    let mut notice = match r.kind {
        PushKind::New => tr!(
            "sync-push-new",
            branch = branch,
            target = &target,
            summary = &summary
        ),
        PushKind::FastForward => tr!(
            "sync-push-fast-forward",
            branch = branch,
            target = &target,
            summary = &summary
        ),
        PushKind::Forced => tr!(
            "sync-push-forced",
            branch = branch,
            target = &target,
            summary = &summary,
            n = r.lost.len()
        ),
        PushKind::UpToDate => tr!("sync-push-up-to-date", branch = branch, target = &target),
    };
    // A remote chosen by the push configuration is not what one expects: it is
    // said.
    match r.remote_source {
        PushRemoteSource::PushRemote => notice.push_str(&tr!(
            "sync-push-to-push-remote",
            remote = &r.remote,
            branch = &r.branch
        )),
        PushRemoteSource::PushDefault => {
            notice.push_str(&tr!("sync-push-to-push-default", remote = &r.remote));
        }
        PushRemoteSource::Param
        | PushRemoteSource::Upstream
        | PushRemoteSource::Origin
        | PushRemoteSource::Only => {}
    }
    if r.backup.is_some() {
        notice.push_str(&tr!("sync-push-backup"));
    }
    if r.set_upstream {
        notice.push_str(&tr!("sync-push-set-upstream", target = &target));
    }
    notice.push_str(&crate::messages_text(&r.warnings));
    let notice = crate::status::with_backup(notice, r.backup.as_deref());
    let warning = match (r.status_error, r.refs_error) {
        (Some(e), _) => Some(tr!("sync-push-status-not-reread", error = e)),
        (None, Some(e)) => Some(tr!("sync-push-refs-not-reread", error = e)),
        (None, None) => None,
    };
    Outcome {
        notice,
        refs: r.refs,
        status: r.status,
        history: None,
        moved_head: false,
        show_changes: false,
        warning,
    }
}

/// The outcome of `remote.push_tag`.
fn push_tag_outcome(r: RemotePushTagResult) -> Outcome {
    for line in &r.messages {
        log::info!("{PUSH_TAG}: {line}");
    }
    let (tag, remote, id) = (&r.name, &r.remote, r.target.short());
    let notice = match (r.kind, r.annotated) {
        (TagPushKind::UpToDate, _) => tr!("sync-push-tag-up-to-date", tag = tag, remote = remote),
        (TagPushKind::New, true) => tr!(
            "sync-push-tag-new-annotated",
            tag = tag,
            remote = remote,
            id = id
        ),
        (TagPushKind::New, false) => tr!("sync-push-tag-new", tag = tag, remote = remote, id = id),
    };
    let notice = if r.summary.is_empty() {
        notice
    } else {
        format!("{notice} ({})", r.summary)
    };
    Outcome {
        notice,
        refs: r.refs,
        status: None,
        history: None,
        moved_head: false,
        show_changes: false,
        warning: r
            .refs_error
            .map(|e| tr!("sync-push-refs-not-reread", error = e)),
    }
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_sync(&mut self, action: Action) {
        match action {
            Action::Pull(branch, strategy) => {
                self.send_sync(Request::pull(branch, strategy), false);
            }
            Action::Push(branch) => self.send_sync(Request::push(branch), false),
            Action::PushTag(remote, name) => {
                self.send_sync(Request::push_tag(remote, name), false);
            }
            // The click is in the details dialog, which is the open dialog.
            Action::ForcePush => {
                if let Some(request) = self.repo.as_ref().and_then(|r| r.sync.force()) {
                    self.send_sync(request, true);
                }
            }
            Action::ConfirmForcePush => {
                if let Some(request) = self.repo.as_ref().and_then(|r| r.sync.confirm_force()) {
                    self.send_sync(request, true);
                }
            }
            Action::Cancel => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.sync.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::Dismiss => {
                if let Some(repo) = &mut self.repo {
                    repo.sync.dismiss();
                }
            }
            Action::RejectedDetails => {
                if let Some(repo) = &mut self.repo
                    && !repo.modal_open()
                {
                    repo.sync.open_details();
                }
            }
            Action::DismissRejected => {
                if let Some(repo) = &mut self.repo {
                    repo.sync.dismiss_rejected();
                }
            }
        }
    }

    /// Sends a pull or push, if there is no other write in progress (fetch included: one network
    /// operation at a time) nor an open dialog; `confirmed`: it is the click in the details of
    /// a rejected push or in the force confirmation, which are themselves the open dialog.
    pub(crate) fn send_sync(&mut self, request: Request, confirmed: bool) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        if self.executor.writing().is_some() || (repo.modal_open() && !confirmed) {
            return;
        }
        let Some(request) = repo.sync.request(request, &repo.head) else {
            return;
        };
        repo.write_error = None;
        repo.write_label = Some(request.label());
        let ticket = self.executor.submit(request.op.command(), request.params());
        self.pending
            .insert(ticket, crate::Request::Sync(generation));
        if let Some(repo) = &mut self.repo {
            repo.sync.started(ticket, request);
        }
    }

    /// Answer of `remote.pull`/`remote.push`: refs, status and journal from the answer, then the re-read
    /// log (all refs: the commits that arrived, the moved branch, the updated
    /// remote-tracking branch); after a pull of the active branch the new HEAD is selected. A
    /// rejected push leaves its notice in the tab.
    pub(crate) fn receive_sync(&mut self, generation: u64, done: Done) {
        // A push may create a remote branch, a pull downloads some.
        if self.current(generation).is_some() {
            self.request_remotes(generation);
        }
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received = repo.sync.receive(
            done.ticket,
            done.result,
            done.error_kind,
            done.error_detail,
            done.confirm_expected,
            &repo.head,
        );
        match received {
            Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                if let Some(history) = outcome.history {
                    repo.history = history;
                }
                let refs_read = outcome.refs.is_some();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                let status_read = outcome.status.is_some();
                if let Some(status) = outcome.status {
                    repo.set_status(status);
                }
                if outcome.show_changes {
                    // Pull merge stopped on a conflict: the stage area with the files to resolve.
                    repo.stash.select(None);
                    repo.select_worktree(true);
                } else if outcome.moved_head && !repo.worktree_selected {
                    repo.select_after_load = repo.head.target();
                }
                self.notice.set(outcome.notice);
                self.error = outcome.warning.unwrap_or_default();
                self.follow_diff();
                if !refs_read {
                    self.submit("repo.refs", json!({}), crate::Request::Refs(generation));
                }
                if !status_read {
                    self.submit("repo.status", json!({}), crate::Request::Status(generation));
                }
                if let Some(repo) = self.current(generation) {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            Received::Cancelled { notice, reload } => {
                self.notice.set(notice);
                self.error.clear();
                if reload {
                    self.load();
                }
            }
            Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ticket;
    use crate::test_util::oid;
    use project_domain::{Change, ChangedFile, Commit, RepoPath, Signature};
    use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

    fn open(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        (registry, session)
    }

    /// Sends `request` as the app would and delivers the answer to the `SyncBox`.
    fn run(r: &Registry, s: &mut Session, sync: &mut SyncBox, request: Request) -> Received {
        let status = r.run("repo.status", s, json!({})).unwrap();
        let head: Head = serde_json::from_value(status["head"].clone()).unwrap();
        let request = sync.request(request, &head).expect("nothing in progress");
        let ticket = Ticket::for_test(1);
        let params = request.params();
        let command = request.op.command();
        sync.started(ticket, request);
        let result = r.run(command, s, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        let expected = result.as_ref().err().and_then(ErrorKind::expected);
        let text = result.map_err(|e| e.text());
        sync.receive(ticket, text, kind, detail, expected, &head)
    }

    fn main_head() -> Head {
        Head::Branch {
            name: "main".into(),
            target: Some(oid(12)),
        }
    }

    #[test]
    fn the_branch_is_pinned_when_the_request_starts() {
        let sync = SyncBox::default();
        let pinned = sync.request(Request::push(None), &main_head()).unwrap();
        assert_eq!(pinned.params(), json!({ "branch": "main" }));
        // A given branch stays that one.
        let other = sync
            .request(Request::pull(Some("x".into()), None), &main_head())
            .unwrap();
        assert_eq!(other.branch.as_deref(), Some("x"));
        // Detached HEAD: nothing to pin, the engine refuses.
        let detached = sync
            .request(Request::push(None), &Head::Detached(oid(12)))
            .unwrap();
        assert_eq!(detached.branch, None);
    }

    #[test]
    fn force_and_pull_from_the_notice_keep_the_branch_after_a_checkout() {
        let (r, mut s) = open(diverged(None));
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        let mut sync = SyncBox::default();
        run(&r, &mut s, &mut sync, Request::push(None));
        let rejected = sync.rejected().expect("the notice").clone();
        assert_eq!(rejected.branch.as_deref(), Some("main"));
        assert!(sync.open_details());
        // Checkout from the terminal while the details are open.
        r.run(
            "branch.checkout",
            &mut s,
            json!({ "name": "feature/graph" }),
        )
        .unwrap();
        let pull = Request::pull(rejected.branch, None);
        assert_eq!(pull.params(), json!({ "branch": "main" }));
        let force = sync.force().unwrap();
        assert_eq!(force.params(), json!({ "branch": "main", "force": true }));
        let received = run(&r, &mut s, &mut sync, force);
        assert!(matches!(received, Received::Confirm), "{received:?}");
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(confirm.branch.as_deref(), Some("main"));
        let confirmed = sync.confirm_force().unwrap();
        assert_eq!(confirmed.branch.as_deref(), Some("main"));
        let received = run(&r, &mut s, &mut sync, confirmed);
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome.notice.starts_with("Force push of “main”"),
            "{}",
            outcome.notice
        );
        let refs = outcome.refs.unwrap();
        let graph_ref = refs
            .iter()
            .find(|r| r.name == "refs/remotes/origin/feature/graph")
            .unwrap();
        assert_ne!(graph_ref.target, oid(12), "feature/graph was not forced");
    }

    fn who() -> Signature {
        Signature {
            name: "Carla".into(),
            email: "carla@example.com".into(),
            time: 1_700_000_000 + 20 * 3_600,
            offset_min: 0,
        }
    }

    /// The example without local changes, with `origin/main` on a commit (20) that `main` does not
    /// have; if given, 20 creates `file` with content different from the local one.
    fn diverged(file: Option<&str>) -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        repo.set_entries(Vec::new());
        repo.insert_remote_commit(
            "origin",
            Commit {
                id: oid(20),
                parents: vec![oid(11)],
                author: who(),
                committer: who(),
                summary: "From the remote".into(),
                body: String::new(),
            },
        );
        if let Some(file) = file {
            repo.set_blob(oid(5_000), "from the remote\n");
            repo.set_changes(
                oid(20),
                vec![ChangedFile {
                    path: RepoPath::from(file),
                    change: Change::Added,
                    original: None,
                    score: None,
                    old_mode: 0,
                    new_mode: 0o100644,
                    old_oid: Oid::from_bytes([0; Oid::LEN]),
                    new_oid: oid(5_000),
                }],
            );
        }
        repo.set_branch_on_remote("origin", "main", oid(20));
        repo
    }

    #[test]
    fn params_and_labels() {
        assert_eq!(Request::pull(None, None).params(), json!({}));
        assert_eq!(
            Request::pull(Some("x".into()), Some(PullStrategy::Rebase)).params(),
            json!({ "branch": "x", "strategy": "rebase" })
        );
        assert_eq!(Request::push(None).params(), json!({}));
        assert_eq!(Request::push(None).label(), "Push");
        assert_eq!(
            Request::pull(Some("feature/graph".into()), None).label(),
            "Pull of “feature/graph”"
        );
    }

    #[test]
    fn one_at_a_time_and_cancel_once() {
        let mut sync = SyncBox::default();
        let ticket = Ticket::for_test(3);
        sync.started(ticket, Request::push(None));
        assert_eq!(sync.request(Request::pull(None, None), &main_head()), None);
        assert_eq!(sync.cancel(), Some(ticket));
        assert_eq!(sync.cancel(), None);
        let r = sync.receive(
            ticket,
            Err("cancelled".into()),
            Some(ErrorKind::Cancelled),
            None,
            None,
            &main_head(),
        );
        let Received::Cancelled { notice, reload } = r else {
            panic!("{r:?}")
        };
        assert!(reload);
        assert!(notice.starts_with("Push cancelled"), "{notice}");
        assert!(notice.contains("Fetch"), "{notice}");
        assert!(sync.running().is_none());
    }

    #[test]
    fn pull_of_feature_graph_is_a_fast_forward() {
        let (r, mut s) = open(MemoryBackend::sample());
        let mut sync = SyncBox::default();
        let received = run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(Some("feature/graph".into()), None),
        );
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Pull of “feature/graph”: fast-forward to origin/feature/graph"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("1 new commit"),
            "{}",
            outcome.notice
        );
        assert!(
            crate::status::hides(&outcome.notice, "refs/project/backup/pull-1"),
            "{}",
            outcome.notice
        );
        // It is not the active branch: HEAD does not move.
        assert!(!outcome.moved_head);
        let refs = outcome.refs.unwrap();
        let graph_ref = refs
            .iter()
            .find(|r| r.name == "refs/heads/feature/graph")
            .unwrap();
        assert_eq!(graph_ref.target, oid(13));
        assert_eq!(graph_ref.upstream.as_ref().unwrap().behind, Some(0));
        // Annulla lo propone.
        let history = outcome.history.unwrap();
        assert!(history.can_undo);
        assert!(
            history
                .next_undo
                .unwrap()
                .starts_with("Pull of `feature/graph`")
        );
        assert!(sync.window().is_none());

        // Again: already up to date.
        let received = run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(Some("feature/graph".into()), None),
        );
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert_eq!(
            outcome.notice,
            "Pull of “feature/graph”: already up to date with origin/feature/graph"
        );
    }

    #[test]
    fn pull_merge_by_default_is_explained() {
        let (r, mut s) = open(diverged(Some("new.txt")));
        let mut sync = SyncBox::default();
        let received = run(&r, &mut s, &mut sync, Request::pull(None, None));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Pull of “main”: merge of origin/main"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome
                .notice
                .contains("merge because pull.rebase is not configured"),
            "{}",
            outcome.notice
        );
        assert!(outcome.moved_head);
        assert!(outcome.history.unwrap().can_undo);
    }

    #[test]
    fn pull_merge_conflict_stays_in_progress_without_a_window() {
        let (r, mut s) = open(diverged(Some("src/bin/app.rs")));
        let mut sync = SyncBox::default();
        let received = run(&r, &mut s, &mut sync, Request::pull(None, None));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        // It is not an error: no window, the stage area with the files and the re-read status (with
        // the operation in progress, hence the Continue/Abort panel).
        assert_eq!(sync.window(), None);
        assert!(outcome.show_changes);
        assert!(!outcome.moved_head);
        assert!(
            outcome
                .notice
                .starts_with("Pull of “main”: merge of origin/main stopped: 1 conflicted file"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("Continue merge"),
            "{}",
            outcome.notice
        );
        assert!(
            crate::status::hides(&outcome.notice, "refs/project/backup/pull-1"),
            "{}",
            outcome.notice
        );
        assert_eq!(
            outcome.status.unwrap().operation,
            Some(project_domain::Operation::Merge)
        );
        assert!(!outcome.history.unwrap().can_undo);
    }

    #[test]
    fn pull_rebase_conflict_stays_in_progress_without_a_window() {
        // Since S32 a stopped rebase stays in progress like the merge (`rebase.continue`,
        // `rebase.abort` from the panel): no error, no window, a notice that speaks of the rebase.
        let (r, mut s) = open(diverged(Some("src/bin/app.rs")));
        let mut sync = SyncBox::default();
        let received = run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(None, Some(PullStrategy::Rebase)),
        );
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert_eq!(sync.window(), None);
        assert!(outcome.show_changes);
        assert!(!outcome.moved_head);
        assert!(
            outcome
                .notice
                .starts_with("Pull of “main”: rebase onto origin/main stopped: 1 conflicted file"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("Continue rebase…")
                && outcome.notice.contains("Abort rebase…")
                && !outcome.notice.contains("merge"),
            "{}",
            outcome.notice
        );
        assert_eq!(
            outcome.status.unwrap().operation,
            Some(project_domain::Operation::Rebase)
        );
        assert!(!outcome.history.unwrap().can_undo);
    }

    #[test]
    fn push_of_main_is_a_fast_forward() {
        let (r, mut s) = open(MemoryBackend::sample());
        let mut sync = SyncBox::default();
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Push of “main” to origin/main ("),
            "{}",
            outcome.notice
        );
        assert!(outcome.history.is_none());
        assert!(!outcome.notice.contains("pushDefault"));
        let refs = outcome.refs.unwrap();
        let origin = refs
            .iter()
            .find(|r| r.name == "refs/remotes/origin/main")
            .unwrap();
        assert_eq!(origin.target, oid(12));

        // The first push of a new branch sets the upstream.
        r.run("branch.create", &mut s, json!({ "name": "fresh" }))
            .unwrap();
        let received = run(&r, &mut s, &mut sync, Request::push(Some("fresh".into())));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome.notice.ends_with("; upstream set to origin/fresh"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn push_rejected_offers_pull_then_force_asks_to_confirm() {
        let (r, mut s) = open(diverged(None));
        let mut sync = SyncBox::default();
        assert_eq!(
            sync.force(),
            None,
            "the force does not start without the details dialog"
        );
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        let Received::Failed { message, reload } = received else {
            panic!("{received:?}")
        };
        assert!(!reload);
        assert!(
            message.contains("the remote has commits you don't have: Pull"),
            "{message}"
        );
        // A notice in the tab, not a dialog: nothing is blocked.
        assert_eq!(sync.window(), None);
        assert!(!sync.modal_open());
        let rejected = sync.rejected().expect("the notice").clone();
        assert!(rejected.non_fast_forward());
        assert_eq!(rejected.title, "Push of “main” rejected");
        assert_eq!(
            rejected.summary,
            "The remote has commits you don't have: Pull to integrate them, then push again."
        );
        assert!(
            rejected
                .explanation
                .starts_with("The remote has commits you don't have: Pull to integrate them")
        );
        assert!(rejected.git.as_deref().unwrap().contains("[rejected]"));
        // The force is offered only in the details, which are a modal dialog.
        assert_eq!(sync.force(), None);
        assert!(sync.open_details());
        assert!(sync.modal_open());
        assert_eq!(sync.window(), Some(&Window::Details(rejected)));

        // "Force (with lease)…": force without confirm. The local remote-tracking branch is old (the
        // remote has 20, here it has not arrived yet): the lease stops it.
        let force = sync.force().unwrap();
        assert_eq!(force.params(), json!({ "branch": "main", "force": true }));
        let received = run(&r, &mut s, &mut sync, force);
        assert!(matches!(received, Received::Failed { .. }), "{received:?}");
        assert_eq!(
            sync.window(),
            None,
            "the details closed when the push started"
        );
        let stale = sync.rejected().expect("the notice").clone();
        assert!(stale.stale());
        assert_eq!(stale.title, "Force push of “main” rejected");
        assert!(stale.explanation.contains("Fetch and check"));
        assert!(sync.open_details());
        assert_eq!(sync.force(), None, "after the stale lease no force");
        sync.dismiss();

        // After the fetch the force would remove 20: the engine asks for confirmation.
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        assert!(matches!(received, Received::Failed { .. }));
        assert!(sync.open_details());
        let force = sync.force().unwrap();
        let received = run(&r, &mut s, &mut sync, force);
        assert!(matches!(received, Received::Confirm), "{received:?}");
        assert!(sync.modal_open());
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(
            confirm.reason.contains("“From the remote”"),
            "{}",
            confirm.reason
        );
        assert!(confirm.reason.contains("refs/project/backup/push-"));
        // "Do not force" closes without sending anything.
        let pending = sync.window().cloned();
        sync.dismiss();
        assert_eq!(sync.confirm_force(), None);
        sync.window = pending;

        let confirmed = sync.confirm_force().unwrap();
        assert_eq!(
            confirmed.params(),
            json!({
                "branch": "main", "force": true, "confirm": true,
                "expected": oid(20).to_string()
            })
        );
        let received = run(&r, &mut s, &mut sync, confirmed);
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Force push of “main” to origin/main"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("1 commit removed from the remote"),
            "{}",
            outcome.notice
        );
        assert!(crate::status::hides(
            &outcome.notice,
            "refs/project/backup/push-1"
        ));
        assert!(!sync.modal_open());
    }

    fn refs(r: &Registry, s: &mut Session) -> Vec<RefInfo> {
        serde_json::from_value(r.run("repo.refs", s, json!({})).unwrap()).unwrap()
    }

    /// A push of `main` rejected because the remote has commit 20: the notice.
    fn rejected_main() -> (Registry, Session, SyncBox) {
        let (r, mut s) = open(diverged(None));
        let mut sync = SyncBox::default();
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        assert!(matches!(received, Received::Failed { .. }), "{received:?}");
        assert!(sync.rejected().is_some());
        (r, s, sync)
    }

    #[test]
    fn the_rejected_notice_blocks_nothing_and_is_dismissed() {
        let (r, mut s, mut sync) = rejected_main();
        assert!(!sync.modal_open(), "a notice is not a dialog");
        assert_eq!(sync.window(), None);
        // The refs still say "not behind" (nobody fetched): that is not a reason to hide it.
        let now = refs(&r, &mut s);
        sync.follow(&main_head(), &now);
        sync.follow(&main_head(), &now);
        assert!(sync.rejected().is_some());
        // Another pull or push can start while it is there.
        assert!(
            sync.request(Request::pull(None, None), &main_head())
                .is_some()
        );
        // The details are a dialog; closing them leaves the notice.
        assert!(sync.open_details());
        assert!(sync.modal_open());
        sync.dismiss();
        assert!(!sync.modal_open());
        assert!(sync.rejected().is_some());
        // "×".
        sync.dismiss_rejected();
        assert_eq!(sync.rejected(), None);
        assert!(!sync.open_details(), "no details without a notice");
    }

    #[test]
    fn the_rejected_notice_goes_when_the_branch_is_no_longer_behind() {
        let (r, mut s, mut sync) = rejected_main();
        // A fetch shows the branch behind: the notice still has something to say.
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        let behind = refs(&r, &mut s);
        sync.follow(&main_head(), &behind);
        assert!(sync.rejected().is_some());
        // A pull done elsewhere (the terminal), seen by a refresh: no longer behind.
        r.run("remote.pull", &mut s, json!({})).unwrap();
        let merged = refs(&r, &mut s);
        let head: Head = {
            let status = r.run("repo.status", &mut s, json!({})).unwrap();
            serde_json::from_value(status["head"].clone()).unwrap()
        };
        sync.follow(&head, &merged);
        assert_eq!(sync.rejected(), None);
    }

    #[test]
    fn the_rejected_notice_goes_with_a_pull_from_it() {
        // "Pull" in the notice, without a fetch before: the refs never showed the branch behind.
        let (r, mut s, mut sync) = rejected_main();
        let branch = sync.rejected().unwrap().branch.clone();
        let received = run(&r, &mut s, &mut sync, Request::pull(branch, None));
        assert!(matches!(received, Received::Done(_)), "{received:?}");
        assert_eq!(sync.rejected(), None);

        // A pull that stops on a conflict did what the notice asked for: the panel of the
        // merge in progress takes over.
        let (r, mut s) = open(diverged(Some("src/bin/app.rs")));
        let mut sync = SyncBox::default();
        run(&r, &mut s, &mut sync, Request::push(None));
        assert!(sync.rejected().is_some());
        let received = run(&r, &mut s, &mut sync, Request::pull(None, None));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(outcome.show_changes, "stopped on a conflict");
        assert_eq!(sync.rejected(), None);
    }

    #[test]
    fn the_rejected_notice_goes_with_a_successful_push_of_that_branch() {
        let (r, mut s, mut sync) = rejected_main();
        // The push of another branch says nothing about `main`.
        r.run("branch.create", &mut s, json!({ "name": "fresh" }))
            .unwrap();
        let received = run(&r, &mut s, &mut sync, Request::push(Some("fresh".into())));
        assert!(matches!(received, Received::Done(_)), "{received:?}");
        assert!(sync.rejected().is_some());
        // A push rejected again replaces the notice; one that fails otherwise leaves it.
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        assert!(matches!(received, Received::Failed { .. }), "{received:?}");
        assert!(sync.rejected().is_some());
        // Integrated from the terminal, then the push goes through.
        r.run("remote.pull", &mut s, json!({})).unwrap();
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        assert!(matches!(received, Received::Done(_)), "{received:?}");
        assert_eq!(sync.rejected(), None);
    }

    #[test]
    fn the_rejected_notice_goes_when_the_current_branch_changes() {
        let (r, mut s, mut sync) = rejected_main();
        let now = refs(&r, &mut s);
        let other = Head::Branch {
            name: "feature/graph".into(),
            target: Some(oid(12)),
        };
        sync.follow(&other, &now);
        assert_eq!(sync.rejected(), None);

        // Detached HEAD is a change too; and so is the pushed branch that no longer exists.
        let (r, mut s, mut sync) = rejected_main();
        sync.follow(&Head::Detached(oid(12)), &refs(&r, &mut s));
        assert_eq!(sync.rejected(), None);
        let (_, _, mut sync) = rejected_main();
        sync.follow(&main_head(), &[]);
        assert_eq!(sync.rejected(), None);
    }

    #[test]
    fn push_remote_from_the_push_config_is_mentioned() {
        let value = json!({
            "remote": "fork", "remote_source": "push_default", "branch": "main",
            "remote_branch": "main", "kind": "new", "summary": "[new branch]",
            "before": null, "after": oid(12).to_string(), "set_upstream": false,
            "forced": false, "lost": [], "backup": null, "updated": [], "messages": [],
            "warnings": [], "status": null, "refs": null
        });
        let outcome = push_outcome(serde_json::from_value(value).unwrap());
        assert!(
            outcome.notice.contains("to “fork” (remote.pushDefault)"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn push_of_a_tag_new_then_up_to_date_then_rejected() {
        let request = Request::push_tag("origin".into(), "v0.1.0".into());
        assert_eq!(
            request.params(),
            json!({ "remote": "origin", "name": "v0.1.0" })
        );
        assert_eq!(request.label(), "Push of tag “v0.1.0” to “origin”");
        // HEAD's branch is not pinned on a tag push.
        let pinned = SyncBox::default().request(request.clone(), &main_head());
        assert_eq!(pinned.as_ref().and_then(|r| r.branch.clone()), None);

        let (r, mut s) = open(MemoryBackend::sample());
        let mut sync = SyncBox::default();
        let Received::Done(outcome) = run(&r, &mut s, &mut sync, request.clone()) else {
            panic!()
        };
        assert!(
            outcome
                .notice
                .starts_with("Annotated tag “v0.1.0” published on “origin” (at "),
            "{}",
            outcome.notice
        );
        assert!(outcome.history.is_none());
        assert!(sync.running().is_none());
        let Received::Done(outcome) = run(&r, &mut s, &mut sync, request.clone()) else {
            panic!()
        };
        assert_eq!(
            outcome.notice,
            "Tag “v0.1.0” is already on “origin” with the same value ([up to date])"
        );

        // On the remote with another value: rejected with its own explanation, nothing to force.
        let mut repo = MemoryBackend::sample();
        repo.set_tag_on_remote("origin", "v0.1.0", oid(3), None);
        let (r, mut s) = open(repo);
        let received = run(&r, &mut s, &mut sync, request);
        let Received::Failed { message, reload } = received else {
            panic!("{received:?}")
        };
        assert!(!reload);
        assert!(
            message.contains("the remote already has this tag on another value"),
            "{message}"
        );
        let rejected = sync.rejected().expect("the notice").clone();
        assert_eq!(rejected.reason, PushRejection::TagExists);
        assert_eq!(rejected.title, "Push of tag “v0.1.0” to “origin” rejected");
        assert!(sync.open_details());
        assert!(
            rejected.explanation.contains("a tag is never overwritten"),
            "{}",
            rejected.explanation
        );
        assert_eq!(sync.force(), None);
    }

    #[test]
    fn push_rejected_by_a_hook_shows_its_output() {
        let mut repo = MemoryBackend::sample();
        repo.fail_pre_push(Some("pre-push: tests failed"));
        let (r, mut s) = open(repo);
        let mut sync = SyncBox::default();
        run(&r, &mut s, &mut sync, Request::push(None));
        let rejected = sync.rejected().expect("the notice").clone();
        assert_eq!(rejected.reason, PushRejection::Hook);
        assert_eq!(rejected.git.as_deref(), Some("pre-push: tests failed"));
        assert!(sync.open_details());
        assert_eq!(sync.force(), None);
    }

    #[test]
    fn a_confirm_without_force_is_not_a_force_dialog() {
        let mut sync = SyncBox::default();
        let ticket = Ticket::for_test(0);
        sync.started(ticket, Request::push(None));
        let r = sync.receive(
            ticket,
            Err("confirmation needed".into()),
            Some(ErrorKind::ConfirmRequired),
            Some("motivo".into()),
            None,
            &main_head(),
        );
        assert!(matches!(r, Received::Failed { .. }), "{r:?}");
        assert!(!sync.modal_open());
    }

    #[test]
    fn a_force_confirm_without_expected_cannot_be_confirmed() {
        let mut sync = SyncBox::default();
        let ticket = Ticket::for_test(0);
        sync.started(
            ticket,
            Request {
                force: true,
                ..Request::push(None)
            },
        );
        let r = sync.receive(
            ticket,
            Err("confirmation needed".into()),
            Some(ErrorKind::ConfirmRequired),
            Some("the local remote-tracking branch is gone".into()),
            None,
            &main_head(),
        );
        assert!(matches!(r, Received::Confirm), "{r:?}");
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(!confirm.can_confirm());
        assert_eq!(sync.confirm_force(), None);
    }

    #[test]
    fn a_remote_that_moved_after_the_dialog_asks_again() {
        let (r, mut s) = open(diverged(None));
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        // The confirmation seen when `origin/main` was still on 11; now it is on 20.
        let mut sync = SyncBox {
            window: Some(Window::ConfirmForce(ConfirmForce {
                branch: Some("main".into()),
                reason: "old reason".into(),
                expected: Some(oid(11)),
                changed: false,
            })),
            ..SyncBox::default()
        };
        let confirmed = sync.confirm_force().unwrap();
        assert_eq!(
            confirmed.params(),
            json!({
                "branch": "main", "force": true, "confirm": true,
                "expected": oid(11).to_string()
            })
        );
        let received = run(&r, &mut s, &mut sync, confirmed);
        // Nothing published: the dialog updates with the new oid and waits for another click.
        assert!(matches!(received, Received::Confirm), "{received:?}");
        assert!(sync.running().is_none());
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(confirm.changed);
        assert_eq!(confirm.expected, Some(oid(20)));
        assert_ne!(confirm.reason, "old reason");
        assert_eq!(
            r.run("repo.refs", &mut s, json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["name"] == "refs/remotes/origin/main")
                .unwrap()["target"],
            oid(20).to_string()
        );
        let again = sync.confirm_force().unwrap();
        assert_eq!(again.expected, Some(oid(20)));
        let received = run(&r, &mut s, &mut sync, again);
        assert!(matches!(received, Received::Done(_)), "{received:?}");
    }

    #[test]
    fn pull_without_upstream_is_explained_in_a_window() {
        let (r, mut s) = open(MemoryBackend::sample());
        r.run("branch.create", &mut s, json!({ "name": "lone" }))
            .unwrap();
        let mut sync = SyncBox::default();
        run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(Some("lone".into()), None),
        );
        let Some(Window::Failure(failure)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(failure.title, "Pull of “lone” not possible");
        assert!(failure.explanation.contains("--set-upstream-to"));
    }
}
