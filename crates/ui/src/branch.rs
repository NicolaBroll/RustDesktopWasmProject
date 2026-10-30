//! Local branches from the refs column menu (S24): `branch.create`, `branch.checkout`,
//! `branch.delete`, `branch.rename`; and `tag.create`, from the same name dialog, and `tag.delete`
//! (S47), from the tag menu. Data only, no egui: tested with unit tests.
//!
//! The name of a new (or renamed) branch or of a new tag is typed in a small dialog; the only check done
//! here is that it is not empty: the engine decides whether it is valid, and its error stays in the dialog.
//! Deletion happens in two steps like discard: first without `confirm`, which deletes a merged
//! branch right away or answers `ConfirmRequired`; only after the click in the confirmation
//! dialog, `"confirm": true`. A checkout refused because it would overwrite local changes
//! opens a dialog with the files.

use crate::discard::{History, Phase};
use crate::{Action, Done, Request, Tab, params};
use crate::{ErrorKind, messages_text};
use project_domain::{Oid, RefInfo, RefKind, Status};
use project_domain::{branch_short, tag_short};
use project_engine::{
    BranchCheckoutParams, BranchCheckoutResult, BranchCreateParams, BranchCreateResult,
    BranchDeleteParams, BranchDeleteResult, BranchRenameParams, BranchRenameResult,
    TagCreateParams, TagCreateResult, TagDeleteParams, TagDeleteResult,
};
use project_i18n::tr;
use serde_json::{Value, json};

/// A command on branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Create,
    Checkout,
    Delete,
    Rename,
    /// `tag.create`: the name dialog serves tags too.
    CreateTag,
    /// `tag.delete`: always confirmed, like a branch with unmerged commits.
    DeleteTag,
}

impl Op {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            Op::Create => "branch.create",
            Op::Checkout => "branch.checkout",
            Op::Delete => "branch.delete",
            Op::Rename => "branch.rename",
            Op::CreateTag => "tag.create",
            Op::DeleteTag => "tag.delete",
        }
    }
}

/// Where a new branch starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// The commit, always by full id: it is what the user saw, even if the ref moved in the
    /// meantime.
    pub target: Oid,
    /// How to show it in the dialog, e.g. `origin/main` or `commit 1a2b3c4`.
    pub label: String,
}

impl Start {
    /// The starting point of the `info` ref and the proposed name: the name of the remote branch without
    /// the remote (`origin/feature/x` -> `feature/x`); empty for local branches and tags (the same
    /// name would already be taken, or ambiguous with the tag).
    pub fn from_ref(info: &RefInfo) -> (Self, String) {
        let label = info.short_name().to_owned();
        let name = match info.kind {
            RefKind::RemoteBranch => label
                .split_once('/')
                .map(|(_, rest)| rest)
                .filter(|rest| !rest.is_empty() && *rest != "HEAD")
                .unwrap_or_default()
                .to_owned(),
            RefKind::LocalBranch | RefKind::Tag | RefKind::Stash => String::new(),
        };
        let start = Start {
            target: info.target,
            label,
        };
        (start, name)
    }

    /// The starting point of a commit in the list.
    pub fn from_commit(id: Oid) -> Self {
        Start {
            target: id,
            label: tr!("branch-start-commit", id = id.short()),
        }
    }
}

/// The name dialog: new branch or rename.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameKind {
    Create(Start),
    /// The current short name.
    Rename(String),
    /// A new tag on the commit of `Start`.
    Tag(Start),
}

/// The dialog to type the name of a branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameDialog {
    pub kind: NameKind,
    /// The typed text.
    pub name: String,
    /// The tag message (only for [`NameKind::Tag`]): with it the tag is annotated.
    pub message: String,
    /// The engine error from the last submit, shown in the dialog.
    pub error: Option<String>,
    /// True while the command is running.
    pub sending: bool,
}

impl NameDialog {
    /// The name to send: without surrounding spaces.
    fn trimmed(&self) -> &str {
        self.name.trim()
    }

    /// True if the button is enabled: a name is typed (and, for a rename, different from the
    /// current one), no submit in progress. The rest is checked by the engine.
    pub fn can_submit(&self) -> bool {
        let name = self.trimmed();
        !self.sending
            && !name.is_empty()
            && match &self.kind {
                NameKind::Create(_) | NameKind::Tag(_) => true,
                NameKind::Rename(old) => name != old,
            }
    }

    fn op(&self) -> Op {
        match self.kind {
            NameKind::Create(_) => Op::Create,
            NameKind::Rename(_) => Op::Rename,
            NameKind::Tag(_) => Op::CreateTag,
        }
    }

    fn params(&self) -> Value {
        let name = self.trimmed();
        match &self.kind {
            NameKind::Create(start) => params(BranchCreateParams {
                name: name.to_owned(),
                start: Some(start.target.to_string()),
            }),
            NameKind::Rename(old) => params(BranchRenameParams {
                name: old.clone(),
                new_name: name.to_owned(),
            }),
            NameKind::Tag(start) => {
                let message = self.message.trim();
                params(TagCreateParams {
                    name: name.to_owned(),
                    target: Some(start.target.to_string()),
                    message: (!message.is_empty()).then(|| message.to_owned()),
                })
            }
        }
    }

    /// The description of the operation for the status bar.
    pub fn label(&self) -> String {
        match &self.kind {
            NameKind::Create(_) => tr!("branch-create-label", name = self.trimmed()),
            NameKind::Rename(old) => tr!("branch-rename-label", name = old),
            NameKind::Tag(_) => tr!("branch-tag-label", name = self.trimmed()),
        }
    }
}

/// The deletion of a branch, from the check to the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteDialog {
    /// The short name.
    pub name: String,
    /// The branch commit as the user saw it: it goes to the engine as `expected` in both
    /// submits, so the confirmation is valid only for that commit.
    pub target: Oid,
    pub phase: Phase,
}

/// The deletion of a local tag, from the check to the answer: the engine always asks for
/// confirmation, with the commit and, for an annotated tag, the tag object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTagDialog {
    /// The short name.
    pub name: String,
    pub phase: Phase,
    /// The tag value shown in the confirmation (the tag object of an annotated tag, the commit of
    /// a lightweight one), as the engine gave it: it goes back as `"expected"`, so the confirmation
    /// holds only for that value.
    pub expected: Option<String>,
}

/// The refused checkout: it would overwrite local changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// The short name of the branch.
    pub name: String,
    /// The files listed by git.
    pub files: Vec<String>,
}

/// The open branch dialog (or the deletion being checked, still without a dialog).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Name(NameDialog),
    Delete(DeleteDialog),
    DeleteTag(DeleteTagDialog),
    Refused(Refused),
}

/// Commands on branches: one dialog at a time, one command at a time.
#[derive(Debug, Default)]
pub struct BranchBox {
    dialog: Option<Dialog>,
    /// The checkout in progress, short name (it has no dialog).
    checkout: Option<String>,
}

/// What came back from a `branch.*` command.
#[derive(Debug)]
pub enum Received {
    /// Done: what to update in the view.
    Done(Box<Outcome>),
    /// The name is not accepted: the error is in the dialog, which stays open.
    Inline,
    /// The deletion needs confirmation: the dialog opens.
    Confirm,
    /// Checkout refused because of local changes: the dialog with the files opens.
    Refused,
    /// Refused or failed: the message to show; with `reload` everything is reloaded
    /// (the operation may have been done, or half done).
    Failed { message: String, reload: bool },
}

/// The result of a successful command, for the view.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub op: Op,
    pub notice: String,
    /// The new refs, if the command reloaded them.
    pub refs: Option<Vec<RefInfo>>,
    /// The new status (checkout).
    pub status: Option<Status>,
    /// A problem after the successful operation (refs or status not reloaded).
    pub warning: Option<String>,
    pub history: History,
}

impl BranchBox {
    /// The dialog to draw: the deletion is shown only when it asks for confirmation.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Delete(delete) => matches!(delete.phase, Phase::Confirm { .. }),
            Dialog::DeleteTag(delete) => matches!(delete.phase, Phase::Confirm { .. }),
            Dialog::Name(_) | Dialog::Refused(_) => true,
        })
    }

    /// True if a dialog is open: no other writes, keys go to it.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// True if there is a dialog or a branch command in progress: no other one is opened.
    fn busy(&self) -> bool {
        self.dialog.is_some() || self.checkout.is_some()
    }

    /// Opens the dialog for a new branch from `start`, with the proposed name `name`.
    pub fn open_create(&mut self, start: Start, name: String) -> bool {
        self.open_name(NameKind::Create(start), name)
    }

    /// Opens the dialog for a new tag on the commit of `start`.
    pub fn open_tag(&mut self, start: Start) -> bool {
        self.open_name(NameKind::Tag(start), String::new())
    }

    /// Opens the dialog to rename the branch `name` (short).
    pub fn open_rename(&mut self, name: String) -> bool {
        self.open_name(NameKind::Rename(name.clone()), name)
    }

    fn open_name(&mut self, kind: NameKind, name: String) -> bool {
        if self.busy() {
            return false;
        }
        self.dialog = Some(Dialog::Name(NameDialog {
            kind,
            name,
            message: String::new(),
            error: None,
            sending: false,
        }));
        true
    }

    /// The text of the name dialog changed.
    pub fn set_name(&mut self, name: String) {
        if let Some(Dialog::Name(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.name = name;
        }
    }

    /// The text of the tag message changed.
    pub fn set_message(&mut self, message: String) {
        if let Some(Dialog::Name(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.message = message;
        }
    }

    /// Submit of the name dialog: command, parameters and description. `None` if it cannot be done.
    pub fn submit_name(&mut self) -> Option<(Op, Value, String)> {
        let Some(Dialog::Name(dialog)) = &mut self.dialog else {
            return None;
        };
        if !dialog.can_submit() {
            return None;
        }
        dialog.sending = true;
        dialog.error = None;
        Some((dialog.op(), dialog.params(), dialog.label()))
    }

    /// Starts the checkout of the branch `name` (short).
    pub fn start_checkout(&mut self, name: String) -> Option<Value> {
        if self.busy() {
            return None;
        }
        let params = params(BranchCheckoutParams { name: name.clone() });
        self.checkout = Some(name);
        Some(params)
    }

    /// Starts the deletion of the branch `name` (short) that the user sees on `target`: the
    /// parameters of the first submit, without `confirm`.
    pub fn start_delete(&mut self, name: String, target: Oid) -> Option<Value> {
        if self.busy() {
            return None;
        }
        let params = params(BranchDeleteParams {
            name: name.clone(),
            confirm: false,
            expected: Some(target.to_string()),
        });
        self.dialog = Some(Dialog::Delete(DeleteDialog {
            name,
            target,
            phase: Phase::Checking,
        }));
        Some(params)
    }

    /// The user confirmed the deletion: the parameters with `"confirm": true`.
    pub fn confirm_delete(&mut self) -> Option<Value> {
        let Some(Dialog::Delete(delete)) = &mut self.dialog else {
            return None;
        };
        if !matches!(delete.phase, Phase::Confirm { .. }) {
            return None;
        }
        delete.phase = Phase::Sending;
        Some(params(BranchDeleteParams {
            name: delete.name.clone(),
            confirm: true,
            expected: Some(delete.target.to_string()),
        }))
    }

    /// Starts the deletion of the local tag `name` (short): the parameters of the first submit,
    /// without `confirm` (the engine answers with what is lost).
    pub fn start_delete_tag(&mut self, name: String) -> Option<Value> {
        if self.busy() {
            return None;
        }
        let params = params(TagDeleteParams {
            name: name.clone(),
            confirm: false,
            expected: None,
        });
        self.dialog = Some(Dialog::DeleteTag(DeleteTagDialog {
            name,
            phase: Phase::Checking,
            expected: None,
        }));
        Some(params)
    }

    /// The user confirmed the tag deletion: the parameters with `"confirm": true` and the value
    /// seen. `None` if the engine did not give it (nothing can be confirmed).
    pub fn confirm_delete_tag(&mut self) -> Option<Value> {
        let Some(Dialog::DeleteTag(delete)) = &mut self.dialog else {
            return None;
        };
        if !matches!(delete.phase, Phase::Confirm { .. }) {
            return None;
        }
        let expected = delete.expected.clone()?;
        delete.phase = Phase::Sending;
        Some(params(TagDeleteParams {
            name: delete.name.clone(),
            confirm: true,
            expected: Some(expected),
        }))
    }

    /// Closes the open dialog without sending anything; not during a submit.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Name(dialog)) => !dialog.sending,
            Some(Dialog::Delete(delete)) => matches!(delete.phase, Phase::Confirm { .. }),
            Some(Dialog::DeleteTag(delete)) => matches!(delete.phase, Phase::Confirm { .. }),
            Some(Dialog::Refused(_)) => true,
            None => false,
        };
        if closable {
            self.dialog = None;
        }
    }

    /// The answer of the command `op`.
    pub fn receive(
        &mut self,
        op: Op,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
        expected: Option<String>,
    ) -> Received {
        match op {
            Op::Create | Op::Rename | Op::CreateTag => self.receive_name(op, result),
            Op::Checkout => self.receive_checkout(result, kind, detail),
            Op::Delete => self.receive_delete(result, kind, detail),
            Op::DeleteTag => self.receive_delete_tag(result, kind, detail, expected),
        }
    }

    fn receive_name(&mut self, op: Op, result: Result<Value, String>) -> Received {
        let Some(Dialog::Name(dialog)) = &mut self.dialog else {
            return unexpected(op);
        };
        if !dialog.sending || dialog.op() != op {
            return unexpected(op);
        }
        match result {
            Err(e) => {
                // Any refusal (invalid name, already taken, vanished rev) stays in the dialog:
                // nothing changed and the name can be fixed.
                dialog.sending = false;
                dialog.error = Some(e);
                Received::Inline
            }
            Ok(value) => {
                self.dialog = None;
                let outcome =
                    match op {
                        Op::Create => crate::decode::<BranchCreateResult>(op.command(), value)
                            .map(create_outcome),
                        Op::CreateTag => crate::decode::<TagCreateResult>(op.command(), value)
                            .map(tag_create_outcome),
                        _ => crate::decode::<BranchRenameResult>(op.command(), value)
                            .map(rename_outcome),
                    };
                done(outcome)
            }
        }
    }

    fn receive_checkout(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(name) = self.checkout.take() else {
            return unexpected(Op::Checkout);
        };
        match result {
            Err(_) if kind == Some(ErrorKind::CheckoutConflict) => {
                let files = detail
                    .unwrap_or_default()
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                self.dialog = Some(Dialog::Refused(Refused { name, files }));
                Received::Refused
            }
            Err(e) => {
                // Refused before starting: nothing changed. A git error instead
                // may have left the checkout half done: everything is reloaded.
                let refused =
                    matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams));
                let message = if refused {
                    tr!("branch-checkout-refused", name = &name, error = &e)
                } else {
                    tr!("branch-checkout-failed", name = &name, error = &e)
                };
                Received::Failed {
                    message,
                    reload: !refused,
                }
            }
            Ok(value) => done(
                crate::decode::<BranchCheckoutResult>(Op::Checkout.command(), value)
                    .map(checkout_outcome),
            ),
        }
    }

    fn receive_delete(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Delete(delete)) = self.dialog.take() else {
            return unexpected(Op::Delete);
        };
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && delete.phase == Phase::Checking =>
            {
                self.dialog = Some(Dialog::Delete(DeleteDialog {
                    name: delete.name,
                    target: delete.target,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                }));
                Received::Confirm
            }
            Err(e) => {
                let message = if kind == Some(ErrorKind::NotAllowed) {
                    tr!("branch-delete-refused", name = &delete.name, error = &e)
                } else {
                    tr!("branch-delete-failed", name = &delete.name, error = &e)
                };
                Received::Failed {
                    message,
                    reload: false,
                }
            }
            Ok(value) => done(
                crate::decode::<BranchDeleteResult>(Op::Delete.command(), value)
                    .map(delete_outcome),
            ),
        }
    }
}

impl BranchBox {
    fn receive_delete_tag(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
        expected: Option<String>,
    ) -> Received {
        let Some(Dialog::DeleteTag(delete)) = self.dialog.take() else {
            return unexpected(Op::DeleteTag);
        };
        match result {
            // The first answer, or the tag changed since the confirmation: (again) the dialog,
            // with the value to confirm.
            Err(_) if kind == Some(ErrorKind::ConfirmRequired) => {
                self.dialog = Some(Dialog::DeleteTag(DeleteTagDialog {
                    name: delete.name,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                    expected,
                }));
                Received::Confirm
            }
            Err(e) => {
                let message = if kind == Some(ErrorKind::NotAllowed) {
                    tr!("branch-tag-delete-refused", name = &delete.name, error = &e)
                } else {
                    tr!("branch-tag-delete-failed", name = &delete.name, error = &e)
                };
                Received::Failed {
                    message,
                    reload: false,
                }
            }
            Ok(value) => done(
                crate::decode::<TagDeleteResult>(Op::DeleteTag.command(), value)
                    .map(tag_delete_outcome),
            ),
        }
    }
}

fn unexpected(op: Op) -> Received {
    Received::Failed {
        message: tr!("common-unexpected-response", command = op.command()),
        reload: true,
    }
}

/// A result that cannot be read: the operation is probably done, everything is reloaded.
fn done(outcome: Result<Outcome, String>) -> Received {
    match outcome {
        Ok(outcome) => Received::Done(Box::new(outcome)),
        Err(message) => Received::Failed {
            message,
            reload: true,
        },
    }
}

/// The outcome of `tag.create`.
fn tag_create_outcome(r: TagCreateResult) -> Outcome {
    let tag = tag_short(&r.tag);
    let id = r.target.short();
    let notice = if r.annotated {
        tr!("branch-tag-created-annotated", tag = tag, id = id)
    } else {
        tr!("branch-tag-created", tag = tag, id = id)
    };
    Outcome {
        op: Op::CreateTag,
        notice,
        refs: r.refs,
        status: None,
        warning: r.refs_error.map(refs_warning),
        history: r.journal.into(),
    }
}

/// The outcome of `tag.delete`.
fn tag_delete_outcome(r: TagDeleteResult) -> Outcome {
    let tag = tag_short(&r.tag);
    let (id, backup) = (r.target.short(), &r.backup);
    let notice = if r.annotated {
        tr!(
            "branch-tag-deleted-annotated",
            tag = tag,
            id = id,
            backup = backup
        )
    } else {
        tr!("branch-tag-deleted", tag = tag, id = id, backup = backup)
    };
    Outcome {
        op: Op::DeleteTag,
        notice,
        refs: r.refs,
        status: None,
        warning: r.refs_error.map(refs_warning),
        history: r.journal.into(),
    }
}

/// The outcome of `branch.create`.
fn create_outcome(r: BranchCreateResult) -> Outcome {
    Outcome {
        op: Op::Create,
        notice: tr!(
            "branch-created",
            branch = branch_short(&r.branch),
            id = r.target.short()
        ),
        refs: r.refs,
        status: None,
        warning: r.refs_error.map(refs_warning),
        history: r.journal.into(),
    }
}

/// The outcome of `branch.checkout`.
fn checkout_outcome(r: BranchCheckoutResult) -> Outcome {
    let name = branch_short(&r.branch);
    let notice = if r.changed {
        tr!("branch-checked-out", branch = name)
    } else {
        tr!("branch-already-active", branch = name)
    };
    Outcome {
        op: Op::Checkout,
        notice: notice + &messages_text(&r.warnings),
        refs: None,
        status: r.status,
        warning: r
            .status_error
            .map(|e| tr!("branch-checkout-status-error", error = e)),
        history: r.journal.into(),
    }
}

/// The notice of `branch.delete`: with the configuration removed it names the upstream if there
/// was one, otherwise the keys.
fn delete_notice(r: &BranchDeleteResult) -> String {
    let branch = branch_short(&r.branch);
    let id = r.target.short();
    let (unmerged, backup) = (r.unmerged, &r.backup);
    let upstream = r
        .removed_config
        .iter()
        .any(|c| c.key.ends_with(".merge") || c.key.ends_with(".remote"));
    if r.removed_config.is_empty() {
        tr!(
            "branch-deleted",
            branch = branch,
            id = id,
            unmerged = unmerged,
            backup = backup
        )
    } else if upstream {
        tr!(
            "branch-deleted-upstream",
            branch = branch,
            id = id,
            unmerged = unmerged,
            backup = backup
        )
    } else {
        let keys: Vec<&str> = r.removed_config.iter().map(|c| c.key.as_str()).collect();
        tr!(
            "branch-deleted-config",
            branch = branch,
            id = id,
            unmerged = unmerged,
            backup = backup,
            keys = keys.join(", ")
        )
    }
}

/// The outcome of `branch.delete`.
fn delete_outcome(r: BranchDeleteResult) -> Outcome {
    Outcome {
        op: Op::Delete,
        notice: delete_notice(&r) + &messages_text(&r.warnings),
        refs: r.refs,
        status: None,
        warning: r.refs_error.map(refs_warning),
        history: r.journal.into(),
    }
}

/// The outcome of `branch.rename`.
fn rename_outcome(r: BranchRenameResult) -> Outcome {
    Outcome {
        op: Op::Rename,
        notice: tr!(
            "branch-renamed",
            previous = branch_short(&r.previous),
            branch = branch_short(&r.branch)
        ) + &messages_text(&r.warnings),
        refs: r.refs,
        status: None,
        warning: r.refs_error.map(refs_warning),
        history: r.journal.into(),
    }
}

fn refs_warning(error: String) -> String {
    tr!("branch-refs-warning", error = error)
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_branch(&mut self, action: Action) {
        match action {
            Action::NewBranch(start, name) => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.branch.open_create(start, name);
                }
            }
            Action::NewTag(start) => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.branch.open_tag(start);
                }
            }
            Action::TagMessage(message) => {
                if let Some(repo) = &mut self.repo {
                    repo.branch.set_message(message);
                }
            }
            Action::RenameBranch(name) => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.branch.open_rename(name);
                }
            }
            Action::BranchName(name) => {
                if let Some(repo) = &mut self.repo {
                    repo.branch.set_name(name);
                }
            }
            Action::SubmitBranchName => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((op, params, label)) = repo.branch.submit_name() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::Checkout(name) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = tr!("branch-checkout-label", name = &name);
                if let Some(params) = repo.branch.start_checkout(name) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::Checkout;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::DeleteBranch(name, target) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = tr!("branch-delete-label", name = &name);
                if let Some(params) = repo.branch.start_delete(name, target) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::Delete;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::ConfirmDeleteBranch => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let label = match repo.branch.dialog() {
                    Some(Dialog::Delete(d)) => tr!("branch-delete-label", name = &d.name),

                    _ => return,
                };
                if let Some(params) = repo.branch.confirm_delete() {
                    repo.write_label = Some(label);
                    let op = Op::Delete;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::DeleteTag(name) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = tr!("branch-tag-delete-label", name = &name);
                if let Some(params) = repo.branch.start_delete_tag(name) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::DeleteTag;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::ConfirmDeleteTag => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let label = match repo.branch.dialog() {
                    Some(Dialog::DeleteTag(d)) => {
                        tr!("branch-tag-delete-label", name = &d.name)
                    }
                    _ => return,
                };
                if let Some(params) = repo.branch.confirm_delete_tag() {
                    repo.write_label = Some(label);
                    let op = Op::DeleteTag;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::CloseBranchDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.branch.close();
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Answer of `branch.*`: refs and journal from the answer, then the log reloaded (a deleted
    /// branch can take some commits with it); after a checkout everything is reloaded (HEAD, graph,
    /// stage area). A refused name stays in its dialog.
    pub(crate) fn receive_branch(&mut self, generation: u64, op: Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo.branch.receive(
            op,
            done.result,
            done.error_kind,
            done.error_detail,
            done.confirm_expected,
        ) {
            Received::Inline | Received::Confirm | Received::Refused => {
                self.error.clear();
                self.notice.clear();
            }
            Received::Done(outcome) => {
                repo.history = outcome.history.clone();
                let refs_read = outcome.refs.is_some();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                if let Some(status) = outcome.status {
                    repo.set_status(status);
                }
                let checkout = outcome.op == Op::Checkout;
                // After a checkout the new HEAD is selected as soon as the log arrives.
                if checkout && !repo.worktree_selected {
                    repo.select_after_load = repo.head.target();
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                if checkout {
                    self.follow_diff();
                    self.load();
                } else {
                    // Usually the refs arrive with the answer: the log is enough.
                    if !refs_read {
                        self.submit("repo.refs", json!({}), Request::Refs(generation));
                    }
                    if let Some(repo) = self.current(generation) {
                        let first = repo.list.reset();
                        self.request_page(first);
                    }
                }
            }
            Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    // A refused deletion (e.g. the branch is no longer on the seen commit): the
                    // refs are reloaded, so the column shows where it is now.
                    if matches!(op, Op::Delete | Op::DeleteTag) {
                        self.submit("repo.refs", json!({}), Request::Refs(generation));
                    }
                    // The journal is reloaded anyway: it is cheap and stays aligned.
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{GitBackend, LogQuery, MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

    fn remote(name: &str) -> RefInfo {
        RefInfo {
            name: format!("refs/remotes/{name}"),
            kind: RefKind::RemoteBranch,
            target: Oid::from_bytes([7; 20]),
            tag_object: None,
            upstream: None,
        }
    }

    #[test]
    fn proposed_names_drop_the_remote() {
        let (start, name) = Start::from_ref(&remote("origin/feature/x"));
        assert_eq!(name, "feature/x");
        assert_eq!(start.label, "origin/feature/x");
        assert_eq!(start.target, Oid::from_bytes([7; 20]));
        assert_eq!(Start::from_ref(&remote("origin/HEAD")).1, "");
        let tag = RefInfo {
            name: "refs/tags/v1".to_owned(),
            kind: RefKind::Tag,
            ..remote("x")
        };
        assert_eq!(Start::from_ref(&tag).1, "");
        let commit = Start::from_commit(Oid::from_bytes([7; 20]));
        assert!(commit.label.starts_with("commit "), "{}", commit.label);
    }

    #[test]
    fn the_name_dialog_sends_only_a_written_name() {
        let mut b = BranchBox::default();
        let start = Start::from_commit(Oid::from_bytes([3; 20]));
        assert!(b.open_create(start, String::new()));
        // One dialog at a time.
        assert!(!b.open_rename("main".to_owned()));
        assert!(b.modal_open());
        assert_eq!(b.submit_name(), None);
        b.set_name("  feature/y ".to_owned());
        let (op, params, label) = b.submit_name().unwrap();
        assert_eq!(op, Op::Create);
        assert_eq!(
            params,
            json!({ "name": "feature/y", "start": Oid::from_bytes([3; 20]).to_string() })
        );
        assert_eq!(label, "Creating branch “feature/y”");
        // During the submit nothing changes, nothing closes, nothing starts twice.
        assert_eq!(b.submit_name(), None);
        b.set_name("other".to_owned());
        b.close();
        let Some(Dialog::Name(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.name, "  feature/y ");

        // The engine error stays in the dialog, which can be fixed and resubmitted.
        let r = b.receive(Op::Create, Err("invalid name".to_owned()), None, None, None);
        assert!(matches!(r, Received::Inline));
        let Some(Dialog::Name(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.error.as_deref(), Some("invalid name"));
        assert!(!dialog.sending);
        b.close();
        assert!(b.dialog().is_none());

        // Rename: the same name is not sent.
        assert!(b.open_rename("main".to_owned()));
        assert_eq!(b.submit_name(), None);
        b.set_name("primary".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        assert_eq!(op, Op::Rename);
        assert_eq!(params, json!({ "name": "main", "new_name": "primary" }));
    }

    #[test]
    fn the_tag_dialog_sends_the_message_only_if_written() {
        let mut opener = MemoryOpener::default();
        opener.insert("r", MemoryBackend::sample());
        let mut s = Session::new(Arc::new(opener));
        let r = Registry::builtin();
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        let head = s.repo().unwrap().head().unwrap().target().unwrap();

        let mut b = BranchBox::default();
        assert!(b.open_tag(Start::from_commit(head)));
        assert_eq!(b.submit_name(), None, "no name yet");
        b.set_name(" v2 ".to_owned());
        b.set_message("  ".to_owned());
        let (op, params, label) = b.submit_name().unwrap();
        assert_eq!(op, Op::CreateTag);
        assert_eq!(params, json!({ "name": "v2", "target": head.to_string() }));
        assert_eq!(label, "Creating tag “v2”");
        let Received::Done(outcome) = b.receive(
            op,
            r.run(op.command(), &mut s, params)
                .map_err(|e| e.to_string()),
            None,
            None,
            None,
        ) else {
            panic!()
        };
        assert!(
            outcome.notice.starts_with("Tag “v2” created"),
            "{}",
            outcome.notice
        );
        assert!(outcome.history.can_undo);
        assert!(b.dialog().is_none());

        assert!(b.open_tag(Start::from_commit(head)));
        b.set_name("v3".to_owned());
        b.set_message("Release 3\n".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        assert_eq!(params["message"], "Release 3");
        let Received::Done(outcome) = b.receive(
            op,
            r.run(op.command(), &mut s, params)
                .map_err(|e| e.to_string()),
            None,
            None,
            None,
        ) else {
            panic!()
        };
        assert!(
            outcome.notice.starts_with("Annotated tag “v3”"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn delete_asks_confirmation_only_when_the_engine_does() {
        let mut b = BranchBox::default();
        let tip = Oid::from_bytes([7; 20]);
        let check = b.start_delete("feature/x".to_owned(), tip).unwrap();
        assert_eq!(
            check,
            json!({ "name": "feature/x", "expected": tip.to_string() })
        );
        // The check is not shown and not confirmed before the answer.
        assert!(b.dialog().is_none());
        assert_eq!(b.confirm_delete(), None);
        assert_eq!(b.start_delete("other".to_owned(), tip), None);
        assert_eq!(b.start_checkout("other".to_owned()), None);

        let r = b.receive(
            Op::Delete,
            Err(String::new()),
            Some(ErrorKind::ConfirmRequired),
            Some("2 unmerged commits".to_owned()),
            None,
        );
        assert!(matches!(r, Received::Confirm));
        let Some(Dialog::Delete(d)) = b.dialog() else {
            panic!()
        };
        assert_eq!(
            d.phase,
            Phase::Confirm {
                reason: "2 unmerged commits".to_owned()
            }
        );
        // Closing sends nothing.
        b.close();
        assert!(b.dialog().is_none());
        assert_eq!(b.confirm_delete(), None);

        b.start_delete("feature/x".to_owned(), tip).unwrap();
        b.receive(
            Op::Delete,
            Err(String::new()),
            Some(ErrorKind::ConfirmRequired),
            None,
            None,
        );
        let send = b.confirm_delete().unwrap();
        assert_eq!(
            send,
            json!({ "name": "feature/x", "confirm": true, "expected": tip.to_string() })
        );
        assert!(
            b.dialog().is_none(),
            "during the submit the dialog is closed"
        );
        // A new confirmation request after `confirm: true` does not reopen the dialog.
        let r = b.receive(
            Op::Delete,
            Err("x".to_owned()),
            Some(ErrorKind::ConfirmRequired),
            None,
            None,
        );
        assert!(matches!(r, Received::Failed { .. }), "{r:?}");
        assert!(b.dialog().is_none());

        // A refusal: the message, nothing to reload.
        b.start_delete("main".to_owned(), tip).unwrap();
        let r = b.receive(
            Op::Delete,
            Err("it is the current branch".to_owned()),
            Some(ErrorKind::NotAllowed),
            None,
            None,
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert_eq!(message, "Cannot delete “main”: it is the current branch");
        assert!(!reload);
    }

    #[test]
    fn checkout_conflicts_open_the_list_of_files() {
        let mut b = BranchBox::default();
        assert_eq!(
            b.start_checkout("feature/x".to_owned()).unwrap(),
            json!({ "name": "feature/x" })
        );
        assert!(!b.modal_open());
        let r = b.receive(
            Op::Checkout,
            Err("refused".to_owned()),
            Some(ErrorKind::CheckoutConflict),
            Some("README.md\nsrc/lib.rs".to_owned()),
            None,
        );
        assert!(matches!(r, Received::Refused));
        assert_eq!(
            b.dialog(),
            Some(&Dialog::Refused(Refused {
                name: "feature/x".to_owned(),
                files: vec!["README.md".to_owned(), "src/lib.rs".to_owned()],
            }))
        );
        b.close();
        assert!(b.dialog().is_none());

        b.start_checkout("x".to_owned()).unwrap();
        let r = b.receive(
            Op::Checkout,
            Err("merge in progress".to_owned()),
            Some(ErrorKind::NotAllowed),
            None,
            None,
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert_eq!(message, "Cannot check out “x”: merge in progress");
        assert!(!reload);
        b.start_checkout("x".to_owned()).unwrap();
        let r = b.receive(
            Op::Checkout,
            Err("git".to_owned()),
            Some(ErrorKind::Git),
            None,
            None,
        );
        assert!(matches!(r, Received::Failed { reload: true, .. }), "{r:?}");
    }

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

    /// The sample repo with `experiment`, a branch with a commit after `main`.
    fn with_unmerged_branch() -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        let head = repo.log(&LogQuery::page(0, 1)).unwrap().remove(0);
        let extra = project_domain::Commit {
            id: Oid::from_bytes([0xee; 20]),
            parents: vec![head.id],
            summary: "Unmerged trial".to_owned(),
            ..head
        };
        repo.set_changes(extra.id, Vec::new());
        repo.set_branch("experiment", extra.id);
        repo.insert_commit(extra);
        repo
    }

    /// Sends `params` to `op` and passes the answer to the box, as the UI does.
    fn send(b: &mut BranchBox, r: &Registry, s: &mut Session, op: Op, params: Value) -> Received {
        match r.run(op.command(), s, params) {
            Ok(value) => b.receive(op, Ok(value), None, None, None),
            Err(e) => b.receive(
                op,
                Err(e.text()),
                Some(ErrorKind::of(&e)),
                ErrorKind::detail(&e),
                ErrorKind::expected(&e),
            ),
        }
    }

    fn done(r: Received) -> Outcome {
        match r {
            Received::Done(outcome) => *outcome,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn delete_tag_confirms_the_value_seen_and_undo_recreates_it() {
        let (r, mut s) = open(MemoryBackend::sample());
        let mut b = BranchBox::default();
        let params = b.start_delete_tag("v0.1.0".to_owned()).unwrap();
        assert_eq!(params, json!({ "name": "v0.1.0" }));
        assert!(!b.modal_open(), "no dialog while checking");
        assert_eq!(b.confirm_delete_tag(), None);
        let received = send(&mut b, &r, &mut s, Op::DeleteTag, params);
        assert!(matches!(received, Received::Confirm), "{received:?}");
        let Some(Dialog::DeleteTag(d)) = b.dialog() else {
            panic!()
        };
        let Phase::Confirm { reason } = &d.phase else {
            panic!()
        };
        assert!(reason.contains("annotated"), "{reason}");
        let value = d.expected.clone().expect("the engine gives the value");
        assert!(reason.contains(&value), "{reason}");

        let params = b.confirm_delete_tag().unwrap();
        assert_eq!(
            params,
            json!({ "name": "v0.1.0", "confirm": true, "expected": value })
        );
        assert!(
            b.dialog().is_none(),
            "during the submit the dialog is closed"
        );
        b.close();
        let outcome = done(send(&mut b, &r, &mut s, Op::DeleteTag, params));
        assert_eq!(outcome.op, Op::DeleteTag);
        assert!(
            outcome
                .notice
                .starts_with("Annotated tag “v0.1.0” deleted (was at "),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("tag-delete-1"),
            "{}",
            outcome.notice
        );
        assert!(outcome.history.can_undo);
        let refs = outcome.refs.unwrap();
        assert!(!refs.iter().any(|r| r.name == "refs/tags/v0.1.0"));
        r.run("edit.undo", &mut s, json!({})).unwrap();
        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", &mut s, json!({})).unwrap()).unwrap();
        assert!(refs.iter().any(|r| r.name == "refs/tags/v0.1.0"));

        // A tag that does not exist: refused, the message names it, the dialog does not open.
        let params = b.start_delete_tag("nope".to_owned()).unwrap();
        let received = send(&mut b, &r, &mut s, Op::DeleteTag, params);
        let Received::Failed { message, reload } = received else {
            panic!("{received:?}")
        };
        assert!(message.starts_with("Cannot delete tag “nope”"), "{message}");
        assert!(!reload);
        assert!(b.dialog().is_none());
    }

    #[test]
    fn a_tag_confirmation_without_the_value_cannot_be_sent() {
        let mut b = BranchBox::default();
        b.start_delete_tag("v1".to_owned()).unwrap();
        b.receive(
            Op::DeleteTag,
            Err(String::new()),
            Some(ErrorKind::ConfirmRequired),
            Some("reason".to_owned()),
            None,
        );
        assert!(b.modal_open());
        assert_eq!(b.confirm_delete_tag(), None);
        b.close();
        assert!(b.dialog().is_none());
    }

    /// The commit of the local branch `name`, as the refs column shows it.
    fn tip(r: &Registry, s: &mut Session, name: &str) -> Oid {
        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", s, json!({})).unwrap()).unwrap();
        let full = format!("refs/heads/{name}");
        refs.iter().find(|i| i.name == full).unwrap().target
    }

    fn has_branch(outcome: &Outcome, name: &str) -> bool {
        let full = format!("refs/heads/{name}");
        outcome
            .refs
            .as_ref()
            .unwrap()
            .iter()
            .any(|r| r.name == full)
    }

    /// The full path with the sample repo: new branch from a remote branch, wrong name
    /// fixed in the dialog, rename, deletion with confirmation, then Undo.
    #[test]
    fn create_rename_delete_with_the_sample_repo() {
        let (r, mut s) = open(with_unmerged_branch());
        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", &mut s, json!({})).unwrap()).unwrap();
        let remote = refs
            .iter()
            .find(|i| i.name == "refs/remotes/origin/main")
            .unwrap();
        let mut b = BranchBox::default();

        // The proposed name (`main`) already exists: the error stays in the dialog.
        let (start, name) = Start::from_ref(remote);
        assert_eq!(name, "main");
        b.open_create(start, name);
        let (op, params, _) = b.submit_name().unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, op, params),
            Received::Inline
        ));
        let Some(Dialog::Name(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(
            dialog.error.as_ref().unwrap().contains("already exists"),
            "{dialog:?}"
        );
        // A name that git does not accept: the engine says so.
        b.set_name("with space".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, op, params),
            Received::Inline
        ));
        b.set_name("from-origin".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        let created = done(send(&mut b, &r, &mut s, op, params));
        assert!(b.dialog().is_none());
        assert!(has_branch(&created, "from-origin"));
        let new = created
            .refs
            .as_ref()
            .unwrap()
            .iter()
            .find(|i| i.name == "refs/heads/from-origin")
            .unwrap();
        assert_eq!(new.target, remote.target);
        assert!(
            created.notice.contains("“from-origin” created"),
            "{}",
            created.notice
        );
        assert!(created.history.can_undo);

        // Rename.
        b.open_rename("from-origin".to_owned());
        b.set_name("trial".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        let renamed = done(send(&mut b, &r, &mut s, op, params));
        assert!(has_branch(&renamed, "trial") && !has_branch(&renamed, "from-origin"));
        assert!(
            renamed.notice.contains("renamed to “trial”"),
            "{}",
            renamed.notice
        );

        // `trial` is on origin/main, which is in HEAD (main): deleted without confirmation.
        let check = b
            .start_delete("trial".to_owned(), tip(&r, &mut s, "trial"))
            .unwrap();
        let deleted = done(send(&mut b, &r, &mut s, Op::Delete, check));
        assert!(!has_branch(&deleted, "trial"));
        assert!(
            deleted
                .notice
                .contains("backup in refs/project/backup/delete-"),
            "{}",
            deleted.notice
        );

        // `feature/graph` is merged and has an upstream: deleted at once, with the configuration.
        let check = b
            .start_delete("feature/graph".to_owned(), tip(&r, &mut s, "feature/graph"))
            .unwrap();
        let deleted = done(send(&mut b, &r, &mut s, Op::Delete, check));
        assert!(
            deleted
                .notice
                .contains("its upstream setting was removed too")
                && deleted
                    .notice
                    .contains("recreates it with its configuration"),
            "{}",
            deleted.notice
        );
        r.run("edit.undo", &mut s, json!({})).unwrap();

        // A new branch from a local branch: no proposed name.
        let graph_branch = refs
            .iter()
            .find(|i| i.name == "refs/heads/feature/graph")
            .unwrap();
        let (start, name) = Start::from_ref(graph_branch);
        assert_eq!(name, "");
        b.open_create(start, "copy".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        assert!(has_branch(
            &done(send(&mut b, &r, &mut s, op, params)),
            "copy"
        ));

        // A branch that is no longer on the seen commit is not deleted: the engine says so.
        let main = tip(&r, &mut s, "main");
        let stale = b.start_delete("experiment".to_owned(), main).unwrap();
        let refused = send(&mut b, &r, &mut s, Op::Delete, stale);
        assert!(
            matches!(&refused, Received::Failed { reload: false, .. }),
            "{refused:?}"
        );

        // `experiment` has a commit that main does not have: deleting it needs confirmation.
        let check = b
            .start_delete("experiment".to_owned(), tip(&r, &mut s, "experiment"))
            .unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, Op::Delete, check),
            Received::Confirm
        ));
        let Some(Dialog::Delete(DeleteDialog {
            phase: Phase::Confirm { reason },
            ..
        })) = b.dialog()
        else {
            panic!()
        };
        assert!(reason.contains("is not merged"), "{reason}");
        let send_params = b.confirm_delete().unwrap();
        let deleted = done(send(&mut b, &r, &mut s, Op::Delete, send_params));
        assert!(!has_branch(&deleted, "experiment"));
        assert!(deleted.notice.contains("unmerged"), "{}", deleted.notice);
        assert_eq!(
            deleted.history.next_undo.is_some(),
            deleted.history.can_undo
        );

        // Undo recreates it.
        r.run("edit.undo", &mut s, json!({})).unwrap();
        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", &mut s, json!({})).unwrap()).unwrap();
        assert!(refs.iter().any(|i| i.name == "refs/heads/experiment"));

        // The confirmation is valid for the shown commit: if in the meantime the branch was recreated
        // elsewhere, `confirm: true` is refused and the branch stays.
        let seen = tip(&r, &mut s, "experiment");
        let check = b.start_delete("experiment".to_owned(), seen).unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, Op::Delete, check),
            Received::Confirm
        ));
        r.run(
            "branch.delete",
            &mut s,
            json!({ "name": "experiment", "confirm": true }),
        )
        .unwrap();
        r.run(
            "branch.create",
            &mut s,
            json!({ "name": "experiment", "start": graph_branch.target.to_string() }),
        )
        .unwrap();
        let confirmed = b.confirm_delete().unwrap();
        let refused = send(&mut b, &r, &mut s, Op::Delete, confirmed);
        let Received::Failed { message, .. } = refused else {
            panic!("{refused:?}")
        };
        assert!(message.contains("Cannot delete"), "{message}");
        assert_eq!(tip(&r, &mut s, "experiment"), graph_branch.target);
    }

    /// Checkout with the sample repo: refused because of local changes (with the files), then
    /// successful with a clean worktree, and Undo goes back to the previous branch.
    #[test]
    fn checkout_with_the_sample_repo() {
        let (r, mut s) = open(MemoryBackend::sample());
        let mut b = BranchBox::default();
        let params = b.start_checkout("feature/graph".to_owned()).unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, Op::Checkout, params),
            Received::Refused
        ));
        let Some(Dialog::Refused(refused)) = b.dialog() else {
            panic!()
        };
        assert!(!refused.files.is_empty());
        assert!(
            refused.files.iter().any(|f| f == "README.md"),
            "{refused:?}"
        );
        b.close();

        let mut clean = MemoryBackend::sample();
        clean.set_entries(Vec::new());
        let (r, mut s) = open(clean);
        let params = b.start_checkout("feature/graph".to_owned()).unwrap();
        let out = done(send(&mut b, &r, &mut s, Op::Checkout, params));
        let status = out.status.unwrap();
        assert!(
            matches!(&status.head, project_domain::Head::Branch { name, .. } if name == "feature/graph"),
            "{:?}",
            status.head
        );
        assert!(
            out.notice.starts_with("Switched to branch “feature/graph”"),
            "{}",
            out.notice
        );
        assert!(out.history.can_undo);

        // The branch is already active: nothing changes.
        let params = b.start_checkout("feature/graph".to_owned()).unwrap();
        let out = done(send(&mut b, &r, &mut s, Op::Checkout, params));
        assert!(
            out.notice.contains("already the current branch"),
            "{}",
            out.notice
        );

        // Deleting the active branch is refused.
        let params = b
            .start_delete("feature/graph".to_owned(), tip(&r, &mut s, "feature/graph"))
            .unwrap();
        let r2 = send(&mut b, &r, &mut s, Op::Delete, params);
        assert!(
            matches!(r2, Received::Failed { reload: false, .. }),
            "{r2:?}"
        );

        let err = r.run("edit.undo", &mut s, json!({})).map(|_| ());
        assert!(err.is_ok(), "{err:?}");
        let status: Status =
            serde_json::from_value(r.run("repo.status", &mut s, json!({})).unwrap()).unwrap();
        assert!(
            matches!(&status.head, project_domain::Head::Branch { name, .. } if name == "main"),
            "{:?}",
            status.head
        );
    }
}
