//! Merge and in-progress operation (S29): `merge.start` from the branch and commit menus, the panel
//! of the in-progress operation (`repo.operation`) with `merge.continue` and `merge.abort`. Data only,
//! no egui: tested with unit tests. Command contract in `impl-docs/git/merge.md`.
//!
//! - **Panel**: when the repo status (`repo.status`) says there is an operation in progress
//!   (merge, rebase, cherry-pick, revert, bisect, `git am`) `repo.operation` is read: what is
//!   coming in, the conflicted files, the message prepared by git and the commands that finish or
//!   abort the operation from here (merge, cherry-pick, revert, rebase; `pick.rs`). It is reread at every new status
//!   (after every command and with "Refresh"), so resolved conflicts disappear from the list.
//! - **Start**: `merge.start` with the chosen branch name (or commit id). A finished merge
//!   (fast-forward or merge commit) enters the journal: "Undo" undoes it. A merge
//!   stopped on conflicts **stays in progress** (it is not an error): the panel appears. A refusal
//!   (local changes, detached HEAD, operation already in progress) opens a window that explains it.
//! - **Continue**: a dialog with the merge commit message, prefilled with git's;
//!   a refusal (conflicts remain, a hook) stays in the dialog.
//! - **Abort the merge**: in two steps, like dropping a stash: first without
//!   `confirm` (the engine always answers with `ConfirmRequired` and what would be lost), then, only
//!   after the click in the red dialog, `"confirm": true`.
//! - **Rebase** (S32): `rebase.continue` (no message: the commit it stopped on keeps its own; it
//!   may stop again on the next conflict, and the panel stays) and `rebase.abort` like the others.

use crate::ErrorKind;
use crate::discard::{History, Phase};
use crate::fetch::Failure;
use crate::{Action, Done, Request, Tab};
pub use pick::{MainlineDialog, Pick};
use project_domain::branch_short;
use project_domain::{Head, Oid, Operation, RefInfo, Status};
use project_engine::{
    MergeAbortResult, MergeApplied, MergeContinueResult, MergeStartResult, Message,
    RebaseContinueResult, RepoOperationResult,
};
use project_i18n::tr;
use serde_json::{Value, json};

#[path = "pick.rs"]
mod pick;

/// The id of the command that reads the in-progress operation.
pub const OPERATION: &str = "repo.operation";

/// How many conflicted files are listed in the panel; the others are seen in the stage area.
pub const BANNER_FILES: usize = 8;

/// The operation that is finished or aborted from the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Merge,
    CherryPick,
    Revert,
    /// A rebase (interactive or of a pull): "Continue" has no message, git goes on with the plan.
    Rebase,
}

impl Kind {
    const ALL: [Kind; 4] = [Kind::Merge, Kind::CherryPick, Kind::Revert, Kind::Rebase];

    /// The id of the command that finishes the operation.
    pub fn continue_command(self) -> &'static str {
        match self {
            Kind::Merge => "merge.continue",
            Kind::CherryPick => "cherry_pick.continue",
            Kind::Revert => "revert.continue",
            Kind::Rebase => "rebase.continue",
        }
    }

    /// The id of the command that aborts the operation.
    pub fn abort_command(self) -> &'static str {
        match self {
            Kind::Merge => "merge.abort",
            Kind::CherryPick => "cherry_pick.abort",
            Kind::Revert => "revert.abort",
            Kind::Rebase => "rebase.abort",
        }
    }

    /// From the `continue` id of `repo.operation`.
    pub fn from_continue(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.continue_command() == id)
    }

    /// From the `abort` id of `repo.operation`.
    pub fn from_abort(id: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.abort_command() == id)
    }

    /// The name in texts: "merge", "cherry-pick", "revert".
    pub fn name(self) -> &'static str {
        match self {
            Kind::Merge => "merge",
            Kind::CherryPick => "cherry-pick",
            Kind::Revert => "revert",
            Kind::Rebase => "rebase",
        }
    }

    /// The name capitalized, at the start of a sentence.
    pub fn title(self) -> &'static str {
        match self {
            Kind::Merge => "Merge",
            Kind::CherryPick => "Cherry-pick",
            Kind::Revert => "Revert",
            Kind::Rebase => "Rebase",
        }
    }

    /// The commit that "Continue" creates: "merge commit" or "commit".
    pub fn commit_noun(self) -> String {
        match self {
            Kind::Merge => tr!("merge-noun-merge-commit"),
            Kind::CherryPick | Kind::Revert | Kind::Rebase => tr!("merge-noun-commit"),
        }
    }

    /// True if "Continue" creates a commit with a message to write (not the rebase: the commit
    /// it stopped on keeps its message, `rebase.continue` takes no parameters).
    pub fn has_message(self) -> bool {
        self != Kind::Rebase
    }

    /// The prefix of the abort backup: `refs/project/backup/merge-abort-…`.
    pub fn abort_backup(self) -> String {
        format!("refs/project/backup/{}-abort-…", self.name())
    }
}

/// A command of merge, cherry-pick or revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Start,
    /// `commit.cherry_pick` o `commit.revert`.
    Pick(Pick),
    Continue(Kind),
    Abort(Kind),
}

impl Op {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            Op::Start => "merge.start",
            Op::Pick(pick) => pick.command(),
            Op::Continue(kind) => kind.continue_command(),
            Op::Abort(kind) => kind.abort_command(),
        }
    }
}

/// The name of the operation, for the panel: "Merge", "Rebase", ...
pub fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "Merge",
        Operation::Rebase => "Rebase",
        Operation::CherryPick => "Cherry-pick",
        Operation::Revert => "Revert",
        Operation::Bisect => "Bisect",
        Operation::Am => "`git am`",
    }
}

/// How an operation that the UI cannot yet finish is finished from the terminal.
pub fn terminal_hint(operation: Operation) -> String {
    match operation {
        Operation::Merge => tr!("merge-terminal-merge"),
        Operation::Rebase => tr!("merge-terminal-rebase"),
        Operation::CherryPick => tr!("merge-terminal-cherry-pick"),
        Operation::Revert => tr!("merge-terminal-revert"),
        Operation::Bisect => tr!("merge-terminal-bisect"),
        Operation::Am => tr!("merge-terminal-am"),
    }
}

/// What is coming in, for the panel: `"feature" (Title)`, several entries separated by commas.
pub fn incoming_text(info: &RepoOperationResult) -> Option<String> {
    if info.incoming.is_empty() {
        return None;
    }
    Some(
        info.incoming
            .iter()
            .map(|i| match &i.summary {
                Some(summary) => {
                    tr!(
                        "merge-incoming-summary",
                        label = &i.label,
                        summary = summary
                    )
                }
                None => tr!("merge-incoming", label = &i.label),
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

/// The panel row: type of operation and what is coming in (or, for a rebase, what it is
/// reapplied onto).
pub fn title(info: &RepoOperationResult) -> String {
    let Some(operation) = info.operation else {
        return String::new();
    };
    let name = operation_name(operation);
    match operation {
        Operation::Rebase => {
            let branch = info.rebase_branch.as_deref().map(branch_short);
            match (branch, info.onto.map(|o| o.short())) {
                (Some(branch), Some(onto)) => tr!(
                    "merge-title-rebase-branch-onto",
                    name = name,
                    branch = branch,
                    onto = onto
                ),
                (Some(branch), None) => {
                    tr!("merge-title-rebase-branch", name = name, branch = branch)
                }
                (None, Some(onto)) => tr!("merge-title-rebase-onto", name = name, onto = onto),
                (None, None) => tr!("merge-title", name = name),
            }
        }
        Operation::Merge => match incoming_text(info) {
            Some(incoming) => {
                tr!(
                    "merge-title-merge-incoming",
                    name = name,
                    incoming = incoming
                )
            }
            None => tr!("merge-title", name = name),
        },
        _ => match incoming_text(info) {
            Some(incoming) => tr!("merge-title-incoming", name = name, incoming = incoming),
            None => tr!("merge-title", name = name),
        },
    }
}

/// Why "Merge" cannot be offered, or which branch it would merge into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// It merges into the active branch with this name.
    Into(String),
    /// It cannot: the reason to show.
    Blocked(String),
}

impl Gate {
    /// From HEAD and the status: a merge must be done on a branch with commits, with no operation in
    /// progress and no conflicts (the engine checks again, with the local changes).
    pub fn new(head: &Head, status: Option<&Status>) -> Self {
        let branch = match head {
            Head::Branch {
                name,
                target: Some(_),
            } => name.clone(),
            Head::Branch { target: None, .. } => {
                return Gate::Blocked(tr!("merge-gate-unborn"));
            }
            Head::Detached(_) => {
                return Gate::Blocked(tr!("merge-gate-detached"));
            }
        };
        if let Some(status) = status {
            if status.operation.is_some() {
                return Gate::Blocked(tr!("merge-gate-operation"));
            }
            if status.has_conflicts() {
                return Gate::Blocked(tr!("merge-gate-conflicts"));
            }
        }
        Gate::Into(branch)
    }
}

/// The "Continue" dialog: the message of the commit that finishes the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueDialog {
    /// The operation being finished.
    pub kind: Kind,
    pub message: String,
    /// The engine error at the last submit (conflicts remain, a hook), in the dialog.
    pub error: Option<String>,
    /// The git text of the error, if any.
    pub git: Option<String>,
    /// True while the command is running.
    pub sending: bool,
}

impl ContinueDialog {
    /// True if the message is needed and empty or only whitespace: the button stays disabled.
    pub fn is_blank(&self) -> bool {
        self.kind.has_message() && self.message.trim().is_empty()
    }
}

/// The open dialog: one at a time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Continue(ContinueDialog),
    /// The abort: check, confirmation with the engine's reason, submit.
    Abort(Kind, Phase),
    /// The parent (`mainline`) for the cherry-pick or revert of a merge commit.
    Mainline(MainlineDialog),
}

/// The merge, cherry-pick or revert started and not yet answered.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Starting {
    /// What is merged or applied, as shown (branch name or abbreviated id).
    what: String,
    /// The branch it is merged into or applied onto.
    into: String,
}

/// The UI state for merge and in-progress operation.
#[derive(Debug, Default)]
pub struct MergeBox {
    /// The last read of `repo.operation`; `None` without an operation in progress.
    info: Option<RepoOperationResult>,
    /// The error of the last read.
    info_error: Option<String>,
    /// The status number for which `repo.operation` was last requested.
    asked_for: Option<u64>,
    /// True while `repo.operation` is in progress.
    reading: bool,
    starting: Option<Starting>,
    dialog: Option<Dialog>,
    /// The window of a refused or failed start.
    failure: Option<Failure>,
}

/// What came back from a merge command.
#[derive(Debug)]
pub enum Received {
    /// Done: what to update in the view.
    Done(Box<Outcome>),
    /// Refused: the error is in the "Continue" dialog, which stays open.
    Inline,
    /// The abort confirmation is needed: the dialog opens.
    Confirm,
    /// Refused or failed: the message for the status bar; with `reload` everything is
    /// reread (something may have changed).
    Failed { message: String, reload: bool },
}

/// The result of a successful command, for the view.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    pub status: Option<Status>,
    pub refs: Option<Vec<RefInfo>>,
    pub history: History,
    /// A problem after the successful operation (git warnings, status or refs not reread).
    pub warning: Option<String>,
    /// The branch moved: the log is reread and the new HEAD is selected.
    pub moved: bool,
    /// The merge stopped on conflicts: the stage area opens with the files to resolve.
    pub show_changes: bool,
}

impl MergeBox {
    /// A new repo status (number `seq`, increasing) with `operation`: without an operation the
    /// panel disappears; with an operation, if it has not yet been requested for this status,
    /// `true`: `repo.operation` must be requested (one at a time; when finished, it is requested again if in the meantime
    /// the status changed).
    pub fn want_info(&mut self, operation: Option<Operation>, seq: u64) -> bool {
        if operation.is_none() {
            self.info = None;
            self.info_error = None;
            self.asked_for = Some(seq);
            // Without an operation there is no Continue or Abort to show.
            if matches!(self.dialog, Some(Dialog::Continue(ref d)) if !d.sending)
                || matches!(self.dialog, Some(Dialog::Abort(_, Phase::Confirm { .. })))
            {
                self.dialog = None;
            }
            return false;
        }
        if self.reading || self.asked_for == Some(seq) {
            return false;
        }
        self.reading = true;
        self.asked_for = Some(seq);
        true
    }

    /// The response of `repo.operation`.
    pub fn receive_info(&mut self, result: Result<Value, String>) {
        self.reading = false;
        match result.and_then(|value| crate::decode::<RepoOperationResult>(OPERATION, value)) {
            Ok(info) => {
                self.info = info.operation.is_some().then_some(info);
                self.info_error = None;
            }
            Err(e) => self.info_error = Some(e),
        }
    }

    /// The in-progress operation read from `repo.operation`, if any.
    pub fn info(&self) -> Option<&RepoOperationResult> {
        self.info.as_ref()
    }

    /// The error of the last read of `repo.operation`.
    pub fn info_error(&self) -> Option<&str> {
        self.info_error.as_deref()
    }

    /// The refused start window, if it is open.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Closes the refused start window.
    pub fn dismiss(&mut self) {
        self.failure = None;
    }

    /// The dialog to draw: the abort is only visible when it asks for confirmation.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Abort(_, phase) => matches!(phase, Phase::Confirm { .. }),
            Dialog::Continue(_) | Dialog::Mainline(_) => true,
        })
    }

    /// True if a dialog is open: no other writes, the keys go to it.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// True if a merge command is waiting for a response.
    pub fn busy(&self) -> bool {
        self.starting.is_some()
            || match &self.dialog {
                Some(Dialog::Continue(d)) => d.sending,
                Some(Dialog::Abort(_, phase)) => !matches!(phase, Phase::Confirm { .. }),
                Some(Dialog::Mainline(_)) | None => false,
            }
    }

    /// Starts the merge of `rev` (shown as `what`) into branch `into`: parameters and
    /// description. `None` if another merge command is in progress or a dialog is open.
    pub fn start(
        &mut self,
        rev: String,
        what: String,
        into: String,
        no_ff: bool,
    ) -> Option<(Value, String)> {
        if self.busy() || self.dialog.is_some() {
            return None;
        }
        let mut params = json!({ "rev": rev });
        if no_ff {
            params["no_ff"] = json!(true);
        }
        let label = tr!("merge-start-label", what = &what, into = &into);
        self.failure = None;
        self.starting = Some(Starting { what, into });
        Some((params, label))
    }

    /// Opens the "Continue" dialog with the message prepared by git, for the operation that
    /// `repo.operation` says to finish from here (merge, cherry-pick, revert). `false` if there is
    /// none or if another dialog is open.
    pub fn open_continue(&mut self) -> bool {
        if self.dialog.is_some() || self.busy() {
            return false;
        }
        let Some(info) = &self.info else {
            return false;
        };
        let Some(kind) = info
            .continue_command
            .as_deref()
            .and_then(Kind::from_continue)
        else {
            return false;
        };
        let message = if kind.has_message() {
            info.message.clone().unwrap_or_default()
        } else {
            String::new()
        };
        self.dialog = Some(Dialog::Continue(ContinueDialog {
            kind,
            message: message.trim_end().to_owned(),
            error: None,
            git: None,
            sending: false,
        }));
        true
    }

    /// The message of the "Continue" dialog changed.
    pub fn set_message(&mut self, message: String) {
        if let Some(Dialog::Continue(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.message = message;
        }
    }

    /// Submit of the "Continue" dialog: command, parameters and description. `None` if it cannot be done
    /// (empty message, submit already in progress).
    pub fn submit_continue(&mut self) -> Option<(Op, Value, String)> {
        let Some(Dialog::Continue(dialog)) = &mut self.dialog else {
            return None;
        };
        if dialog.sending || dialog.is_blank() {
            return None;
        }
        dialog.sending = true;
        dialog.error = None;
        dialog.git = None;
        let params = if dialog.kind.has_message() {
            json!({ "message": dialog.message.trim_end() })
        } else {
            json!({})
        };
        Some((
            Op::Continue(dialog.kind),
            params,
            tr!("merge-continue-label", name = dialog.kind.name()),
        ))
    }

    /// Starts the abort: command and parameters of the first submit, without `confirm` (the
    /// engine answers with what would be lost). `None` if there is no operation to abort
    /// from here.
    pub fn start_abort(&mut self) -> Option<(Op, Value, String)> {
        if self.dialog.is_some() || self.busy() {
            return None;
        }
        let info = self.info.as_ref()?;
        let kind = info.abort_command.as_deref().and_then(Kind::from_abort)?;
        self.dialog = Some(Dialog::Abort(kind, Phase::Checking));
        Some((
            Op::Abort(kind),
            json!({}),
            tr!("merge-abort-check-label", name = kind.name()),
        ))
    }

    /// The user confirmed the abort: command and parameters with `"confirm": true`.
    pub fn confirm_abort(&mut self) -> Option<(Op, Value, String)> {
        let Some(Dialog::Abort(kind, phase)) = &mut self.dialog else {
            return None;
        };
        if !matches!(phase, Phase::Confirm { .. }) {
            return None;
        }
        *phase = Phase::Sending;
        Some((
            Op::Abort(*kind),
            json!({ "confirm": true }),
            tr!("merge-abort-label", name = kind.name()),
        ))
    }

    /// Closes the open dialog without sending anything; not during a submit.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Continue(dialog)) => !dialog.sending,
            Some(Dialog::Abort(_, phase)) => matches!(phase, Phase::Confirm { .. }),
            Some(Dialog::Mainline(_)) => true,
            None => false,
        };
        if closable {
            self.dialog = None;
        }
    }

    /// The response of command `op`.
    pub fn receive(
        &mut self,
        op: Op,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        match op {
            Op::Start => self.receive_start(result, kind, detail),
            Op::Pick(pick) => self.receive_pick(pick, result, kind, detail),
            Op::Continue(op) => self.receive_continue(op, result, kind, detail),
            Op::Abort(op) => self.receive_abort(op, result, kind, detail),
        }
    }

    fn receive_start(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(starting) = self.starting.take() else {
            return unexpected(Op::Start);
        };
        let what = tr!(
            "merge-start-label",
            what = &starting.what,
            into = &starting.into
        );
        match result {
            Err(e) => {
                // Engine refusals (local changes, detached HEAD, operation in progress,
                // rev that does not exist): nothing changed. Other errors (git): maybe, so it rereads.
                let refused = matches!(
                    kind,
                    Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams | ErrorKind::UnknownRev)
                );
                let (title, explanation) = if refused {
                    (
                        tr!("merge-refused-title", title = "Merge"),
                        tr!("merge-refused", what = &what, error = &e),
                    )
                } else {
                    (
                        tr!("merge-failed-title", title = "Merge"),
                        tr!("merge-failed", what = &what, error = &e),
                    )
                };
                let git = detail.filter(|d| kind == Some(ErrorKind::Git) && !d.trim().is_empty());
                self.failure = Some(Failure {
                    title,
                    explanation: explanation.clone(),
                    git,
                });
                Received::Failed {
                    message: explanation,
                    reload: !refused,
                }
            }
            Ok(value) => match crate::decode::<MergeStartResult>(Op::Start.command(), value) {
                Ok(result) => Received::Done(Box::new(start_outcome(result, &starting))),
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }

    fn receive_continue(
        &mut self,
        op: Kind,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Continue(dialog)) = &mut self.dialog else {
            return unexpected(Op::Continue(op));
        };
        if !dialog.sending || dialog.kind != op {
            return unexpected(Op::Continue(op));
        }
        let title = op.title();
        match result {
            // The commit is made but then something does not add up: it must not be repeated, everything is reread.
            Err(e) if kind == Some(ErrorKind::PostCommit) => {
                self.dialog = None;
                Received::Failed {
                    message: tr!("merge-concluded-but", title = title, error = &e),
                    reload: true,
                }
            }
            // Conflicts remain, no merge in progress, invalid message, a hook that
            // refuses, nothing to commit: nothing is committed, it is fixed in the dialog.
            Err(e) => {
                dialog.sending = false;
                dialog.git =
                    detail.filter(|d| kind == Some(ErrorKind::Git) && !d.trim().is_empty());
                dialog.error = Some(e);
                Received::Inline
            }
            Ok(value) if op == Kind::Rebase => {
                self.dialog = None;
                match crate::decode::<RebaseContinueResult>(Op::Continue(op).command(), value) {
                    Ok(result) => Received::Done(Box::new(rebase_continue_outcome(result))),
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                }
            }
            Ok(value) => {
                self.dialog = None;
                match crate::decode::<MergeContinueResult>(Op::Continue(op).command(), value) {
                    Ok(result) => Received::Done(Box::new(continue_outcome(result, op))),
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                }
            }
        }
    }

    fn receive_abort(
        &mut self,
        op: Kind,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let phase = match self.dialog.take() {
            Some(Dialog::Abort(k, phase)) if k == op => phase,
            other => {
                self.dialog = other;
                return unexpected(Op::Abort(op));
            }
        };
        let name = op.name();
        match result {
            Err(_) if kind == Some(ErrorKind::ConfirmRequired) && phase == Phase::Checking => {
                self.dialog = Some(Dialog::Abort(
                    op,
                    Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                ));
                Received::Confirm
            }
            Err(e) => {
                let refused =
                    matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams));
                let message = if refused {
                    tr!("merge-abort-refused", name = name, error = &e)
                } else {
                    tr!("merge-abort-failed", name = name, error = &e)
                };
                Received::Failed {
                    message,
                    reload: !refused,
                }
            }
            Ok(value) => match crate::decode::<MergeAbortResult>(Op::Abort(op).command(), value) {
                Ok(result) => Received::Done(Box::new(abort_outcome(result, op))),
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }
}

fn unexpected(op: Op) -> Received {
    Received::Failed {
        message: tr!("common-unexpected-response", command = op.command()),
        reload: true,
    }
}

/// The git warnings and the problem after a successful operation: status or refs not reread.
pub(crate) fn reread_warning(
    what: &str,
    warnings: &[String],
    status: Option<String>,
    refs: Option<String>,
) -> Option<String> {
    let parts: Vec<String> = warnings
        .iter()
        .map(|w| tr!("merge-warning", warning = w))
        .chain(status.map(|e| tr!("merge-status-not-reloaded", error = e)))
        .chain(refs.map(|e| tr!("merge-refs-not-reloaded", error = e)))
        .collect();
    (!parts.is_empty()).then(|| tr!("merge-done-but", what = what, problems = parts.join("; ")))
}

/// The engine warnings as plain texts, for [`reread_warning`].
pub(crate) fn message_texts(warnings: &[Message]) -> Vec<String> {
    warnings.iter().map(Message::text).collect()
}

/// The outcome of `merge.start`.
fn start_outcome(r: MergeStartResult, starting: &Starting) -> Outcome {
    let notice = start_notice(&r, starting);
    let warning = reread_warning(
        &tr!("merge-done", title = "Merge"),
        &message_texts(&r.warnings),
        r.status_error,
        r.refs_error,
    );
    Outcome {
        notice,
        moved: r.after != r.before,
        show_changes: r.in_progress && !r.conflicts.is_empty(),
        status: r.status,
        refs: r.refs,
        history: r.journal.into(),
        warning,
    }
}

/// The message of `merge.start` for the status bar.
fn start_notice(r: &MergeStartResult, starting: &Starting) -> String {
    let name = if r.source.name.is_empty() {
        starting.what.as_str()
    } else {
        r.source.name.as_str()
    };
    let branch = &r.branch;
    let commits = tr!("merge-commits", n = r.behind);
    if r.in_progress {
        return match r.conflicts.len() {
            0 => tr!("merge-stopped-before-commit", name = name, branch = branch),
            n => tr!(
                "merge-stopped",
                name = name,
                branch = branch,
                files = tr!("merge-conflict-files", n = n)
            ),
        };
    }
    let id = r.after.short();
    match r.strategy {
        MergeApplied::UpToDate => tr!("merge-up-to-date", branch = branch, name = name),
        MergeApplied::FastForward => tr!(
            "merge-fast-forward",
            branch = branch,
            commits = commits,
            name = name,
            id = id
        ),
        MergeApplied::Merge => tr!(
            "merge-merged",
            name = name,
            branch = branch,
            commits = commits,
            id = id
        ),
    }
}

/// The outcome of `merge.continue` (also `cherry_pick.continue` and `revert.continue`, same result).
fn continue_outcome(r: MergeContinueResult, op: Kind) -> Outcome {
    let title = op.title();
    let warning = reread_warning(
        &tr!("merge-concluded", title = title),
        &[],
        r.status_error,
        r.refs_error,
    );
    let noun = op.commit_noun();
    let id = r.commit.id.short();
    let notice = match r.branch.as_deref().map(branch_short) {
        Some(branch) => tr!(
            "merge-concluded-notice-branch",
            title = title,
            noun = noun,
            id = id,
            branch = branch
        ),
        None => tr!(
            "merge-concluded-notice",
            title = title,
            noun = noun,
            id = id
        ),
    };
    Outcome {
        notice,
        status: r.status,
        refs: r.refs,
        history: r.journal.into(),
        warning,
        moved: true,
        show_changes: false,
    }
}

/// The outcome of `merge.abort` (also the abort of cherry-pick, revert and rebase, same result).
fn abort_outcome(r: MergeAbortResult, op: Kind) -> Outcome {
    let title = op.title();
    let warning = reread_warning(
        &tr!("merge-aborted", title = title),
        &message_texts(&r.warnings),
        r.status_error,
        r.refs_error,
    );
    let notice = tr!(
        "merge-aborted-notice",
        title = title,
        backup = &r.backup,
        n = r.files.len()
    );
    Outcome {
        notice,
        status: r.status,
        refs: r.refs,
        history: r.journal.into(),
        warning,
        // The rebase abort puts HEAD back on the branch: the log changes.
        moved: op == Kind::Rebase,
        show_changes: false,
    }
}

/// The outcome of `rebase.continue`: concluded, or stopped again on the next commit (conflicts, a
/// hook): the stage area opens with the files and the panel stays.
fn rebase_continue_outcome(r: RebaseContinueResult) -> Outcome {
    let mut warnings = message_texts(&r.warnings);
    let notice = if r.in_progress {
        match r.conflicts.len() {
            0 => {
                let message = r.message.trim();
                if !message.is_empty() {
                    warnings.push(message.to_owned());
                }
                tr!("merge-rebase-stopped-before-commit")
            }
            n => tr!(
                "merge-rebase-stopped",
                files = tr!("merge-conflict-files", n = n)
            ),
        }
    } else {
        let mut notice = match &r.branch {
            Some(branch) => tr!(
                "merge-rebase-finished-branch",
                branch = branch,
                id = r.after.short()
            ),
            None => tr!("merge-rebase-finished", id = r.after.short()),
        };
        if let Some(backup) = &r.backup {
            notice.push_str(&tr!("merge-rebase-backup", backup = backup));
        }
        notice
    };
    let what = if r.in_progress {
        tr!("merge-rebase-stopped-short")
    } else {
        tr!("merge-concluded", title = Kind::Rebase.title())
    };
    let warning = reread_warning(&what, &warnings, r.status_error, r.refs_error);
    Outcome {
        notice,
        show_changes: r.in_progress && !r.conflicts.is_empty(),
        status: r.status,
        refs: r.refs,
        history: r.journal.into(),
        warning,
        // HEAD moved in any case (the commits applied, or the branch at the end).
        moved: true,
    }
}

/// The name of the commit `id` to show: the first local branch, remote branch or tag that points to it,
/// otherwise the abbreviated id. Used by the commit menu.
pub fn commit_name(id: Oid, refs: &[RefInfo]) -> String {
    use project_domain::RefKind;
    for kind in [RefKind::LocalBranch, RefKind::RemoteBranch, RefKind::Tag] {
        if let Some(r) = refs.iter().find(|r| r.kind == kind && r.target == id) {
            return r.short_name().to_owned();
        }
    }
    id.short()
}

impl Tab {
    /// The actions of this function, routed by [`Tab::apply`].
    pub(crate) fn apply_merge(&mut self, action: Action) {
        match action {
            Action::Merge { rev, what, no_ff } => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let Gate::Into(into) = Gate::new(&repo.head, repo.status.as_ref()) else {
                    return;
                };
                if let Some((params, label)) = repo.merge.start(rev, what, into, no_ff) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::Start;
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::ContinueMerge => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.merge.open_continue();
                }
            }
            Action::MergeMessage(message) => {
                if let Some(repo) = &mut self.repo {
                    repo.merge.set_message(message);
                }
            }
            Action::SubmitContinueMerge => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((op, params, label)) = repo.merge.submit_continue() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::AbortMerge => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                if let Some((op, params, label)) = repo.merge.start_abort() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::ConfirmAbortMerge => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((op, params, label)) = repo.merge.confirm_abort() {
                    repo.write_label = Some(label);
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::Pick {
                pick,
                rev,
                what,
                parents,
            } => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let Gate::Into(into) = Gate::new(&repo.head, repo.status.as_ref()) else {
                    return;
                };
                // The parents of a merge commit, with the name and subject if known.
                let parents = parents
                    .into_iter()
                    .map(|id| {
                        let name = commit_name(id, &repo.refs);
                        let summary = repo
                            .list
                            .commits()
                            .iter()
                            .find(|c| c.id == id)
                            .map(|c| c.summary.clone());
                        (id, name, summary)
                    })
                    .collect();
                if let Some((params, label)) = repo.merge.start_pick(pick, rev, what, into, parents)
                {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::Pick(pick);
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::Mainline(n) => {
                if let Some(repo) = &mut self.repo {
                    repo.merge.set_mainline(n);
                }
            }
            Action::SubmitMainline => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((op, params, label)) = repo.merge.submit_mainline() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::CloseMergeDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.merge.close();
                }
            }
            Action::DismissMergeError => {
                if let Some(repo) = &mut self.repo {
                    repo.merge.dismiss();
                }
            }
            _ => unreachable!("action routed to the wrong function"),
        }
    }

    /// Response of `merge.*`: status, refs and journal from the response (the panel is reread with the
    /// new status), then the log reread if the branch moved, with the new HEAD
    /// selected. A merge stopped on conflicts opens the stage area; a refused start opens
    /// its window, a refusal of "Continue" stays in the dialog.
    pub(crate) fn receive_merge(&mut self, generation: u64, op: Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received = repo
            .merge
            .receive(op, done.result, done.error_kind, done.error_detail);
        self.apply_received(generation, &done.command, received);
    }

    /// What came back from a merge, cherry-pick, revert or rebase command: the notice, status,
    /// refs and journal from the response, the log reread if the branch moved.
    pub(crate) fn apply_received(&mut self, generation: u64, command: &str, received: Received) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        match received {
            Received::Inline | Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            Received::Done(outcome) => {
                log::info!("{command}: {}", outcome.notice);
                repo.history = outcome.history.clone();
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
                    repo.stash.select(None);
                    repo.select_worktree(true);
                } else if outcome.moved && !repo.worktree_selected {
                    repo.select_after_load = repo.head.target();
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                self.follow_diff();
                if !refs_read {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                }
                if !status_read {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                }
                if outcome.moved
                    && let Some(repo) = self.current(generation)
                {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            Received::Failed { message, reload } => {
                log::warn!("{command}: {message}");
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, ChangedFile, Commit, RepoPath, Signature};
    use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    fn sig(n: u64) -> Signature {
        Signature {
            name: "Ada".into(),
            email: "ada@example.com".into(),
            time: 1_700_000_000 + i64::try_from(n).unwrap() * 60,
            offset_min: 0,
        }
    }

    /// The commit `n` with parent `parent` that takes `a.txt` from `old` (blob) to `new`.
    fn commit(
        repo: &mut MemoryBackend,
        n: u64,
        parent: Option<u64>,
        summary: &str,
        file: (&str, Option<u64>, u64),
    ) {
        let (path, old, new) = file;
        repo.set_blob(oid(100 + new), format!("{summary}\n"));
        repo.insert_commit(Commit {
            id: oid(n),
            parents: parent.map(oid).into_iter().collect(),
            author: sig(n),
            committer: sig(n),
            summary: summary.into(),
            body: String::new(),
        });
        repo.set_changes(
            oid(n),
            vec![ChangedFile {
                path: RepoPath::from(path),
                change: if old.is_some() {
                    Change::Modified
                } else {
                    Change::Added
                },
                original: None,
                score: None,
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid: old.map_or(Oid::from_bytes([0; Oid::LEN]), |o| oid(100 + o)),
                new_oid: oid(100 + new),
            }],
        );
    }

    /// `c1` base; `main` on `c2` (a.txt “ours”); `other` on `c3` from `c1` (a.txt “theirs”:
    /// conflict); `ahead` on `c4` from `c2` (fast-forward).
    fn opened() -> (Registry, Session) {
        let mut repo = MemoryBackend::new();
        repo.set_identity("Ada", "ada@example.com");
        commit(&mut repo, 1, None, "base", ("a.txt", None, 1));
        commit(&mut repo, 2, Some(1), "ours", ("a.txt", Some(1), 2));
        commit(&mut repo, 3, Some(1), "theirs", ("a.txt", Some(1), 3));
        commit(&mut repo, 4, Some(2), "ahead", ("b.txt", None, 4));
        repo.set_branch("main", oid(2));
        repo.set_branch("other", oid(3));
        repo.set_branch("ahead", oid(4));
        repo.set_head_branch("main");
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        (registry, session)
    }

    /// Runs `op` like the executor and passes the response to `receive`.
    fn run(r: &Registry, s: &mut Session, b: &mut MergeBox, op: Op, params: Value) -> Received {
        let result = r.run(op.command(), s, params);
        let (kind, detail) = match &result {
            Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
            Ok(_) => (None, None),
        };
        b.receive(op, result.map_err(|e| e.text()), kind, detail)
    }

    /// Rereads status and operation like the UI: `want_info` and, if needed, `repo.operation`.
    fn refresh(r: &Registry, s: &mut Session, b: &mut MergeBox, seq: u64) {
        let status: Status =
            serde_json::from_value(r.run("repo.status", s, json!({})).unwrap()).unwrap();
        if b.want_info(status.operation, seq) {
            b.receive_info(r.run(OPERATION, s, json!({})).map_err(|e| e.text()));
        }
    }

    fn done(received: Received) -> Outcome {
        match received {
            Received::Done(outcome) => *outcome,
            other => panic!("expected Done, got {other:?}"),
        }
    }

    fn start(b: &mut MergeBox, rev: &str) -> Value {
        b.start(rev.into(), rev.into(), "main".into(), false)
            .unwrap()
            .0
    }

    #[test]
    fn conflict_banner_resolution_and_conclusion() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        refresh(&r, &mut s, &mut b, 1);
        assert!(b.info().is_none());

        let params = start(&mut b, "other");
        assert!(b.busy());
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(!outcome.moved);
        assert!(outcome.show_changes);
        assert!(
            outcome.notice.contains("1 conflicted file"),
            "{}",
            outcome.notice
        );
        assert_eq!(
            outcome.status.as_ref().and_then(|s| s.operation),
            Some(Operation::Merge)
        );

        // The new status brings the panel.
        refresh(&r, &mut s, &mut b, 2);
        let info = b.info().expect("merge in progress");
        assert_eq!(info.operation, Some(Operation::Merge));
        assert_eq!(info.conflicts, vec![RepoPath::from("a.txt")]);
        assert!(!info.ready);
        assert_eq!(title(info), "Merge in progress: merging “other” (theirs)");
        assert_eq!(info.continue_command.as_deref(), Some("merge.continue"));
        // The same status is not requested again.
        assert!(!b.want_info(Some(Operation::Merge), 2));

        // With conflicts the engine refuses: the error stays in the dialog.
        assert!(b.open_continue());
        let Some(Dialog::Continue(dialog)) = b.dialog() else {
            panic!("Continue dialog");
        };
        assert!(
            dialog.message.starts_with("Merge branch 'other'"),
            "{}",
            dialog.message
        );
        let (_, params, _) = b.submit_continue().unwrap();
        assert!(matches!(
            run(&r, &mut s, &mut b, Op::Continue(Kind::Merge), params),
            Received::Inline
        ));
        let Some(Dialog::Continue(dialog)) = b.dialog() else {
            panic!("the dialog stays open");
        };
        assert!(dialog.error.as_deref().unwrap().contains("a.txt"));
        b.close();
        assert!(b.dialog().is_none());

        // Marked as resolved: ready.
        r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
            .unwrap();
        refresh(&r, &mut s, &mut b, 3);
        assert!(b.info().unwrap().ready);
        assert!(b.info().unwrap().conflicts.is_empty());

        assert!(b.open_continue());
        b.set_message("Merge other\n\nWith our version".into());
        let (op, params, label) = b.submit_continue().unwrap();
        assert_eq!(op, Op::Continue(Kind::Merge));
        assert_eq!(params["message"], "Merge other\n\nWith our version");
        assert_eq!(label, "Finishing the merge");
        let outcome = done(run(&r, &mut s, &mut b, Op::Continue(Kind::Merge), params));
        assert!(outcome.moved);
        assert!(outcome.history.can_undo);
        assert!(
            outcome.notice.starts_with("Merge finished"),
            "{}",
            outcome.notice
        );
        assert!(b.dialog().is_none());

        // Merge finished: the panel disappears.
        refresh(&r, &mut s, &mut b, 4);
        assert!(b.info().is_none());
    }

    #[test]
    fn abort_in_two_steps() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "other");
        done(run(&r, &mut s, &mut b, Op::Start, params));
        refresh(&r, &mut s, &mut b, 1);

        let (_, params, _) = b.start_abort().unwrap();
        assert_eq!(params, json!({}));
        // While it checks nothing is shown and it cannot start again.
        assert!(b.dialog().is_none());
        assert!(b.start_abort().is_none());
        assert!(matches!(
            run(&r, &mut s, &mut b, Op::Abort(Kind::Merge), params),
            Received::Confirm
        ));
        let Some(Dialog::Abort(Kind::Merge, Phase::Confirm { reason })) = b.dialog() else {
            panic!("abort confirmation");
        };
        assert!(reason.contains("a.txt"), "{reason}");
        assert!(b.modal_open());

        let (_, params, label) = b.confirm_abort().unwrap();
        assert_eq!(params, json!({ "confirm": true }));
        assert_eq!(label, "Aborting the merge");
        let outcome = done(run(&r, &mut s, &mut b, Op::Abort(Kind::Merge), params));
        assert!(
            outcome.notice.contains("refs/project/backup/merge-abort-"),
            "{}",
            outcome.notice
        );
        assert!(!outcome.moved);
        assert_eq!(outcome.status.as_ref().unwrap().operation, None);
        refresh(&r, &mut s, &mut b, 2);
        assert!(b.info().is_none());
    }

    #[test]
    fn abandoned_abort_sends_nothing() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "other");
        done(run(&r, &mut s, &mut b, Op::Start, params));
        refresh(&r, &mut s, &mut b, 1);
        let (_, params, _) = b.start_abort().unwrap();
        run(&r, &mut s, &mut b, Op::Abort(Kind::Merge), params);
        b.close();
        assert!(b.dialog().is_none());
        assert!(b.confirm_abort().is_none());
        refresh(&r, &mut s, &mut b, 2);
        assert!(b.info().is_some(), "the merge stays in progress");
    }

    #[test]
    fn fast_forward_and_already_up_to_date() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "ahead");
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(outcome.moved);
        assert!(!outcome.show_changes);
        assert!(
            outcome.notice.starts_with("Fast-forward"),
            "{}",
            outcome.notice
        );
        assert!(outcome.history.can_undo);

        let params = start(&mut b, "ahead");
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(!outcome.moved);
        assert!(
            outcome.notice.contains("nothing to merge"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn refusal_opens_the_window() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "no-such-branch");
        let Received::Failed { reload, message } = run(&r, &mut s, &mut b, Op::Start, params)
        else {
            panic!("refused");
        };
        assert!(!reload);
        assert!(message.contains("no-such-branch"), "{message}");
        let failure = b.failure().expect("window");
        assert_eq!(failure.title, "Merge not possible");
        assert!(!b.busy());
        b.dismiss();
        assert!(b.failure().is_none());

        // `no_ff`: a merge commit even where a fast-forward would be enough.
        let params = b
            .start("ahead".into(), "ahead".into(), "main".into(), true)
            .unwrap()
            .0;
        assert_eq!(params, json!({ "rev": "ahead", "no_ff": true }));
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(
            outcome.notice.contains("merge commit"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn menu_gate() {
        let head = Head::Branch {
            name: "main".into(),
            target: Some(oid(1)),
        };
        assert_eq!(Gate::new(&head, None), Gate::Into("main".into()));
        assert!(matches!(
            Gate::new(&Head::Detached(oid(1)), None),
            Gate::Blocked(_)
        ));
        let empty = Head::Branch {
            name: "main".into(),
            target: None,
        };
        assert!(matches!(Gate::new(&empty, None), Gate::Blocked(_)));
        let status = Status {
            head: head.clone(),
            upstream: None,
            entries: Vec::new(),
            operation: Some(Operation::Rebase),
        };
        assert!(matches!(Gate::new(&head, Some(&status)), Gate::Blocked(_)));
    }

    #[test]
    fn rebase_and_cherry_pick_title() {
        let info: RepoOperationResult = serde_json::from_value(json!({
            "operation": "Rebase",
            "head": { "Detached": oid(1).to_string() },
            "conflicts": ["x.txt"],
            "incoming": [],
            "message": null,
            "onto": oid(2).to_string(),
            "rebase_branch": "refs/heads/feature",
            "ready": false,
            "continue": null,
            "abort": null,
        }))
        .unwrap();
        assert_eq!(
            title(&info),
            format!("Rebase of “feature” in progress onto {}", oid(2).short())
        );
        let mut b = MergeBox {
            info: Some(info),
            ..MergeBox::default()
        };
        // Without commands from here, no dialogs.
        assert!(!b.open_continue());
        assert!(b.start_abort().is_none());

        let pick: RepoOperationResult = serde_json::from_value(json!({
            "operation": "CherryPick",
            "head": { "Detached": oid(1).to_string() },
            "conflicts": [],
            "incoming": [{ "id": oid(3).to_string(), "label": "abc1234", "summary": "Fix", "refs": [] }],
            "message": "Fix\n",
            "onto": null,
            "rebase_branch": null,
            "ready": true,
            "continue": null,
            "abort": null,
        }))
        .unwrap();
        assert_eq!(title(&pick), "Cherry-pick in progress: “abc1234” (Fix)");
    }

    #[test]
    fn commit_name_from_refs() {
        let refs = vec![RefInfo {
            name: "refs/remotes/origin/x".into(),
            kind: project_domain::RefKind::RemoteBranch,
            target: oid(5),
            tag_object: None,
            upstream: None,
        }];
        assert_eq!(commit_name(oid(5), &refs), "origin/x");
        assert_eq!(commit_name(oid(6), &refs), oid(6).short());
    }
}

#[cfg(test)]
#[path = "merge_rebase_tests.rs"]
mod rebase_tests;
