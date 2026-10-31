//! The stashes from the "Stash" section of the refs column (S28): `stash.list`, `stash.push`,
//! `stash.apply`, `stash.pop`, `stash.drop`. Data only, no egui: tested with unit tests.
//!
//! The list comes from `stash.list`, read on open (and with "Refresh", after undo and redo) and
//! taken from the command responses. Each stash is drawn in the graph next to its `base` commit
//! (the list's `repo.log` asks for `"stashes": true`, so that commit is always there).
//!
//! - **Push**: a dialog with an optional message, "include untracked" and "keep the
//!   index"; an engine refusal (nothing to stash, conflicts, operation in progress)
//!   stays in the dialog. It enters the journal: "Undo" reapplies it.
//! - **Apply/pop**: always with `index` and `expected` (the id seen): if the list changed the
//!   engine refuses and the list is reread. They do not enter the journal: the notice does not offer
//!   "Undo" (which would concern the previous operation). With non-empty `conflicts` it is a
//!   partial success: a window opens with the files, to be resolved from the stage area.
//! - **Drop**: in two steps like removing a remote: first without `confirm` (the engine
//!   always answers with `ConfirmRequired` and what would be lost), then, only after the click in the
//!   red dialog, `"confirm": true`.

use crate::discard::{History, Phase};
use crate::messages_text;
use crate::running::Running;
use crate::{Done, Request, Tab, params};
use crate::{ErrorKind, Ticket, format};
use project_domain::{Oid, Signature, Status};
use project_engine::{
    Message, StashApplyParams, StashApplyResult, StashDropParams, StashDropResult, StashEntry,
    StashListResult, StashPush, StashPushResult,
};
use project_i18n::tr;
use serde_json::{Value, json};

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Opens the "Stash" dialog to set changes aside.
    Open,
    /// The fields of the "Stash" dialog changed.
    Fields(PushFields),
    /// Submits the "Stash" dialog (`stash.push`).
    Submit,
    /// Selects the stash with this id: its files on the right, its base in the graph.
    Select(Oid),
    /// Applies the stash with this id (`true`: pop, i.e. apply and, if all goes well, delete
    /// it), with `index` and `expected` from the list seen.
    Apply(Oid, bool),
    /// "Also restore the stage" for apply and pop.
    RestoreIndex(bool),
    /// Deletes the stash with this id: first without confirmation, the engine always asks and
    /// the dialog opens.
    Drop(Oid),
    /// Confirms the pending deletion: resends with `"confirm": true`.
    ConfirmDrop,
    /// Closes the open stash dialog without sending anything.
    CloseDialog,
    /// Closes the apply or pop conflicts window.
    DismissConflicts,
}

/// The id of the command that lists the stashes.
pub const LIST: &str = "stash.list";

/// A command that changes the stashes (or the files, for apply and pop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Push,
    Apply,
    Pop,
    Drop,
}

impl Op {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            Op::Push => "stash.push",
            Op::Apply => "stash.apply",
            Op::Pop => "stash.pop",
            Op::Drop => "stash.drop",
        }
    }
}

/// The texts of a stash ([`StashEntry`], as given by `stash.list`) shown by the UI.
pub trait StashText {
    /// The message without the `On <branch>: ` / `WIP on <branch>: ` prefix, which the row
    /// shows separately.
    fn title(&self) -> &str;
    /// Date of the stash, e.g. `2026-10-23 09:12` (UTC).
    fn date(&self) -> String;
    /// Branch and base: `on main · 1a2b3c4`, `detached HEAD · 1a2b3c4`.
    fn origin(&self) -> String;
    /// The text of the row tooltip.
    fn tooltip(&self) -> String;
}

impl StashText for StashEntry {
    fn title(&self) -> &str {
        let message = self.message.as_str();
        for prefix in ["On ", "WIP on "] {
            if let Some(rest) = message.strip_prefix(prefix)
                && let Some((_, title)) = rest.split_once(": ")
            {
                return title;
            }
        }
        message
    }

    fn date(&self) -> String {
        format::date_time(&Signature {
            name: String::new(),
            email: String::new(),
            time: self.time,
            offset_min: 0,
        })
    }

    fn origin(&self) -> String {
        match &self.branch {
            Some(branch) => tr!(
                "stash-origin-branch",
                branch = branch,
                base = self.base.short()
            ),
            None => tr!("stash-origin-detached", base = self.base.short()),
        }
    }

    fn tooltip(&self) -> String {
        let created = tr!(
            "stash-tooltip-created",
            base = self.base.to_string(),
            date = self.date()
        );
        let mut lines = vec![
            self.name.clone(),
            self.message.clone(),
            self.id.to_string(),
            created,
        ];
        if self.untracked {
            lines.push(tr!("stash-tooltip-untracked"));
        }
        lines.push(tr!("stash-tooltip-click"));
        lines.push(tr!("stash-tooltip-right"));
        lines.join("\n")
    }
}

/// The fields of the "Stash" dialog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushFields {
    pub message: String,
    /// "Include untracked files" (`include_untracked`).
    pub include_untracked: bool,
    /// "Keep the index" (`keep_index`).
    pub keep_index: bool,
}

/// The "Stash" dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDialog {
    pub fields: PushFields,
    /// The engine error at the last submit, shown in the dialog.
    pub error: Option<String>,
    /// The running command.
    pub running: Option<Running>,
}

impl PushDialog {
    /// The message as the engine wants it: one line, no leading or trailing spaces (empty
    /// = git writes `WIP on ...`). Pasted newlines become spaces: the engine would
    /// refuse them.
    pub fn message(&self) -> String {
        self.fields
            .message
            .split(['\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The parameters of `stash.push`.
    pub fn params(&self) -> Value {
        let message = self.message();
        params(StashPush {
            message: (!message.is_empty()).then_some(message),
            include_untracked: self.fields.include_untracked,
            keep_index: self.fields.keep_index,
            paths: None,
        })
    }
}

/// The drop of a stash, from the check to the response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropDialog {
    pub stash: StashEntry,
    pub phase: Phase,
}

/// The open stash dialog (or the drop being checked, still without a dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Push(PushDialog),
    Drop(DropDialog),
}

/// Apply or pop stopped on conflicts: the stash is partly applied, the files are to be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflicts {
    pub op: Op,
    /// `stash@{n}` as it was when it was applied.
    pub stash: String,
    pub files: Vec<String>,
}

/// The stash list, the selected stash and the dialogs: one dialog at a time.
#[derive(Debug, Default)]
pub struct StashBox {
    /// The last list read; `None` until it arrives.
    list: Option<Vec<StashEntry>>,
    /// The error of the last list read.
    list_error: Option<String>,
    /// The stash shown on the right in place of the commit details (by id: the position
    /// changes at every push and drop).
    selected: Option<Oid>,
    dialog: Option<Dialog>,
    /// The apply or pop in progress (the stash seen), for the messages.
    applying: Option<(Op, StashEntry)>,
    /// "Restore the index too" for apply and pop (`restore_index`).
    pub restore_index: bool,
    /// The conflicts window after apply or pop.
    conflicts: Option<Conflicts>,
    /// The stashes already applied by a pop that then failed to remove them from the list: the
    /// changes are in the files, the pop must not be repeated (Drop stays).
    popped: Vec<Oid>,
}

/// What came back from a stash command.
#[derive(Debug)]
pub enum Received {
    /// Done: what to update in the view.
    Done(Box<Outcome>),
    /// Refused: the error is in the stash dialog, which stays open.
    Inline,
    /// The drop confirmation is needed: the dialog opens.
    Confirm,
    /// Refused or failed: the message to show; with `reload` everything is reread,
    /// otherwise only the stash list and the journal.
    Failed { message: String, reload: bool },
}

/// The result of a successful command, for the view.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub op: Op,
    pub notice: String,
    /// The new status, if the command reread it (absent for drop, which does not touch files).
    pub status: Option<Status>,
    /// The journal, for the commands that enter it (push and drop); apply and pop do not change it.
    pub history: Option<History>,
    /// A problem after the successful operation (list or status not reread).
    pub warning: Option<String>,
    /// True if the log must be reread: the set of stash bases changed (push, drop, successful
    /// pop), and a base that no branch contains leaves the graph.
    pub reload_log: bool,
}

impl StashBox {
    /// The stash list, if it arrived.
    pub fn list(&self) -> Option<&[StashEntry]> {
        self.list.as_deref()
    }

    /// The error of the last list read.
    pub fn list_error(&self) -> Option<&str> {
        self.list_error.as_deref()
    }

    /// The response of `stash.list`.
    pub fn receive_list(&mut self, result: Result<Value, String>) {
        match result.and_then(|value| crate::decode::<StashListResult>(LIST, value)) {
            Ok(list) => self.set_list(list.stashes),
            Err(e) => self.list_error = Some(e),
        }
    }

    /// A new list: a selected stash that is no longer there is deselected.
    fn set_list(&mut self, stashes: Vec<StashEntry>) {
        if let Some(id) = self.selected
            && !stashes.iter().any(|s| s.id == id)
        {
            self.selected = None;
        }
        self.popped.retain(|id| stashes.iter().any(|s| s.id == *id));
        self.list = Some(stashes);
        self.list_error = None;
    }

    /// True if the stash `id` was already applied by a pop that did not remove it from the list:
    /// Pop is no longer offered (it would reapply the same changes), Drop stays.
    pub fn pop_blocked(&self, id: Oid) -> bool {
        self.popped.contains(&id)
    }

    /// The selected stash, if it is still in the list.
    pub fn selected(&self) -> Option<&StashEntry> {
        let id = self.selected?;
        self.list.iter().flatten().find(|s| s.id == id)
    }

    /// Selects the stash `id` (or none).
    pub fn select(&mut self, id: Option<Oid>) {
        self.selected = id.filter(|id| self.list.iter().flatten().any(|s| s.id == *id));
    }

    /// The stash from the list with this id (the position may have changed).
    pub fn find(&self, id: Oid) -> Option<&StashEntry> {
        self.list.iter().flatten().find(|s| s.id == id)
    }

    /// The dialog to draw: the drop is only visible when it asks for confirmation.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Drop(drop) => matches!(drop.phase, Phase::Confirm { .. }),
            Dialog::Push(_) => true,
        })
    }

    /// True if a dialog is open: no other writes, the keys go to it.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// The conflicts window of apply or pop, if it is open.
    pub fn conflicts(&self) -> Option<&Conflicts> {
        self.conflicts.as_ref()
    }

    /// Closes the conflicts window.
    pub fn dismiss(&mut self) {
        self.conflicts = None;
    }

    /// Opens the "Stash" dialog with empty fields.
    pub fn open_push(&mut self) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        self.dialog = Some(Dialog::Push(PushDialog {
            fields: PushFields::default(),
            error: None,
            running: None,
        }));
        true
    }

    /// The fields of the "Stash" dialog changed.
    pub fn set_fields(&mut self, fields: PushFields) {
        if let Some(Dialog::Push(dialog)) = &mut self.dialog
            && dialog.running.is_none()
        {
            dialog.fields = fields;
        }
    }

    /// Submit of the "Stash" dialog: parameters and description. `None` if it cannot be done.
    pub fn submit_push(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Push(dialog)) = &mut self.dialog else {
            return None;
        };
        if dialog.running.is_some() {
            return None;
        }
        dialog.error = None;
        Some((dialog.params(), tr!("stash-push-label")))
    }

    /// The command of [`StashBox::submit_push`] started with `ticket`.
    pub fn push_started(&mut self, ticket: Ticket) {
        if let Some(Dialog::Push(dialog)) = &mut self.dialog {
            dialog.running = Some(Running::new(ticket));
        }
    }

    /// Starts the apply (`pop`: false) or the pop of the stash `id`: parameters and description.
    /// `None` if the stash is not in the list or another apply is in progress.
    pub fn start_apply(&mut self, id: Oid, pop: bool) -> Option<(Value, String)> {
        if self.applying.is_some() || self.dialog.is_some() {
            return None;
        }
        if pop && self.pop_blocked(id) {
            return None;
        }
        let stash = self.find(id)?.clone();
        let op = if pop { Op::Pop } else { Op::Apply };
        let params = params(StashApplyParams {
            index: stash.index,
            expected: Some(stash.id.to_string()),
            restore_index: self.restore_index,
        });
        let label = match op {
            Op::Pop => tr!("stash-pop-label", name = &stash.name),
            _ => tr!("stash-apply-label", name = &stash.name),
        };
        self.conflicts = None;
        self.applying = Some((op, stash));
        Some((params, label))
    }

    /// Starts the drop of the stash `id`: the parameters of the first submit, without `confirm`
    /// (the engine answers with what would be lost).
    pub fn start_drop(&mut self, id: Oid) -> Option<(Value, String)> {
        if self.dialog.is_some() || self.applying.is_some() {
            return None;
        }
        let stash = self.find(id)?.clone();
        let params = params(StashDropParams {
            index: stash.index,
            expected: Some(stash.id.to_string()),
            confirm: false,
        });
        let label = tr!("stash-drop-check-label", name = &stash.name);
        self.dialog = Some(Dialog::Drop(DropDialog {
            stash,
            phase: Phase::Checking,
        }));
        Some((params, label))
    }

    /// The user confirmed the drop: the parameters with `"confirm": true`.
    pub fn confirm_drop(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Drop(drop)) = &mut self.dialog else {
            return None;
        };
        if !matches!(drop.phase, Phase::Confirm { .. }) {
            return None;
        }
        drop.phase = Phase::Sending;
        Some((
            params(StashDropParams {
                index: drop.stash.index,
                expected: Some(drop.stash.id.to_string()),
                confirm: true,
            }),
            tr!("stash-drop-label", name = &drop.stash.name),
        ))
    }

    /// Closes the open dialog without sending anything; not during a submit.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Push(dialog)) => dialog.running.is_none(),
            Some(Dialog::Drop(drop)) => matches!(drop.phase, Phase::Confirm { .. }),
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
            Op::Push => self.receive_push(result, kind),
            Op::Apply | Op::Pop => self.receive_apply(op, result, kind),
            Op::Drop => self.receive_drop(result, kind, detail),
        }
    }

    fn receive_push(&mut self, result: Result<Value, String>, kind: Option<ErrorKind>) -> Received {
        let Some(Dialog::Push(dialog)) = &mut self.dialog else {
            return unexpected(Op::Push);
        };
        if dialog.running.is_none() {
            return unexpected(Op::Push);
        }
        match result {
            // Nothing to stash, conflicts, invalid message: nothing changed,
            // it is fixed in the dialog.
            Err(e) if matches!(kind, Some(ErrorKind::InvalidParams | ErrorKind::NotAllowed)) => {
                dialog.running = None;
                dialog.error = Some(e);
                Received::Inline
            }
            Err(e) => {
                // A git error: the stash may exist, everything is reread.
                self.dialog = None;
                Received::Failed {
                    message: tr!("stash-push-failed", error = e),
                    reload: true,
                }
            }
            Ok(value) => {
                self.dialog = None;
                match crate::decode::<StashPushResult>(Op::Push.command(), value) {
                    Ok(result) => {
                        let warning = reread_warning(
                            &tr!("stash-push-done-what"),
                            result.stashes_error.clone(),
                            result.status_error.clone(),
                        );
                        if let Some(list) = result.stashes {
                            self.set_list(list);
                        }
                        let name = &result.stash.name;
                        let title = result.stash.title();
                        let notice = if result.stash.untracked {
                            tr!("stash-push-done-untracked", name = name, title = title)
                        } else {
                            tr!("stash-push-done", name = name, title = title)
                        };
                        Received::Done(Box::new(Outcome {
                            op: Op::Push,
                            notice,
                            status: result.status,
                            history: Some(result.journal.into()),
                            warning,
                            reload_log: true,
                        }))
                    }
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                }
            }
        }
    }

    fn receive_apply(
        &mut self,
        op: Op,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
    ) -> Received {
        let Some((sent, stash)) = self.applying.take() else {
            return unexpected(op);
        };
        if sent != op {
            return unexpected(op);
        }
        let what = match op {
            Op::Pop => tr!("stash-pop-label", name = &stash.name),
            _ => tr!("stash-apply-label", name = &stash.name),
        };
        match result {
            // Local changes on the stash files, list changed, conflicts open: nothing
            // changed. The list is reread (if it changed, we see where the stash is).
            Err(e) if matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams)) => {
                Received::Failed {
                    message: tr!("stash-op-not-allowed", label = &what, error = e),
                    reload: false,
                }
            }
            Err(e) => Received::Failed {
                message: tr!("stash-op-failed", label = &what, error = e),
                reload: true,
            },
            Ok(value) => match crate::decode::<StashApplyResult>(op.command(), value) {
                Ok(result) => {
                    let mut warning = reread_warning(
                        &tr!("stash-op-done-what", label = &what),
                        result.stashes_error.clone(),
                        result.status_error.clone(),
                    );
                    // Pop applied but the stash was not removed: it is said prominently and the pop
                    // is no longer offered for that stash.
                    if op == Op::Pop && !result.dropped && result.conflicts.is_empty() {
                        if !self.popped.contains(&stash.id) {
                            self.popped.push(stash.id);
                        }
                        let text = tr!("stash-pop-not-dropped-warning", name = &stash.name)
                            + &messages_text(&result.warnings);
                        warning = Some(match warning {
                            Some(w) => format!("{text}; {w}"),
                            None => text,
                        });
                    }
                    if let Some(list) = result.stashes.clone() {
                        self.set_list(list);
                    }
                    if !result.conflicts.is_empty() {
                        self.conflicts = Some(Conflicts {
                            op,
                            stash: stash.name.clone(),
                            files: result.conflicts.clone(),
                        });
                    }
                    let notice = apply_notice(&result, op, &stash);
                    Received::Done(Box::new(Outcome {
                        op,
                        notice,
                        status: result.status,
                        history: None,
                        warning,
                        reload_log: result.dropped,
                    }))
                }
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }

    fn receive_drop(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Drop(drop)) = self.dialog.take() else {
            return unexpected(Op::Drop);
        };
        match result {
            Err(_) if kind == Some(ErrorKind::ConfirmRequired) && drop.phase == Phase::Checking => {
                self.dialog = Some(Dialog::Drop(DropDialog {
                    stash: drop.stash,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                }));
                Received::Confirm
            }
            Err(e) => {
                let refused =
                    matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams));
                let name = &drop.stash.name;
                let message = if refused {
                    tr!("stash-drop-not-allowed", name = name, error = &e)
                } else {
                    tr!("stash-drop-failed", name = name, error = &e)
                };
                Received::Failed {
                    message,
                    reload: !refused,
                }
            }
            Ok(value) => match crate::decode::<StashDropResult>(Op::Drop.command(), value) {
                Ok(result) => {
                    let warning = reread_warning(
                        &tr!("stash-drop-done-what"),
                        result.stashes_error.clone(),
                        None,
                    );
                    if let Some(list) = result.stashes {
                        self.set_list(list);
                    }
                    let mut notice = tr!(
                        "stash-drop-done",
                        name = &result.stash.name,
                        title = result.stash.title(),
                        backup = result.backup.as_deref().unwrap_or_default()
                    );
                    notice.push_str(&messages_text(&result.warnings));
                    Received::Done(Box::new(Outcome {
                        op: Op::Drop,
                        notice,
                        status: None,
                        history: Some(result.journal.into()),
                        warning,
                        reload_log: true,
                    }))
                }
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

/// The problem after a successful operation: list or status not reread.
fn reread_warning(what: &str, stashes: Option<String>, status: Option<String>) -> Option<String> {
    let parts: Vec<String> = [
        stashes.map(|e| tr!("stash-reread-stashes", error = e)),
        status.map(|e| tr!("stash-reread-status", error = e)),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| tr!("stash-reread-but", what = what, problems = parts.join("; ")))
}

/// The message for the status bar after `stash.apply`/`stash.pop`. They do not enter the
/// journal: it is never said that "Undo" undoes them (it would undo the previous operation).
fn apply_notice(r: &StashApplyResult, op: Op, stash: &StashEntry) -> String {
    let name = &stash.name;
    let mut notice = match (op, r.conflicts.len(), r.dropped) {
        (Op::Pop, 0, true) => match &r.backup {
            Some(backup) => tr!("stash-pop-done-backup", name = name, backup = backup),
            None => tr!("stash-pop-done", name = name),
        },
        (Op::Pop, 0, false) => tr!("stash-pop-not-dropped", name = name),
        (_, 0, _) => tr!("stash-apply-done", name = name),
        (_, n, _) => tr!("stash-apply-conflicts", name = name, n = n),
    };
    notice.push_str(&tr!("stash-not-undoable"));
    // The engine warning about a pop with conflicts repeats what the window explains.
    let warnings: Vec<Message> = r
        .warnings
        .iter()
        .filter(|w| !(!r.conflicts.is_empty() && w.key() == "engine-stash-kept-conflicts"))
        .cloned()
        .collect();
    notice.push_str(&messages_text(&warnings));
    notice
}

impl Tab {
    /// The actions of this function, routed by [`Tab::apply`].
    pub(crate) fn apply_stash(&mut self, action: Action) {
        match action {
            Action::Open => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.stash.open_push();
                }
            }
            Action::Fields(fields) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.set_fields(fields);
                }
            }
            Action::Submit => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.stash.submit_push() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::Push;
                    let ticket = self.submit(op.command(), params, Request::Stash(generation, op));
                    if let Some(repo) = &mut self.repo {
                        repo.stash.push_started(ticket);
                    }
                }
            }
            Action::Select(id) => {
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let Some(base) = repo.stash.find(id).map(|s| s.base) else {
                    return;
                };
                // The base is selected in the graph (brought into view), the stash on the right.
                self.seek(base);
                if let Some(repo) = &mut self.repo {
                    repo.worktree_selected = false;
                    repo.compare.close();
                    repo.stash.select(Some(id));
                }
            }
            Action::Apply(id, pop) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                if let Some((params, label)) = repo.stash.start_apply(id, pop) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = if pop { Op::Pop } else { Op::Apply };
                    self.submit(op.command(), params, Request::Stash(generation, op));
                }
            }
            Action::RestoreIndex(on) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.restore_index = on;
                }
            }
            Action::Drop(id) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                if let Some((params, label)) = repo.stash.start_drop(id) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::Drop;
                    self.submit(op.command(), params, Request::Stash(generation, op));
                }
            }
            Action::ConfirmDrop => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.stash.confirm_drop() {
                    repo.write_label = Some(label);
                    let op = Op::Drop;
                    self.submit(op.command(), params, Request::Stash(generation, op));
                }
            }
            Action::CloseDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.close();
                }
            }
            Action::DismissConflicts => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.dismiss();
                }
            }
        }
    }

    /// Response of `stash.*`: list, status and journal from the response, the stash labels
    /// in the graph redone and, if the bases changed, the log reread. A refused push stays
    /// in its dialog; the conflicts of apply and pop open their window.
    pub(crate) fn receive_stash(&mut self, generation: u64, op: Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .stash
            .receive(op, done.result, done.error_kind, done.error_detail)
        {
            Received::Inline | Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                repo.update_labels();
                if let Some(history) = outcome.history {
                    repo.history = history;
                }
                let status_read = outcome.status.is_some();
                if let Some(status) = outcome.status {
                    repo.set_status(status);
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                self.follow_diff();
                // The drop does not touch files; the others do, and if the status did not arrive it is
                // reread.
                if !status_read && outcome.op != Op::Drop {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                }
                if outcome.reload_log
                    && let Some(repo) = self.current(generation)
                {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    // The list may have changed (the stash seen moved): it is reread.
                    self.submit(LIST, json!({}), Request::Stashes(generation));
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, ChangedFile, Commit, EntryKind, RepoPath};
    use project_engine::{
        GitRead, GitStash, MemoryBackend, MemoryOpener, Registry, Session, StashPush,
    };
    use std::sync::Arc;

    fn opened(repo: MemoryBackend) -> (Registry, Session) {
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
    fn run(
        registry: &Registry,
        session: &mut Session,
        b: &mut StashBox,
        op: Op,
        params: Value,
    ) -> Received {
        let result = registry.run(op.command(), session, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        b.receive(op, result.map_err(|e| e.text()), kind, detail)
    }

    fn list(registry: &Registry, session: &mut Session, b: &mut StashBox) {
        let result = registry.run(LIST, session, json!({}));
        b.receive_list(result.map_err(|e| e.text()));
    }

    fn push(registry: &Registry, session: &mut Session, b: &mut StashBox, fields: PushFields) {
        assert!(b.open_push());
        b.set_fields(fields);
        let (params, _) = b.submit_push().unwrap();
        b.push_started(Ticket::for_test(1));
        let r = run(registry, session, b, Op::Push, params);
        assert!(matches!(r, Received::Done(_)), "{r:?}");
    }

    #[test]
    fn title_and_origin() {
        let stash = |message: &str, branch: Option<&str>| StashEntry {
            index: 0,
            name: "stash@{0}".into(),
            id: Oid::from_bytes([1; 20]),
            base: Oid::from_bytes([2; 20]),
            message: message.into(),
            branch: branch.map(Into::into),
            time: 0,
            untracked: true,
        };
        let s = stash("On main: test: colon", Some("main"));
        assert_eq!(s.title(), "test: colon");
        assert_eq!(s.origin(), "on main · 0202020");
        assert_eq!(s.date(), "1970-01-01 00:00");
        assert!(s.tooltip().contains("untracked files"));
        let s = stash("WIP on (no branch): 1a2b3c4 Subject", None);
        assert_eq!(s.title(), "1a2b3c4 Subject");
        assert_eq!(s.origin(), "detached HEAD · 0202020");
        assert_eq!(stash("saved by hand", None).title(), "saved by hand");
    }

    #[test]
    fn a_pop_that_could_not_drop_is_not_offered_again() {
        let stash = StashEntry {
            index: 0,
            name: "stash@{0}".into(),
            id: Oid::from_bytes([1; 20]),
            base: Oid::from_bytes([2; 20]),
            message: "On main: test".into(),
            branch: Some("main".into()),
            time: 0,
            untracked: false,
        };
        let mut b = StashBox::default();
        b.set_list(vec![stash.clone()]);
        let (_, _) = b.start_apply(stash.id, true).unwrap();
        let value = json!({
            "stash": stash,
            "conflicts": [],
            "dropped": false,
            "backup": null,
            "warnings": ["stash `stash@{0}` was applied but not removed from the list (broken): \
                          the changes are in the files; drop it with `stash.drop` (do not repeat \
                          the pop)"],
            "stashes": [stash],
            "status": null,
            "status_error": "x",
        });
        let Received::Done(outcome) = b.receive(Op::Pop, Ok(value), None, None) else {
            panic!()
        };
        assert!(
            outcome.notice.starts_with(
                "stash@{0} applied, but not removed from the list: don't repeat the pop"
            ),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("(broken)"),
            "the engine warning stays"
        );
        let warning = outcome.warning.unwrap();
        assert!(warning.contains("Don't repeat the pop"), "{warning}");
        assert!(!outcome.reload_log);
        assert!(b.pop_blocked(stash.id));
        assert_eq!(b.start_apply(stash.id, true), None, "no second pop");
        assert!(b.start_drop(stash.id).is_some(), "Drop stays");
    }

    #[test]
    fn push_dialog_params() {
        let mut d = PushDialog {
            fields: PushFields {
                message: "  first line\n second \r\n".into(),
                include_untracked: true,
                keep_index: false,
            },
            error: None,
            running: None,
        };
        assert_eq!(
            d.params(),
            json!({
                "message": "first line second",
                "include_untracked": true,
                "keep_index": false,
                "paths": null,
            })
        );
        d.fields.message = "   ".into();
        d.fields.keep_index = true;
        assert_eq!(
            d.params(),
            json!({ "message": null, "include_untracked": true, "keep_index": true, "paths": null }),
            "empty = absent"
        );
    }

    #[test]
    fn push_then_apply_keeps_the_stash() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        assert!(b.list().is_none());
        list(&registry, &mut session, &mut b);
        assert_eq!(b.list(), Some(&[][..]));

        assert!(b.open_push());
        assert!(!b.open_push(), "one dialog at a time");
        assert!(b.modal_open());
        b.set_fields(PushFields {
            message: "test".into(),
            include_untracked: true,
            keep_index: false,
        });
        let (params, label) = b.submit_push().unwrap();
        b.push_started(Ticket::for_test(1));
        assert_eq!(label, "Stashing changes");
        assert_eq!(b.submit_push(), None, "only once");
        b.close();
        assert!(b.modal_open(), "it does not close while sending");
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Push, params) else {
            panic!()
        };
        assert_eq!(
            outcome.notice,
            "Changes stashed in stash@{0} (“test”, with untracked files). \
             “Undo” brings them back"
        );
        assert!(outcome.history.as_ref().unwrap().can_undo);
        assert!(outcome.status.unwrap().entries.is_empty(), "clean worktree");
        assert!(outcome.reload_log);
        assert!(b.dialog().is_none());
        let stash = b.list().unwrap()[0].clone();
        assert_eq!(stash.name, "stash@{0}");
        assert!(stash.untracked);

        // Selection by id.
        b.select(Some(stash.id));
        assert_eq!(b.selected(), Some(&stash));
        b.select(Some(Oid::from_bytes([9; 20])));
        assert_eq!(b.selected(), None, "only a stash from the list");
        b.select(Some(stash.id));

        let (params, label) = b.start_apply(stash.id, false).unwrap();
        assert_eq!(label, "Applying stash@{0}");
        assert_eq!(
            params,
            json!({ "index": 0, "expected": stash.id.to_string(), "restore_index": false })
        );
        assert_eq!(b.start_apply(stash.id, true), None, "one at a time");
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Apply, params)
        else {
            panic!()
        };
        assert_eq!(
            outcome.notice,
            "stash@{0} applied; it stays in the list. “Undo” doesn't revert it"
        );
        assert!(
            outcome.history.is_none(),
            "apply does not enter the journal"
        );
        assert!(!outcome.reload_log);
        assert!(!outcome.status.unwrap().entries.is_empty());
        assert_eq!(b.list().unwrap().len(), 1);
        assert_eq!(b.selected(), Some(&stash), "the stash stays selected");
        assert!(b.conflicts().is_none());
    }

    #[test]
    fn push_with_nothing_to_save_stays_in_the_dialog() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        // Only the untracked files remain: without "include untracked" there is nothing.
        b.open_push();
        let (params, _) = b.submit_push().unwrap();
        b.push_started(Ticket::for_test(1));
        let r = run(&registry, &mut session, &mut b, Op::Push, params);
        assert!(matches!(r, Received::Inline), "{r:?}");
        let Some(Dialog::Push(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(dialog.error.is_some());
        assert!(dialog.running.is_none());
        b.close();
        assert!(b.dialog().is_none());
    }

    #[test]
    fn pop_drops_and_tells_the_backup() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let stash = b.list().unwrap()[0].clone();
        b.select(Some(stash.id));
        b.restore_index = true;
        let (params, label) = b.start_apply(stash.id, true).unwrap();
        assert_eq!(label, "Popping stash@{0}");
        assert_eq!(params["restore_index"], true);
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Pop, params) else {
            panic!()
        };
        assert!(
            outcome
                .notice
                .starts_with("stash@{0} applied and dropped from the list; backup in refs/"),
            "{}",
            outcome.notice
        );
        assert!(outcome.notice.ends_with("“Undo” doesn't revert it"));
        assert!(outcome.reload_log);
        assert!(outcome.history.is_none());
        assert_eq!(b.list(), Some(&[][..]));
        assert_eq!(
            b.selected(),
            None,
            "the dropped stash is no longer selected"
        );
    }

    #[test]
    fn apply_of_a_changed_list_is_refused() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let old = b.list().unwrap()[0].clone();
        // Another stash arrives on top without the UI knowing.
        registry
            .run(
                "stash.push",
                &mut session,
                json!({ "include_untracked": true }),
            )
            .unwrap();
        let (params, _) = b.start_apply(old.id, false).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Apply, params)
        else {
            panic!()
        };
        assert!(
            message.starts_with("Applying stash@{0} not possible: "),
            "{message}"
        );
        assert!(message.contains("is now `stash@{1}`"), "{message}");
        assert!(!reload);
        // With the list reread, the same stash applies from its new position.
        list(&registry, &mut session, &mut b);
        let (params, _) = b.start_apply(old.id, false).unwrap();
        assert_eq!(params["index"], 1);
        assert!(matches!(
            run(&registry, &mut session, &mut b, Op::Apply, params),
            Received::Done(_)
        ));
    }

    /// The sample repo with the stashed changes, then a commit that changes
    /// `README.md` on `main`: the stash goes into conflict on that file.
    fn moved_head() -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        let old = match &repo.status().unwrap().entries[0].kind {
            EntryKind::Ordinary(info) => info.oid_head,
            other => panic!("{other:?}"),
        };
        repo.stash_push(&StashPush::default()).unwrap();
        let head = repo.head().unwrap().target().unwrap();
        let new = MemoryBackend::fake_oid(4242);
        repo.set_blob(new, "README changed on main\n");
        let id = MemoryBackend::fake_oid(4243);
        let who = Signature {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 1_800_000_000,
            offset_min: 0,
        };
        repo.insert_commit(Commit {
            id,
            parents: vec![head],
            author: who.clone(),
            committer: who,
            summary: "README".to_owned(),
            body: String::new(),
        });
        repo.set_changes(
            id,
            vec![ChangedFile {
                path: RepoPath::from("README.md"),
                change: Change::Modified,
                original: None,
                score: None,
                old_mode: 0o100644,
                new_mode: 0o100644,
                old_oid: old,
                new_oid: new,
            }],
        );
        repo.set_branch("main", id);
        repo
    }

    #[test]
    fn conflicts_open_the_window_and_pop_keeps_the_stash() {
        let (registry, mut session) = opened(moved_head());
        let mut b = StashBox::default();
        list(&registry, &mut session, &mut b);
        let stash = b.list().unwrap()[0].clone();
        let (params, _) = b.start_apply(stash.id, true).unwrap();
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Pop, params) else {
            panic!()
        };
        assert_eq!(
            outcome.notice,
            "stash@{0} applied with 1 conflicted file: resolve it and stage it; the stash \
             stays in the list. “Undo” doesn't revert it"
        );
        assert!(
            !outcome.reload_log,
            "the stash stays: the bases do not change"
        );
        let status = outcome.status.unwrap();
        assert!(
            status
                .entries
                .iter()
                .any(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        );
        let conflicts = b.conflicts().unwrap();
        assert_eq!(conflicts.op, Op::Pop);
        assert_eq!(conflicts.stash, "stash@{0}");
        assert_eq!(conflicts.files, ["README.md"]);
        assert_eq!(
            b.list().unwrap().len(),
            1,
            "a pop with conflicts does not drop"
        );
        b.dismiss();
        assert!(b.conflicts().is_none());

        // With the conflict open another apply is refused: the reason in the status bar.
        let (params, _) = b.start_apply(stash.id, false).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Apply, params)
        else {
            panic!()
        };
        assert!(message.contains("conflicted files"), "{message}");
        assert!(!reload);
    }

    #[test]
    fn drop_asks_for_confirmation_and_undo_restores() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let stash = b.list().unwrap()[0].clone();
        let (params, label) = b.start_drop(stash.id).unwrap();
        assert_eq!(
            params,
            json!({ "index": 0, "expected": stash.id.to_string() }),
            "first without confirm"
        );
        assert_eq!(label, "Checking before dropping stash@{0}");
        assert!(!b.modal_open(), "checking: no dialog");
        assert_eq!(b.start_drop(stash.id), None);
        let r = run(&registry, &mut session, &mut b, Op::Drop, params);
        assert!(matches!(r, Received::Confirm), "{r:?}");
        let Some(Dialog::Drop(drop)) = b.dialog() else {
            panic!()
        };
        let Phase::Confirm { reason } = &drop.phase else {
            panic!()
        };
        assert!(reason.contains("stash@{0}"), "{reason}");
        // Nothing changed until confirmed; "Don't drop" closes.
        list(&registry, &mut session, &mut b);
        assert_eq!(b.list().unwrap().len(), 1);
        b.close();
        assert!(b.dialog().is_none());

        let (params, _) = b.start_drop(stash.id).unwrap();
        run(&registry, &mut session, &mut b, Op::Drop, params);
        let (params, label) = b.confirm_drop().unwrap();
        assert_eq!(params["confirm"], true);
        assert_eq!(params["expected"], stash.id.to_string());
        assert_eq!(label, "Dropping stash@{0}");
        assert_eq!(b.confirm_drop(), None, "only once");
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Drop, params) else {
            panic!()
        };
        assert!(
            outcome.notice.starts_with("stash@{0} (“"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome
                .notice
                .contains("backup in refs/project/backup/stash-")
        );
        assert!(
            outcome
                .notice
                .contains("“Undo” puts it back at the top of the list")
        );
        assert!(outcome.history.unwrap().can_undo);
        assert!(outcome.status.is_none());
        assert_eq!(b.list(), Some(&[][..]));

        registry.run("edit.undo", &mut session, json!({})).unwrap();
        list(&registry, &mut session, &mut b);
        assert_eq!(b.list().unwrap()[0].id, stash.id);
    }

    #[test]
    fn drop_of_a_missing_stash_is_refused() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let stash = b.list().unwrap()[0].clone();
        registry
            .run(
                "stash.drop",
                &mut session,
                json!({ "index": 0, "expected": stash.id.to_string(), "confirm": true }),
            )
            .unwrap();
        let (params, _) = b.start_drop(stash.id).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Drop, params)
        else {
            panic!()
        };
        assert!(
            message.starts_with("Dropping stash@{0} not possible"),
            "{message}"
        );
        assert!(!reload);
        assert!(b.dialog().is_none());
    }
}
