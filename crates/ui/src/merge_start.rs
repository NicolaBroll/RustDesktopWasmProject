//! The merge dialog (S58): "Merge into … with options…" from the branch and commit menus. Data
//! only, no egui. Command contract (`merge.start` with `no_ff`, `ff_only`, `squash`, `message`) in
//! `impl-docs/git/merge.md`.
//!
//! It is a part of [`MergeBox`]: the start, the window of a refusal and the panel of the operation
//! in progress are those of the merge. The dialog chooses the mode: merge commit (`no_ff`), squash
//! into one commit (with an editable message; empty, the engine writes its default) or
//! fast-forward only (`ff_only`). A squash stopped on a conflict stays in progress like a merge
//! (`Operation::Squash`, [`Kind::Squash`](super::Kind::Squash)).

use super::{Dialog, MergeBox};
use project_i18n::tr;
use serde_json::Value;

/// How `merge.start` brings the changes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Fast-forward if enough, otherwise a merge commit (the quick menu entry).
    Auto,
    /// Always a merge commit (`no_ff`).
    MergeCommit,
    /// The changes as one ordinary commit (`squash`): the branch is not recorded as merged.
    Squash,
    /// Only a fast-forward (`ff_only`): refused if the branches have diverged.
    FastForwardOnly,
}

impl Mode {
    /// The choices of the dialog, in order.
    pub const CHOICES: [Mode; 3] = [Mode::MergeCommit, Mode::Squash, Mode::FastForwardOnly];

    /// The radio label.
    pub fn label(self) -> String {
        match self {
            Mode::Auto => tr!("merge-mode-auto"),
            Mode::MergeCommit => tr!("merge-mode-merge-commit"),
            Mode::Squash => tr!("merge-mode-squash"),
            Mode::FastForwardOnly => tr!("merge-mode-ff-only"),
        }
    }

    /// One line under the radio: what happens to `into`.
    pub fn explanation(self, what: &str, into: &str) -> String {
        match self {
            Mode::Auto => tr!("merge-mode-auto-text", what = what, into = into),
            Mode::MergeCommit => tr!("merge-mode-merge-commit-text", what = what, into = into),
            Mode::Squash => tr!("merge-mode-squash-text", what = what, into = into),
            Mode::FastForwardOnly => tr!("merge-mode-ff-only-text", what = what, into = into),
        }
    }
}

/// The open merge dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartDialog {
    /// What `merge.start` receives (branch name or commit id).
    pub rev: String,
    /// How to show it.
    pub what: String,
    /// The active branch it is merged into.
    pub into: String,
    pub mode: Mode,
    /// The squash commit message; empty: the engine's default.
    pub message: String,
}

impl StartDialog {
    /// The message to send: only for the squash and only if written.
    pub fn message(&self) -> Option<String> {
        let message = self.message.trim_end();
        (self.mode == Mode::Squash && !message.trim().is_empty()).then(|| message.to_owned())
    }
}

impl MergeBox {
    /// Opens the merge dialog for `rev` (shown as `what`) into branch `into`, with "Merge commit"
    /// chosen. `false` if a command is in progress or another dialog is open.
    pub fn open_start(&mut self, rev: String, what: String, into: String) -> bool {
        if self.busy() || self.dialog.is_some() {
            return false;
        }
        self.failure = None;
        self.dialog = Some(Dialog::Start(StartDialog {
            rev,
            what,
            into,
            mode: Mode::MergeCommit,
            message: String::new(),
        }));
        true
    }

    /// The mode chosen in the dialog.
    pub fn set_mode(&mut self, mode: Mode) {
        if let Some(Dialog::Start(dialog)) = &mut self.dialog {
            dialog.mode = mode;
        }
    }

    /// Submit of the merge dialog: it closes and the merge starts (parameters and description, as
    /// [`MergeBox::start`]). `None` without the dialog.
    pub fn submit_start(&mut self) -> Option<(Value, String)> {
        match self.dialog.take() {
            Some(Dialog::Start(dialog)) => {
                let message = dialog.message();
                self.start(dialog.rev, dialog.what, dialog.into, dialog.mode, message)
            }
            other => {
                self.dialog = other;
                None
            }
        }
    }
}
