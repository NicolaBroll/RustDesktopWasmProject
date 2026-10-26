//! Pull and push (S26): `remote.pull` and `remote.push` in the background, with progress and
//! "Stop" like the fetch. Data only, no egui: tested with unit tests.
//!
//! One network operation at a time: fetch, pull and push are writes
//! ([`CommandSpec::writes`](project_engine::CommandSpec)), and the UI does not send one while
//! another is in progress. Rejections to explain open a window:
//!
//! - pull whose merge or rebase stopped on a conflict: not an error, the operation **stays in
//!   progress** as with `merge.start` (`in_progress` in the result; the rebase since S32). The
//!   re-read status makes the in-progress operation panel appear (Continue/Abort merge or rebase)
//!   and the stage area opens with the files;
//! - pull stopped on a conflict and aborted by the engine ([`ErrorKind::PullConflict`], no longer
//!   produced since S32 but still handled): the repo is as before; the window lists the files and
//!   proposes the other strategy;
//! - rejected push ([`ErrorKind::PushRejected`]): non-fast-forward proposes "Pull" and, as a
//!   second choice, "Force (with lease)…"; stale lease proposes "Fetch"; a hook shows its
//!   output.
//!
//! The force never starts from a bar button: only from that window, first with
//! `"force": true` and without `confirm`. If the remote would lose commits the engine answers
//! `ConfirmRequired` with the commits, and the confirmation opens; `"confirm": true` only after the click.

use crate::ErrorKind;
use crate::discard::History;
use crate::fetch::{self, Failure};
use crate::{Action, Done, Tab, sync};
use project_domain::{Commit, Head, Oid, RefInfo, RepoPath, Status};
use project_engine::{PullOperation, PushRejection};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

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

/// The strategy requested for a pull (`None` in the request: the configured one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Merge,
    Rebase,
}

impl Strategy {
    fn param(self) -> &'static str {
        match self {
            Strategy::Merge => "merge",
            Strategy::Rebase => "rebase",
        }
    }

    /// The other strategy than the operation that stopped on a conflict.
    pub fn other_than(operation: PullOperation) -> Self {
        match operation {
            PullOperation::Merge => Strategy::Rebase,
            PullOperation::Rebase => Strategy::Merge,
        }
    }

    pub fn name(self) -> &'static str {
        self.param()
    }
}

/// What to ask for: pull or push, of which branch, with which options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub op: Op,
    /// The local branch (short name); `None` = HEAD's.
    pub branch: Option<String>,
    /// Pull only: the strategy instead of the configuration.
    pub strategy: Option<Strategy>,
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
    pub fn pull(branch: Option<String>, strategy: Option<Strategy>) -> Self {
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
            return json!({ "remote": remote, "name": name });
        }
        let mut params = json!({});
        if let Some(branch) = &self.branch {
            params["branch"] = json!(branch);
        }
        if let Some(strategy) = self.strategy {
            params["strategy"] = json!(strategy.param());
        }
        if self.force {
            params["force"] = json!(true);
        }
        if self.confirm {
            params["confirm"] = json!(true);
        }
        if let Some(expected) = self.expected {
            params["expected"] = json!(expected.to_string());
        }
        params
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
            (Op::Push, false, None) => tr!("sync-label-push"),
            (Op::Push, true, Some(branch)) => tr!("sync-label-force-branch", branch = branch),
            (Op::Push, true, None) => tr!("sync-label-force"),
            (Op::PushTag, ..) => tr!("sync-label-push"),
        }
    }
}

/// The pull or push in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub ticket: crate::Ticket,
    pub request: Request,
    /// True after "Stop", until the command answers.
    pub cancelling: bool,
}

/// A pull stopped on a conflict and already aborted by the engine: today only the rebase (the stopped merge
/// stays in progress, [`Outcome::show_changes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The pull's branch (`None`: HEAD's).
    pub branch: Option<String>,
    pub operation: PullOperation,
    pub files: Vec<String>,
    /// The engine's message (with the upstream and what to do from the terminal).
    pub message: String,
}

impl Conflict {
    /// The strategy to propose to retry.
    pub fn other(&self) -> Strategy {
        Strategy::other_than(self.operation)
    }
}

/// A rejected push: nothing changed, neither on the remote nor locally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// The push's branch (`None`: HEAD's).
    pub branch: Option<String>,
    pub reason: PushRejection,
    /// What happened and what to do.
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

/// The window opened after a pull or push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Window {
    /// Error to explain (credentials, network, rejection with a long message, git error).
    Failure(Failure),
    Conflict(Conflict),
    Rejected(Rejected),
    /// The force confirmation: it is a modal dialog.
    ConfirmForce(ConfirmForce),
}

/// Pull and push: the one in progress and the open window.
#[derive(Debug, Default)]
pub struct SyncBox {
    running: Option<Running>,
    window: Option<Window>,
}

/// What arrived from `remote.pull` or `remote.push`.
#[derive(Debug)]
pub enum Received {
    Done(Box<Outcome>),
    /// The engine asks for the force confirmation: the window is open.
    Confirm,
    /// Interrupted: with `reload` refs, status and log are re-read (the pull's fetch or the push
    /// may have updated something).
    Cancelled {
        notice: String,
        reload: bool,
    },
    /// Failed or rejected: the message for the status bar (the details, if needed,
    /// in the window); with `reload` refs, status and log are re-read.
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
    pub fn running(&self) -> Option<&Running> {
        self.running.as_ref()
    }

    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// True if the force confirmation is open: while it is, no writes.
    pub fn modal_open(&self) -> bool {
        matches!(self.window, Some(Window::ConfirmForce(_)))
    }

    /// Closes the window.
    pub fn dismiss(&mut self) {
        self.window = None;
    }

    /// The request, if there is not already a pull or push in progress, with the branch pinned: without
    /// `branch` (bar buttons) HEAD's branch as shown now (`head`) is used. This way the
    /// windows that come out of it (rejection, force confirmation) always send the branch by name,
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

    /// "Force (with lease)…" from the window of the push rejected as non-fast-forward: the push
    /// with `"force": true` and without `confirm`. From nowhere else.
    pub fn force(&self) -> Option<Request> {
        let Some(Window::Rejected(rejected)) = &self.window else {
            return None;
        };
        if !rejected.non_fast_forward() || self.running.is_some() || rejected.branch.is_none() {
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

    /// The request started with `ticket`: the window closes.
    pub fn started(&mut self, ticket: crate::Ticket, request: Request) {
        self.window = None;
        self.running = Some(Running {
            ticket,
            request,
            cancelling: false,
        });
    }

    /// "Stop": the ticket to cancel, only once.
    pub fn cancel(&mut self) -> Option<crate::Ticket> {
        let running = self.running.as_mut().filter(|r| !r.cancelling)?;
        running.cancelling = true;
        Some(running.ticket)
    }

    /// The command's answer for `ticket`.
    pub fn receive(
        &mut self,
        ticket: crate::Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
        expected: Option<String>,
    ) -> Received {
        let request = match self.running.take() {
            Some(running) if running.ticket == ticket => running.request,
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
                let outcome = match request.op {
                    Op::Pull => {
                        serde_json::from_value::<PullResult>(value).map(PullResult::outcome)
                    }
                    Op::Push => {
                        serde_json::from_value::<PushResult>(value).map(PushResult::outcome)
                    }
                    Op::PushTag => {
                        serde_json::from_value::<PushTagResult>(value).map(PushTagResult::outcome)
                    }
                };
                return match outcome {
                    Ok(outcome) => Received::Done(Box::new(outcome)),
                    Err(e) => Received::Failed {
                        message: crate::unexpected_result(command, e),
                        reload: true,
                    },
                };
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
            Some(ErrorKind::PullConflict(operation)) => {
                let files = detail
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                self.window = Some(Window::Conflict(Conflict {
                    branch,
                    operation,
                    files,
                    message: error,
                }));
                // The fetch is done: the remote-tracking branch is re-read.
                Received::Failed {
                    message: tr!(
                        "sync-pull-conflict",
                        label = &label,
                        operation = operation.to_string()
                    ),
                    reload: true,
                }
            }
            Some(ErrorKind::PushRejected(reason)) => {
                self.window = Some(Window::Rejected(Rejected {
                    branch,
                    reason,
                    explanation: rejection_text(reason),
                    git: detail,
                }));
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
            // Rejected before starting (detached HEAD, no upstream, local changes,
            // diverged branch with fast-forward only...): the reason is long, it goes in a window.
            // The pull may have already done the fetch: re-read.
            Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => {
                self.window = Some(Window::Failure(Failure {
                    title: tr!("sync-not-possible-title", label = &label),
                    explanation: error.clone(),
                    git: None,
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
                }));
                Received::Failed {
                    message: tr!("common-failed", label = &label, error = error),
                    reload: true,
                }
            }
        }
    }
}

/// What to explain in the window of a rejected push.
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

/// `refs/remotes/origin/main` → `origin/main`.
fn remote_short(name: &str) -> &str {
    name.strip_prefix("refs/remotes/").unwrap_or(name)
}

/// The `fetch` part in the pull result.
#[derive(Debug, Clone, Deserialize)]
struct PullFetch {
    #[serde(default)]
    messages: Vec<String>,
}

/// Result of `remote.pull`.
#[derive(Debug, Clone, Deserialize)]
struct PullResult {
    branch: String,
    upstream: String,
    strategy: String,
    strategy_source: String,
    before: Oid,
    after: Oid,
    ahead: u64,
    behind: u64,
    /// The merge stopped (conflicts, hook, signature) and stays in progress.
    #[serde(default)]
    in_progress: bool,
    #[serde(default)]
    conflicts: Vec<RepoPath>,
    backup: Option<String>,
    fetch: Option<PullFetch>,
    #[serde(default, deserialize_with = "crate::engine_text::texts")]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

impl PullResult {
    fn outcome(self) -> Outcome {
        for line in self.fetch.iter().flat_map(|f| &f.messages) {
            log::info!("{PULL}: {line}");
        }
        let upstream = remote_short(&self.upstream);
        let what = tr!("sync-label-pull-branch", branch = &self.branch);
        let range = format!("{} → {}", self.before.short(), self.after.short());
        let mut notice = match self.strategy.as_str() {
            // The merge or rebase of the pull stays in progress: the panel finishes or aborts it.
            "rebase" if self.in_progress => match self.conflicts.len() {
                0 => tr!("sync-pull-stopped-rebase", what = what, upstream = upstream),
                n => tr!(
                    "sync-pull-stopped-conflicts-rebase",
                    what = what,
                    upstream = upstream,
                    n = n
                ),
            },
            _ if self.in_progress => match self.conflicts.len() {
                0 => tr!("sync-pull-stopped", what = what, upstream = upstream),
                n => tr!(
                    "sync-pull-stopped-conflicts",
                    what = what,
                    upstream = upstream,
                    n = n
                ),
            },
            "up_to_date" if self.ahead > 0 => tr!(
                "sync-pull-up-to-date-ahead",
                what = what,
                upstream = upstream,
                n = self.ahead
            ),
            "up_to_date" => tr!("sync-pull-up-to-date", what = what, upstream = upstream),
            "fast_forward" => tr!(
                "sync-pull-fast-forward",
                what = what,
                upstream = upstream,
                range = range,
                n = self.behind
            ),
            "merge" => tr!(
                "sync-pull-merge",
                what = what,
                upstream = upstream,
                range = range
            ),
            "rebase" => tr!(
                "sync-pull-rebase",
                what = what,
                upstream = upstream,
                range = range,
                n = self.ahead
            ),
            other => tr!("sync-pull-other", what = what, kind = other, range = range),
        };
        if self.strategy == "merge" && self.strategy_source == "default" {
            notice.push_str(&tr!("sync-pull-merge-default"));
        }
        match &self.backup {
            Some(backup) if self.in_progress => {
                notice.push_str(&tr!("sync-pull-backup-in-progress", backup = backup));
            }
            Some(backup) => notice.push_str(&tr!("sync-pull-backup", backup = backup)),
            None => {}
        }
        notice.push_str(&crate::warnings_text(&self.warnings));
        let moved_head = self.after != self.before
            && self.status.as_ref().is_some_and(
                |s| matches!(&s.head, Head::Branch { name, .. } if *name == self.branch),
            );
        let warning = match (self.status_error, self.refs_error) {
            (Some(e), _) => Some(tr!("sync-pull-status-not-reread", error = e)),
            (None, Some(e)) => Some(tr!("sync-pull-refs-not-reread", error = e)),
            (None, None) => None,
        };
        Outcome {
            notice,
            refs: self.refs,
            status: self.status,
            history: Some(self.history),
            moved_head,
            show_changes: self.in_progress && !self.conflicts.is_empty(),
            warning,
        }
    }
}

/// Result of `remote.push`.
#[derive(Debug, Clone, Deserialize)]
struct PushResult {
    remote: String,
    /// Where the remote comes from: `param`, `push_remote`, `push_default`, `upstream`, `origin`,
    /// `only`.
    #[serde(default)]
    remote_source: Option<String>,
    branch: String,
    remote_branch: String,
    kind: String,
    #[serde(default)]
    summary: String,
    set_upstream: bool,
    #[serde(default)]
    lost: Vec<Commit>,
    backup: Option<String>,
    #[serde(default)]
    messages: Vec<String>,
    #[serde(default, deserialize_with = "crate::engine_text::texts")]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
}

impl PushResult {
    fn outcome(self) -> Outcome {
        for line in &self.messages {
            log::info!("{PUSH}: {line}");
        }
        let target = format!("{}/{}", self.remote, self.remote_branch);
        let summary = if self.summary.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.summary)
        };
        let branch = &self.branch;
        let mut notice = match self.kind.as_str() {
            "new" => tr!(
                "sync-push-new",
                branch = branch,
                target = &target,
                summary = &summary
            ),
            "fast_forward" => tr!(
                "sync-push-fast-forward",
                branch = branch,
                target = &target,
                summary = &summary
            ),
            "forced" => tr!(
                "sync-push-forced",
                branch = branch,
                target = &target,
                summary = &summary,
                n = self.lost.len()
            ),
            "up_to_date" => tr!("sync-push-up-to-date", branch = branch, target = &target),
            other => tr!(
                "sync-push-other",
                branch = branch,
                target = &target,
                kind = other
            ),
        };
        // A remote chosen by the push configuration is not what one expects: it is
        // said.
        match self.remote_source.as_deref() {
            Some("push_remote") => notice.push_str(&tr!(
                "sync-push-to-push-remote",
                remote = &self.remote,
                branch = &self.branch
            )),
            Some("push_default") => {
                notice.push_str(&tr!("sync-push-to-push-default", remote = &self.remote));
            }
            _ => {}
        }
        if let Some(backup) = &self.backup {
            notice.push_str(&tr!("sync-push-backup", backup = backup));
        }
        if self.set_upstream {
            notice.push_str(&tr!("sync-push-set-upstream", target = &target));
        }
        notice.push_str(&crate::warnings_text(&self.warnings));
        let warning = match (self.status_error, self.refs_error) {
            (Some(e), _) => Some(tr!("sync-push-status-not-reread", error = e)),
            (None, Some(e)) => Some(tr!("sync-push-refs-not-reread", error = e)),
            (None, None) => None,
        };
        Outcome {
            notice,
            refs: self.refs,
            status: self.status,
            history: None,
            moved_head: false,
            show_changes: false,
            warning,
        }
    }
}

/// Result of `remote.push_tag`.
#[derive(Debug, Clone, Deserialize)]
struct PushTagResult {
    remote: String,
    name: String,
    target: Oid,
    annotated: bool,
    kind: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    messages: Vec<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
}

impl PushTagResult {
    fn outcome(self) -> Outcome {
        for line in &self.messages {
            log::info!("{PUSH_TAG}: {line}");
        }
        let (tag, remote, id) = (&self.name, &self.remote, self.target.short());
        let notice = match (self.kind.as_str(), self.annotated) {
            ("up_to_date", _) => tr!("sync-push-tag-up-to-date", tag = tag, remote = remote),
            (_, true) => tr!(
                "sync-push-tag-new-annotated",
                tag = tag,
                remote = remote,
                id = id
            ),
            (_, false) => tr!("sync-push-tag-new", tag = tag, remote = remote, id = id),
        };
        let notice = if self.summary.is_empty() {
            notice
        } else {
            format!("{notice} ({})", self.summary)
        };
        Outcome {
            notice,
            refs: self.refs,
            status: None,
            history: None,
            moved_head: false,
            show_changes: false,
            warning: self
                .refs_error
                .map(|e| tr!("sync-push-refs-not-reread", error = e)),
        }
    }
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_sync(&mut self, action: Action) {
        match action {
            Action::Pull(branch, strategy) => {
                self.send_sync(sync::Request::pull(branch, strategy), false);
            }
            Action::Push(branch) => self.send_sync(sync::Request::push(branch), false),
            Action::PushTag(remote, name) => {
                self.send_sync(sync::Request::push_tag(remote, name), false);
            }
            Action::ForcePush => {
                if let Some(request) = self.repo.as_ref().and_then(|r| r.sync.force()) {
                    self.send_sync(request, false);
                }
            }
            Action::ConfirmForcePush => {
                if let Some(request) = self.repo.as_ref().and_then(|r| r.sync.confirm_force()) {
                    self.send_sync(request, true);
                }
            }
            Action::CancelSync => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.sync.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::DismissSync => {
                if let Some(repo) = &mut self.repo {
                    repo.sync.dismiss();
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Sends a pull or push, if there is no other write in progress (fetch included: one network
    /// operation at a time) nor an open dialog; `confirmed`: it is the click in the
    /// force confirmation, which is itself the open dialog.
    pub(crate) fn send_sync(&mut self, request: sync::Request, confirmed: bool) {
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
    /// remote-tracking branch); after a pull of the active branch the new HEAD is selected. A rejection opens
    /// its window.
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
        );
        match received {
            sync::Received::Done(outcome) => {
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
                self.notice = outcome.notice;
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
            sync::Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            sync::Received::Cancelled { notice, reload } => {
                self.notice = notice;
                self.error.clear();
                if reload {
                    self.load();
                }
            }
            sync::Received::Failed { message, reload } => {
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
    use project_domain::{Change, ChangedFile, RepoPath, Signature};
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
        sync.receive(ticket, result.map_err(|e| e.text()), kind, detail, expected)
    }

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
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
    fn force_and_pull_from_the_windows_keep_the_branch_after_a_checkout() {
        let (r, mut s) = open(diverged(None));
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        let mut sync = SyncBox::default();
        run(&r, &mut s, &mut sync, Request::push(None));
        let Some(Window::Rejected(rejected)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(rejected.branch.as_deref(), Some("main"));
        // Checkout from the terminal while the window is open.
        r.run(
            "branch.checkout",
            &mut s,
            json!({ "name": "feature/graph" }),
        )
        .unwrap();
        let pull = Request::pull(rejected.branch.clone(), None);
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
            Request::pull(Some("x".into()), Some(Strategy::Rebase)).params(),
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
            outcome.notice.contains("refs/project/backup/pull-1"),
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
            outcome.notice.contains("refs/project/backup/pull-1"),
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
            Request::pull(None, Some(Strategy::Rebase)),
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
            "the force does not start without the window"
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
        let Some(Window::Rejected(rejected)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(rejected.non_fast_forward());
        assert!(
            rejected
                .explanation
                .starts_with("The remote has commits you don't have: Pull to integrate them")
        );
        assert!(rejected.git.as_deref().unwrap().contains("[rejected]"));

        // "Force (with lease)…": force without confirm. The local remote-tracking branch is old (the
        // remote has 20, here it has not arrived yet): the lease stops it.
        let force = sync.force().unwrap();
        assert_eq!(force.params(), json!({ "branch": "main", "force": true }));
        let received = run(&r, &mut s, &mut sync, force);
        assert!(matches!(received, Received::Failed { .. }), "{received:?}");
        let Some(Window::Rejected(stale)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(stale.stale());
        assert!(stale.explanation.contains("Fetch and check"));
        assert_eq!(sync.force(), None, "after the stale lease no force");

        // After the fetch the force would remove 20: the engine asks for confirmation.
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        assert!(matches!(received, Received::Failed { .. }));
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
        assert!(outcome.notice.contains("refs/project/backup/push-1"));
        assert!(!sync.modal_open());
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
        let outcome = serde_json::from_value::<PushResult>(value)
            .unwrap()
            .outcome();
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
        let Some(Window::Rejected(rejected)) = sync.window() else {
            panic!()
        };
        assert_eq!(rejected.reason, PushRejection::TagExists);
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
        let Some(Window::Rejected(rejected)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(rejected.reason, PushRejection::Hook);
        assert_eq!(rejected.git.as_deref(), Some("pre-push: tests failed"));
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
            running: None,
            window: Some(Window::ConfirmForce(ConfirmForce {
                branch: Some("main".into()),
                reason: "old reason".into(),
                expected: Some(oid(11)),
                changed: false,
            })),
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
