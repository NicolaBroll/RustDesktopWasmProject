//! Reset of the current branch (S45) from the commit menu: "Reset `<branch>` to here…" opens a
//! dialog with the three modes (`soft`, `mixed`, `hard`). Every mode chosen asks the engine for the
//! plan (`branch.reset_preview`, read-only): the files a `hard` overwrites, the commits already
//! published, the refusals. "Reset" sends `branch.reset` with the `expected` of the preview, and
//! `"confirm": true` only when the plan shown needs it (a `hard` that overwrites local changes).
//! Data only, no egui. Command contract in `impl-docs/git/reset.md`.

use crate::merge::{Outcome, Received, reread_warning};
use crate::{Action, Done, ErrorKind, Request, Tab};
use project_domain::Oid;
use project_engine::{ResetPreviewResult, ResetResult};
use project_i18n::tr;
use serde_json::{Value, json};

/// The read-only plan for the dialog.
pub const PREVIEW: &str = "branch.reset_preview";
/// The command that moves the branch.
pub const COMMAND: &str = "branch.reset";

/// What the reset touches besides the ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only the ref.
    Soft,
    /// The ref and the index.
    Mixed,
    /// The ref, the index and the tracked files of the worktree.
    Hard,
}

impl Mode {
    /// The modes in the order of the dialog.
    pub const ALL: [Mode; 3] = [Mode::Soft, Mode::Mixed, Mode::Hard];

    /// The value of the `mode` parameter (git's name, the same in every language).
    pub fn param(self) -> &'static str {
        match self {
            Mode::Soft => "soft",
            Mode::Mixed => "mixed",
            Mode::Hard => "hard",
        }
    }

    /// What the mode does, next to its name in the dialog.
    pub fn hint(self) -> String {
        match self {
            Mode::Soft => tr!("reset-mode-soft-hint"),
            Mode::Mixed => tr!("reset-mode-mixed-hint"),
            Mode::Hard => tr!("reset-mode-hard-hint"),
        }
    }
}

/// Where the dialog stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// `branch.reset_preview` sent for the chosen mode.
    Loading,
    /// The plan is shown: "Reset" sends it.
    Ready(ResetPreviewResult),
    /// The engine refuses this reset (operation in progress, files in the way, ...): the reason.
    Refused(String),
    /// `branch.reset` sent.
    Sending(ResetPreviewResult),
}

/// The reset dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct ResetDialog {
    /// The commit to reset to.
    pub target: Oid,
    /// How to show it (branch, tag or abbreviated id).
    pub what: String,
    /// The branch that moves (short name); `None` with a detached HEAD.
    pub branch: Option<String>,
    pub mode: Mode,
    pub phase: Phase,
    /// The refusal of the last "Reset" (HEAD or the files changed meanwhile): the plan is
    /// reread and shown under it.
    pub error: Option<String>,
    /// Number of the last preview asked: older answers are ignored.
    seq: u64,
}

impl ResetDialog {
    /// The name of what moves: the branch, or HEAD when detached.
    pub fn moving(&self) -> String {
        match &self.branch {
            Some(branch) => tr!("reset-branch-name", branch = branch),
            None => tr!("reset-detached-name"),
        }
    }

    /// True if "Reset" can be pressed.
    pub fn can_submit(&self) -> bool {
        matches!(self.phase, Phase::Ready(_))
    }

    /// The description of the write, e.g. "Hard reset of “main” to 1a2b3c4".
    pub fn label(&self) -> String {
        tr!(
            "reset-label",
            mode = self.mode.param(),
            moving = self.moving(),
            id = self.target.short()
        )
    }

    fn preview_params(&self) -> Value {
        json!({ "target": self.target.to_string(), "mode": self.mode.param() })
    }
}

/// The reset dialog and its commands. One at a time.
#[derive(Debug, Default)]
pub struct ResetBox {
    dialog: Option<ResetDialog>,
    seq: u64,
}

impl ResetBox {
    pub fn dialog(&self) -> Option<&ResetDialog> {
        self.dialog.as_ref()
    }

    /// True while the dialog is open: no other writes, keys only to it.
    pub fn modal_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// Opens the dialog for resetting `branch` (`None`: detached HEAD) to `target`, in `mixed`
    /// mode (git's default): the parameters of the preview and its number. `None` if it is
    /// already open.
    pub fn open(
        &mut self,
        target: Oid,
        what: String,
        branch: Option<String>,
    ) -> Option<(Value, u64)> {
        if self.dialog.is_some() {
            return None;
        }
        self.dialog = Some(ResetDialog {
            target,
            what,
            branch,
            mode: Mode::Mixed,
            phase: Phase::Loading,
            error: None,
            seq: 0,
        });
        self.reload()
    }

    /// Another mode: its preview (`None` if the mode is the same or the reset was sent).
    pub fn set_mode(&mut self, mode: Mode) -> Option<(Value, u64)> {
        let dialog = self.dialog.as_mut()?;
        if dialog.mode == mode || matches!(dialog.phase, Phase::Sending(_)) {
            return None;
        }
        dialog.mode = mode;
        dialog.error = None;
        self.reload()
    }

    /// Asks the preview again for the current mode.
    fn reload(&mut self) -> Option<(Value, u64)> {
        self.seq += 1;
        let dialog = self.dialog.as_mut()?;
        dialog.seq = self.seq;
        dialog.phase = Phase::Loading;
        Some((dialog.preview_params(), self.seq))
    }

    /// The answer of preview number `seq`; ignored if it is not the last one asked.
    pub fn receive_preview(&mut self, seq: u64, result: Result<Value, String>) {
        let Some(dialog) = self.dialog.as_mut() else {
            return;
        };
        if dialog.seq != seq || dialog.phase != Phase::Loading {
            return;
        }
        dialog.phase = match result {
            Ok(value) => match crate::decode::<ResetPreviewResult>(PREVIEW, value) {
                Ok(preview) => Phase::Ready(preview),
                Err(message) => Phase::Refused(message),
            },
            Err(e) => Phase::Refused(e),
        };
    }

    /// "Reset": the parameters of `branch.reset` and the description of the write. `confirm`
    /// only if the plan shown (the files listed) needs it.
    pub fn submit(&mut self) -> Option<(Value, String)> {
        let dialog = self.dialog.as_mut()?;
        let Phase::Ready(preview) = &dialog.phase else {
            return None;
        };
        let mut params = dialog.preview_params();
        params["expected"] = json!(preview.expected.to_string());
        if preview.needs_confirm {
            params["confirm"] = json!(true);
        }
        dialog.phase = Phase::Sending(preview.clone());
        dialog.error = None;
        Some((params, dialog.label()))
    }

    /// Closes the dialog without resetting (not while the reset is running).
    pub fn close(&mut self) {
        if !matches!(
            self.dialog.as_ref().map(|d| &d.phase),
            Some(Phase::Sending(_))
        ) {
            self.dialog = None;
        }
    }

    /// The response of `branch.reset`. A refusal (HEAD moved since the preview, new local changes,
    /// files in the way) keeps the dialog with the reason and gives the preview to reread
    /// (`Some`); a git error closes it (something may have changed: everything is reread).
    pub fn receive(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> (Received, Option<(Value, u64)>) {
        let Some(dialog) = self
            .dialog
            .as_mut()
            .filter(|d| matches!(d.phase, Phase::Sending(_)))
        else {
            let message = tr!("common-unexpected-response", command = COMMAND);
            return (
                Received::Failed {
                    message,
                    reload: true,
                },
                None,
            );
        };
        match result {
            Err(e)
                if matches!(
                    kind,
                    Some(
                        ErrorKind::ConfirmRequired
                            | ErrorKind::NotAllowed
                            | ErrorKind::InvalidParams
                            | ErrorKind::UnknownRev
                            | ErrorKind::IndexLocked
                    )
                ) =>
            {
                // Nothing changed: the plan seen is no longer valid, it is reread.
                dialog.error = Some(tr!("reset-refused", error = detail.unwrap_or(e)));
                (Received::Inline, self.reload())
            }
            Err(e) => {
                let mut message = tr!("common-failed", label = dialog.label(), error = &e);
                if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
                    message.push_str(&format!(" ({})", detail.trim()));
                }
                self.dialog = None;
                (
                    Received::Failed {
                        message,
                        reload: true,
                    },
                    None,
                )
            }
            Ok(value) => {
                let dialog = self.dialog.take().expect("checked above");
                let received = match crate::decode::<ResetResult>(COMMAND, value) {
                    Ok(result) => Received::Done(Box::new(outcome(result, &dialog))),
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                };
                (received, None)
            }
        }
    }
}

/// The outcome of `branch.reset`.
fn outcome(r: ResetResult, dialog: &ResetDialog) -> Outcome {
    let mut notice = tr!(
        "reset-done",
        mode = dialog.mode.param(),
        moving = dialog.moving(),
        id = r.after.short()
    );
    if let Some(backup) = &r.backup {
        let stash = r.stash.map(|s| s.short()).unwrap_or_default();
        notice.push_str(&tr!(
            "reset-done-backup",
            n = r.files.len(),
            backup = backup,
            stash = stash
        ));
    }
    if let Some(backup) = &r.index_backup {
        notice.push_str(&tr!("reset-done-index-backup", backup = backup));
    }
    if !r.backups.is_empty() {
        notice.push_str(&tr!(
            "reset-done-commit-backups",
            refs = r.backups.join(", ")
        ));
    }
    // The warnings (commits already published: the next push needs the lease) stay in the
    // notice: the error line is cleared by the reads that follow (log, refs).
    notice.push_str(&crate::messages_text(&r.warnings));
    let warning = reread_warning(&tr!("reset-done-short"), &[], r.status_error, r.refs_error);
    Outcome {
        notice,
        status: r.status,
        refs: r.refs,
        history: r.journal.into(),
        warning,
        moved: r.after != r.before,
        show_changes: false,
    }
}

impl Tab {
    /// The actions of the reset dialog, routed by [`Tab::apply`].
    pub(crate) fn apply_reset(&mut self, action: Action) {
        let generation = self.generation;
        let writing = self.executor.writing().is_some();
        let Some(repo) = &mut self.repo else {
            return;
        };
        let preview = match action {
            Action::OpenReset { target, what } => {
                if writing || repo.modal_open() {
                    return;
                }
                let branch = match &repo.head {
                    project_domain::Head::Branch { name, .. } => Some(name.clone()),
                    project_domain::Head::Detached(_) => None,
                };
                repo.reset.open(target, what, branch)
            }
            Action::ResetMode(mode) => repo.reset.set_mode(mode),
            Action::SubmitReset => {
                if writing {
                    return;
                }
                if let Some((params, label)) = repo.reset.submit() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(COMMAND, params, Request::Reset(generation));
                }
                None
            }
            Action::CloseReset => {
                repo.reset.close();
                None
            }
            _ => unreachable!("action routed to the wrong function"),
        };
        if let Some((params, seq)) = preview {
            self.submit(PREVIEW, params, Request::ResetPreview(generation, seq));
        }
    }

    /// Response of `branch.reset_preview`: the plan in the dialog.
    pub(crate) fn receive_reset_preview(&mut self, generation: u64, seq: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        if let Err(e) = &done.result {
            log::info!("{}: {e}", done.command);
        }
        repo.reset.receive_preview(seq, done.result);
    }

    /// Response of `branch.reset`: done like a merge (status, refs and log reread, the new HEAD
    /// selected); a refusal stays in the dialog, whose plan is reread.
    pub(crate) fn receive_reset(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let (received, preview) =
            repo.reset
                .receive(done.result, done.error_kind, done.error_detail);
        self.apply_received(generation, &done.command, received);
        if let Some((params, seq)) = preview {
            self.submit(PREVIEW, params, Request::ResetPreview(generation, seq));
        }
    }
}

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;
