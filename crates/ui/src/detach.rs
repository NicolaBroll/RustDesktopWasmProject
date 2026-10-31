//! Checking out a commit with detached HEAD (S55): "Check out this commit" in the commit menu
//! (`commit.checkout`), after a small confirmation that explains detached HEAD (skipped with
//! "Don't ask again", a setting), and "Create branch here…" in the top bar (`branch.create`
//! without `start`, then `branch.checkout`). Data only, no egui, in the same [`BranchBox`] as the
//! other branch dialogs: one at a time. A checkout refused because of local changes opens the same
//! dialog as `branch.checkout`; the commits a checkout leaves behind on a detached HEAD are saved
//! by the engine and named in the notice.

use crate::branch::{Action, BranchBox, Dialog, NameKind, Op, Outcome};
use crate::{Request, Tab, params, warnings_text};
use project_domain::{Head, Oid, Status};
use project_engine::{HeadState, JournalState, Message, MessageArg};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::Value;

/// The confirmation of "Check out this commit".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetachDialog {
    /// The commit, by full id.
    pub id: Oid,
    /// "Don't ask again" is ticked: on "Check out" the setting is saved.
    pub dont_ask: bool,
}

/// The parameters of `commit.checkout`.
#[derive(Debug, serde::Serialize)]
struct CommitCheckoutParams {
    rev: String,
}

/// The fields of the `commit.checkout` result the UI reads.
#[derive(Debug, Deserialize)]
struct CommitCheckoutResult {
    head: HeadState,
    changed: bool,
    #[serde(default)]
    warnings: Vec<Message>,
    #[serde(default)]
    backup: Option<String>,
    status: Option<Status>,
    #[serde(default)]
    status_error: Option<String>,
    #[serde(flatten)]
    journal: JournalState,
}

/// The engine warning of the commits saved when leaving a detached HEAD.
const SAVED: &str = "engine-branch-detached-saved";

/// How the running checkout of `id` is named (status bar, refusals).
fn commit_label(id: Oid) -> String {
    tr!("branch-start-commit", id = id.short())
}

impl BranchBox {
    /// Opens the confirmation of "Check out this commit".
    pub fn open_detach(&mut self, id: Oid) -> bool {
        if self.busy() {
            return false;
        }
        self.dialog = Some(Dialog::Detach(DetachDialog {
            id,
            dont_ask: false,
        }));
        true
    }

    /// The "Don't ask again" checkbox changed.
    pub fn set_dont_ask(&mut self, dont_ask: bool) {
        if let Some(Dialog::Detach(dialog)) = &mut self.dialog {
            dialog.dont_ask = dont_ask;
        }
    }

    /// "Check out" in the confirmation: closes it and starts the checkout of its commit.
    pub fn confirm_detach(&mut self) -> Option<Value> {
        let Some(Dialog::Detach(dialog)) = &self.dialog else {
            return None;
        };
        let id = dialog.id;
        self.dialog = None;
        self.start_checkout_commit(id)
    }

    /// Starts `commit.checkout` of `id` (the full id: what the user saw).
    pub fn start_checkout_commit(&mut self, id: Oid) -> Option<Value> {
        if self.busy() {
            return None;
        }
        self.checkout = Some(commit_label(id));
        Some(params(CommitCheckoutParams {
            rev: id.to_string(),
        }))
    }

    /// Opens the new-branch dialog at the detached HEAD, shown at `id`: the branch is created
    /// where HEAD is and then checked out.
    pub fn open_here(&mut self, id: Oid) -> bool {
        if !self.open_name(NameKind::Here(id), String::new()) {
            return false;
        }
        if let Some(Dialog::Name(dialog)) = &mut self.dialog {
            dialog.checkout_after = true;
        }
        true
    }
}

/// The engine warnings as the text appended to a notice, with the commits saved when leaving a
/// detached HEAD said plainly; `backup` named even if no warning does.
pub(crate) fn backup_text(warnings: &[Message], backup: Option<&str>) -> String {
    let mut saved = false;
    let texts: Vec<String> = warnings
        .iter()
        .map(|w| match saved_commits(w) {
            Some((n, name)) => {
                saved = true;
                tr!("detach-saved", n = n, backup = name)
            }
            None => w.text(),
        })
        .collect();
    let mut text = warnings_text(&texts);
    if let Some(backup) = backup.filter(|b| !saved && !texts.iter().any(|t| t.contains(b))) {
        text += &warnings_text(&[tr!("detach-saved-unknown", backup = backup)]);
    }
    text
}

/// The count and the backup of the "commits saved" warning.
fn saved_commits(warning: &Message) -> Option<(u64, String)> {
    if warning.key() != SAVED {
        return None;
    }
    let mut count = None;
    let mut backup = None;
    for (name, arg) in warning.args() {
        match (name.as_ref(), arg) {
            // A count of commits: a small whole number.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            ("count", MessageArg::Number(n)) => count = Some(*n as u64),
            ("backup", MessageArg::Text(b)) => backup = Some(b.clone()),
            _ => {}
        }
    }
    count.zip(backup)
}

/// The outcome of `commit.checkout`.
pub(crate) fn outcome(value: Value) -> Result<Outcome, String> {
    let r: CommitCheckoutResult = crate::decode(Op::CheckoutCommit.command(), value)?;
    let id = match &r.head {
        HeadState::Detached(id) => id.short(),
        HeadState::Branch(name) => name.clone(),
    };
    let notice = if r.changed {
        tr!("detach-checked-out", id = id)
    } else {
        tr!("detach-already", id = id)
    };
    Ok(Outcome {
        op: Op::CheckoutCommit,
        notice: notice + &backup_text(&r.warnings, r.backup.as_deref()),
        refs: None,
        status: r.status,
        warning: r
            .status_error
            .map(|e| tr!("branch-checkout-status-error", error = e)),
        history: r.journal.into(),
        then_checkout: None,
    })
}

impl Tab {
    /// "Check out this commit" and "Create branch here…", dispatched by [`Tab::apply_branch`].
    pub(crate) fn apply_detach(&mut self, action: Action) {
        let generation = self.generation;
        let writing = self.executor.writing().is_some();
        let Some(repo) = &mut self.repo else {
            return;
        };
        let started = match action {
            Action::CheckoutCommit { id, ask } => {
                if writing || repo.modal_open() {
                    return;
                }
                if ask {
                    repo.branch.open_detach(id);
                    return;
                }
                repo.branch.start_checkout_commit(id).map(|p| (p, id))
            }
            Action::DetachDontAsk(dont_ask) => {
                repo.branch.set_dont_ask(dont_ask);
                return;
            }
            Action::ConfirmCheckoutCommit => {
                let Some(Dialog::Detach(dialog)) = repo.branch.dialog() else {
                    return;
                };
                let id = dialog.id;
                if writing {
                    return;
                }
                repo.branch.confirm_detach().map(|p| (p, id))
            }
            Action::CreateBranchHere => {
                if let Head::Detached(id) = repo.head
                    && !writing
                    && !repo.modal_open()
                {
                    repo.branch.open_here(id);
                }
                return;
            }
            _ => return,
        };
        if let Some((params, id)) = started {
            repo.write_error = None;
            repo.write_label = Some(tr!("detach-checkout-label", id = id.short()));
            let op = Op::CheckoutCommit;
            self.submit(op.command(), params, Request::Branch(generation, op));
        }
    }
}

#[cfg(test)]
#[path = "detach_tests.rs"]
mod tests;
