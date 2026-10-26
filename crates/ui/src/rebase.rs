//! Interactive rebase editor (S33): from the commit menu, "Interactive rebase from here…" reads the
//! default plan (`rebase.plan`, base = parent of the chosen commit) and opens an editor where the
//! steps are reordered by dragging, combined (`squash`/`fixup` into the commit **above**), dropped
//! or reworded, with the preview of the commits the plan produces ([`RebasePlan::result`]) and the
//! errors of [`RebasePlan::validate`]. "Run" sends `rebase.start`; commits already published
//! need the confirmation (`ConfirmRequired` → dialog with the engine's reason → `"confirm": true`).
//! Data only, no egui. Command contract in `impl-docs/git/rebase.md`.
//!
//! The steps are **oldest first**, as in the git todo: the order of application.
//! Stopped on a conflict, the rebase stays in progress and the panel of the operation (`merge.rs`,
//! `Kind::Rebase`) finishes or aborts it.

use crate::merge::{Outcome, Received, message_texts, reread_warning};
use crate::{Action, Done, ErrorKind, Request, Tab};
use project_domain::{Oid, PlanErrorReason, RebaseAction, RebasePlan, RebaseStep};
use project_engine::{RebasePlanResult, RebaseStartResult};
use project_i18n::tr;
use serde_json::{Value, json};

/// The command that reads the default plan.
pub const PLAN: &str = "rebase.plan";
/// The command that runs the plan.
pub const START: &str = "rebase.start";

/// The actions offered for a step, in the order of the selector.
pub const ACTIONS: [RebaseAction; 5] = [
    RebaseAction::Pick,
    RebaseAction::Reword,
    RebaseAction::Squash,
    RebaseAction::Fixup,
    RebaseAction::Drop,
];

/// The name of an action, as in git (the same in every language).
pub fn action_name(action: RebaseAction) -> &'static str {
    match action {
        RebaseAction::Pick => "pick",
        RebaseAction::Reword => "reword",
        RebaseAction::Squash => "squash",
        RebaseAction::Fixup => "fixup",
        RebaseAction::Drop => "drop",
    }
}

/// What an action does, for the selector's hover.
pub fn action_hint(action: RebaseAction) -> String {
    match action {
        RebaseAction::Pick => tr!("rebase-action-pick-hint"),
        RebaseAction::Reword => tr!("rebase-action-reword-hint"),
        RebaseAction::Squash => tr!("rebase-action-squash-hint"),
        RebaseAction::Fixup => tr!("rebase-action-fixup-hint"),
        RebaseAction::Drop => tr!("rebase-action-drop-hint"),
    }
}

/// One step of the plan being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub commit: Oid,
    /// The first line of the original message.
    pub summary: String,
    pub action: RebaseAction,
    /// The message typed for this step: kept when the action changes, sent only where the plan
    /// allows it ([`Editor::message_allowed`]) and if not blank.
    pub message: String,
    /// Reachable from a remote-tracking branch: rewriting it needs the confirmation.
    pub published: bool,
}

/// Where the editor stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// `rebase.plan` sent.
    Loading,
    /// The plan could not be read (the reason is in [`Editor::error`]): only "Close".
    Unavailable,
    Editing,
    /// `rebase.start` answered `ConfirmRequired`: the dialog with the engine's reason.
    Confirm {
        reason: String,
    },
    /// `rebase.start` sent.
    Sending,
}

/// The state of the plan for the "Run" button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Ready,
    /// All `pick` in the original order: nothing to do.
    Noop,
    /// The error of [`RebasePlan::validate`]: the step (0-based) and the text.
    Invalid {
        step: Option<usize>,
        text: String,
    },
}

/// Where the message of a resulting commit comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageSource {
    /// The original of the picked commit (or of the first, for a group with only `fixup`).
    Original,
    /// Git joins the messages of the group (`squash` without a message).
    Combined,
    /// Typed in the editor.
    New,
}

/// A commit the plan produces, for the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// The commits it is made of: the picked one, then those squashed into it.
    pub sources: Vec<Oid>,
    /// The first line of its message (for a combined message, the summaries joined by ` + `).
    pub summary: String,
    pub source: MessageSource,
}

/// The editor of one plan.
#[derive(Debug, Clone)]
pub struct Editor {
    pub base: Oid,
    /// The chosen commit (the first rewritten), as shown in the menu.
    pub what: String,
    /// The short name of the branch rewritten.
    pub branch: String,
    pub rows: Vec<Row>,
    /// The commits of the range in the original order.
    original: Vec<Oid>,
    /// The row whose message is edited below the list.
    pub selected: Option<usize>,
    pub phase: Phase,
    /// The plan could not be read, or the engine refused the run (nothing changed).
    pub error: Option<String>,
}

impl Editor {
    /// True if the step `i` takes a message: `reword`, or the last `squash`/`fixup` of a group.
    pub fn message_allowed(&self, i: usize) -> bool {
        let Some(row) = self.rows.get(i) else {
            return false;
        };
        match row.action {
            RebaseAction::Reword => true,
            RebaseAction::Squash | RebaseAction::Fixup => !self
                .rows
                .iter()
                .skip(i + 1)
                .find(|r| r.action != RebaseAction::Drop)
                .is_some_and(|r| r.action.is_combine()),
            RebaseAction::Pick | RebaseAction::Drop => false,
        }
    }

    /// The plan as it would be sent.
    pub fn plan(&self) -> RebasePlan {
        let steps = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, row)| RebaseStep {
                action: row.action,
                commit: row.commit,
                message: (self.message_allowed(i) && !row.message.trim().is_empty())
                    .then(|| row.message.clone()),
            })
            .collect();
        RebasePlan {
            base: self.base,
            steps,
        }
    }

    /// Valid, unchanged or invalid (with the reason in the UI language).
    pub fn check(&self) -> Check {
        let plan = self.plan();
        if let Err(e) = plan.validate(&self.original) {
            return Check::Invalid {
                step: e.step,
                text: reason_text(&e.reason, e.step),
            };
        }
        if plan.is_noop(&self.original) {
            return Check::Noop;
        }
        Check::Ready
    }

    /// The commits the plan produces, oldest first; empty if the plan is not valid.
    pub fn preview(&self) -> Vec<Preview> {
        let plan = self.plan();
        if plan.validate(&self.original).is_err() {
            return Vec::new();
        }
        let row = |id: &Oid| self.rows.iter().find(|r| r.commit == *id);
        plan.result()
            .into_iter()
            .map(|commit| {
                let (summary, source) = match &commit.message {
                    Some(message) => (
                        message.lines().next().unwrap_or_default().to_owned(),
                        MessageSource::New,
                    ),
                    None => {
                        let squashed = commit
                            .sources
                            .iter()
                            .any(|id| row(id).is_some_and(|r| r.action == RebaseAction::Squash));
                        if squashed {
                            let summaries: Vec<&str> = commit
                                .sources
                                .iter()
                                .filter(|id| {
                                    row(id).is_some_and(|r| r.action != RebaseAction::Fixup)
                                })
                                .filter_map(|id| row(id).map(|r| r.summary.as_str()))
                                .collect();
                            (summaries.join(" + "), MessageSource::Combined)
                        } else {
                            let first = commit.sources.first().and_then(row);
                            (
                                first.map(|r| r.summary.clone()).unwrap_or_default(),
                                MessageSource::Original,
                            )
                        }
                    }
                };
                Preview {
                    sources: commit.sources,
                    summary,
                    source,
                }
            })
            .collect()
    }

    /// Moves the step `from` to the position `to` (0..=len, counted before the removal: `to` is
    /// the gap above row `to`); the selection follows the moved row.
    fn move_step(&mut self, from: usize, to: usize) {
        let len = self.rows.len();
        if from >= len || to > len || to == from || to == from + 1 {
            return;
        }
        let row = self.rows.remove(from);
        let at = if to > from { to - 1 } else { to };
        self.rows.insert(at, row);
        self.selected = Some(at);
    }

    fn set_action(&mut self, i: usize, action: RebaseAction) {
        let Some(row) = self.rows.get_mut(i) else {
            return;
        };
        row.action = action;
        // A reword starts from the original summary.
        if action == RebaseAction::Reword && row.message.trim().is_empty() {
            row.message = row.summary.clone();
        }
        if self.message_allowed(i) {
            self.selected = Some(i);
        }
    }
}

/// The text of a plan error; `step` 0-based.
fn reason_text(reason: &PlanErrorReason, step: Option<usize>) -> String {
    let n = step.map_or(0, |s| s + 1);
    match reason {
        PlanErrorReason::UnknownCommit(id) => tr!("rebase-error-unknown", id = id.short()),
        PlanErrorReason::DuplicateCommit(id) => tr!("rebase-error-duplicate", id = id.short()),
        PlanErrorReason::MissingCommit(id) => tr!("rebase-error-missing", id = id.short()),
        PlanErrorReason::AllDropped => tr!("rebase-error-all-dropped"),
        PlanErrorReason::NothingToCombine => tr!("rebase-error-nothing-to-combine", n = n),
        PlanErrorReason::MissingMessage => tr!("rebase-error-missing-message", n = n),
        PlanErrorReason::UnexpectedMessage => tr!("rebase-error-unexpected-message", n = n),
        PlanErrorReason::RewordInGroup => tr!("rebase-error-reword-in-group", n = n),
        PlanErrorReason::EmptyMessage => tr!("rebase-error-empty-message", n = n),
        PlanErrorReason::NulInMessage => tr!("rebase-error-nul", n = n),
    }
}

/// The interactive rebase editor, at most one.
#[derive(Debug, Default)]
pub struct RebaseBox {
    editor: Option<Editor>,
}

impl RebaseBox {
    /// The open editor.
    pub fn editor(&self) -> Option<&Editor> {
        self.editor.as_ref()
    }

    /// True while the editor is open: no other writes, the keys go to it.
    pub fn modal_open(&self) -> bool {
        self.editor.is_some()
    }

    /// Opens the editor for the commits after `base` (the parent of the chosen commit, shown as
    /// `what`): the parameters of `rebase.plan`. `None` if it is already open.
    pub fn open(&mut self, base: Oid, what: String) -> Option<Value> {
        if self.editor.is_some() {
            return None;
        }
        self.editor = Some(Editor {
            base,
            what,
            branch: String::new(),
            rows: Vec::new(),
            original: Vec::new(),
            selected: None,
            phase: Phase::Loading,
            error: None,
        });
        Some(json!({ "base": base.to_string() }))
    }

    /// The response of `rebase.plan`: the rows, or the reason (e.g. merges in the range).
    pub fn receive_plan(&mut self, result: Result<Value, String>) {
        let Some(editor) = self.editor.as_mut().filter(|e| e.phase == Phase::Loading) else {
            return;
        };
        let plan = result.and_then(|value| crate::decode::<RebasePlanResult>(PLAN, value));
        match plan {
            Ok(plan) => {
                editor.branch = plan.branch;
                editor.original = plan.steps.iter().map(|s| s.commit).collect();
                editor.rows = plan
                    .steps
                    .into_iter()
                    .map(|s| Row {
                        published: plan.published.contains(&s.commit),
                        commit: s.commit,
                        summary: s.summary,
                        action: RebaseAction::Pick,
                        message: String::new(),
                    })
                    .collect();
                editor.phase = Phase::Editing;
            }
            Err(e) => {
                editor.error = Some(e);
                editor.phase = Phase::Unavailable;
            }
        }
    }

    /// The editor while it can be edited.
    fn editing(&mut self) -> Option<&mut Editor> {
        self.editor.as_mut().filter(|e| e.phase == Phase::Editing)
    }

    /// Drag and drop (or the arrows): moves step `from` into the gap above row `to` (`len` = at
    /// the bottom).
    pub fn move_step(&mut self, from: usize, to: usize) {
        if let Some(editor) = self.editing() {
            editor.move_step(from, to);
            editor.error = None;
        }
    }

    pub fn set_action(&mut self, i: usize, action: RebaseAction) {
        if let Some(editor) = self.editing() {
            editor.set_action(i, action);
            editor.error = None;
        }
    }

    pub fn set_message(&mut self, i: usize, message: String) {
        if let Some(row) = self.editing().and_then(|e| e.rows.get_mut(i)) {
            row.message = message;
        }
    }

    pub fn select(&mut self, i: usize) {
        if let Some(editor) = self.editing()
            && i < editor.rows.len()
        {
            editor.selected = Some(i);
        }
    }

    /// "Run": the parameters of `rebase.start` and the description, if the plan is valid and
    /// changes something.
    pub fn run(&mut self) -> Option<(Value, String)> {
        let editor = self.editing()?;
        if editor.check() != Check::Ready {
            return None;
        }
        editor.phase = Phase::Sending;
        editor.error = None;
        Some(Self::params(editor, false))
    }

    /// The confirmation for the published commits: sent again with `"confirm": true`.
    pub fn confirm(&mut self) -> Option<(Value, String)> {
        let editor = self
            .editor
            .as_mut()
            .filter(|e| matches!(e.phase, Phase::Confirm { .. }))?;
        editor.phase = Phase::Sending;
        Some(Self::params(editor, true))
    }

    /// "Back to the plan" from the confirmation: nothing sent.
    pub fn cancel_confirm(&mut self) {
        if let Some(editor) = self
            .editor
            .as_mut()
            .filter(|e| matches!(e.phase, Phase::Confirm { .. }))
        {
            editor.phase = Phase::Editing;
        }
    }

    /// Closes the editor (the plan is lost); not while the run is waiting for its answer.
    pub fn close(&mut self) {
        if self
            .editor
            .as_ref()
            .is_some_and(|e| e.phase != Phase::Sending)
        {
            self.editor = None;
        }
    }

    fn params(editor: &Editor, confirm: bool) -> (Value, String) {
        let plan = editor.plan();
        let mut params = json!({
            "base": plan.base.to_string(),
            "steps": serde_json::to_value(&plan.steps).unwrap_or_default(),
        });
        if confirm {
            params["confirm"] = json!(true);
        }
        (params, tr!("rebase-label", branch = &editor.branch))
    }

    /// The response of `rebase.start`.
    pub fn receive_start(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(editor) = self.editor.as_mut().filter(|e| e.phase == Phase::Sending) else {
            return Received::Failed {
                message: tr!("common-unexpected-response", command = START),
                reload: true,
            };
        };
        match result {
            Err(e) if kind == Some(ErrorKind::ConfirmRequired) => {
                editor.phase = Phase::Confirm {
                    reason: detail.unwrap_or(e),
                };
                Received::Confirm
            }
            // Refusals (local changes, plan that changes nothing, merges in the range, ...):
            // nothing changed, the editor stays with the reason.
            Err(e)
                if matches!(
                    kind,
                    Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams | ErrorKind::UnknownRev)
                ) =>
            {
                editor.phase = Phase::Editing;
                editor.error = Some(tr!("rebase-refused", error = &e));
                Received::Inline
            }
            // Git failed: something may have changed, the plan is no longer the one seen.
            Err(e) => {
                let mut message = tr!("rebase-failed", branch = &editor.branch, error = &e);
                if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
                    message.push_str(&format!(" ({})", detail.trim()));
                }
                self.editor = None;
                Received::Failed {
                    message,
                    reload: true,
                }
            }
            Ok(value) => {
                let rows = editor.rows.len();
                let produced = editor.plan().result().len();
                self.editor = None;
                match serde_json::from_value::<RebaseStartResult>(value) {
                    Ok(result) => Received::Done(Box::new(start_outcome(result, rows, produced))),
                    Err(e) => Received::Failed {
                        message: crate::unexpected_result(START, e),
                        reload: true,
                    },
                }
            }
        }
    }
}

/// The outcome of `rebase.start`: `rows` the commits of the range, `produced` those the plan
/// produces.
fn start_outcome(r: RebaseStartResult, rows: usize, produced: usize) -> Outcome {
    let branch = &r.branch;
    let mut warnings = message_texts(&r.warnings);
    let notice = if r.in_progress {
        match r.conflicts.len() {
            0 => {
                let message = r.message.trim();
                if !message.is_empty() {
                    warnings.push(message.to_owned());
                }
                tr!("rebase-stopped-before-commit", branch = branch)
            }
            n => tr!(
                "rebase-stopped",
                branch = branch,
                files = tr!("merge-conflict-files", n = n)
            ),
        }
    } else {
        tr!(
            "rebase-done",
            branch = branch,
            before = rows,
            after = produced,
            id = r.after.short(),
            backup = &r.backup
        )
    };
    let what = if r.in_progress {
        tr!("rebase-stopped-short")
    } else {
        tr!("merge-done", title = "Rebase")
    };
    let warning = reread_warning(&what, &warnings, r.status_error, r.refs_error);
    Outcome {
        notice,
        show_changes: r.in_progress && !r.conflicts.is_empty(),
        status: r.status,
        refs: r.refs,
        history: r.journal.into(),
        warning,
        // Stopped, HEAD is detached on the commits applied so far.
        moved: r.in_progress || r.after != r.before,
    }
}

#[cfg(test)]
#[path = "rebase_tests.rs"]
mod tests;

impl Tab {
    /// The actions of the rebase editor, routed by [`Tab::apply`].
    pub(crate) fn apply_rebase(&mut self, action: Action) {
        let generation = self.generation;
        let writing = self.executor.writing().is_some();
        let Some(repo) = &mut self.repo else {
            return;
        };
        match action {
            Action::OpenRebase { base, what } => {
                if writing || repo.modal_open() {
                    return;
                }
                if let Some(params) = repo.rebase.open(base, what) {
                    self.submit(PLAN, params, Request::RebasePlan(generation));
                }
            }
            Action::RebaseMove { from, to } => repo.rebase.move_step(from, to),
            Action::RebaseChoose(i, action) => repo.rebase.set_action(i, action),
            Action::RebaseMessage(i, message) => repo.rebase.set_message(i, message),
            Action::RebaseSelect(i) => repo.rebase.select(i),
            Action::RunRebase | Action::ConfirmRebase => {
                if writing {
                    return;
                }
                let sent = if matches!(action, Action::RunRebase) {
                    repo.rebase.run()
                } else {
                    repo.rebase.confirm()
                };
                if let Some((params, label)) = sent {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(START, params, Request::Rebase(generation));
                }
            }
            Action::CancelRebaseConfirm => repo.rebase.cancel_confirm(),
            Action::CloseRebase => repo.rebase.close(),
            _ => unreachable!("action routed to the wrong function"),
        }
    }

    /// Response of `rebase.start`: done or stopped like a merge (a stop opens the stage area with
    /// the conflicts and the panel of the operation); a refusal or the confirmation stay in the
    /// editor.
    pub(crate) fn receive_rebase(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received = repo
            .rebase
            .receive_start(done.result, done.error_kind, done.error_detail);
        self.apply_received(generation, &done.command, received);
    }
}
