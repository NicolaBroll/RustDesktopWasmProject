//! Linked worktrees (S64): the "Worktrees" section of the refs column, shown only when the
//! repository has more than one worktree (`worktree.list`); the "New worktree" dialog
//! (`worktree.add`: an existing branch, a new branch or a detached commit, in an absent or empty
//! folder), which opens the new worktree in a tab; "Remove…" (`worktree.remove`), in two steps as
//! the deletion of untracked files: first without `confirm` (the engine refuses or asks for
//! confirmation, saying whether a backup is saved), then, only after the dialog, with
//! `"confirm": true`. Data only, no egui: tested with unit tests. Command contract in
//! `impl-docs/git/worktrees.md`.
//!
//! The list is read when the repo opens, on "Refresh" and when the refs are read again while the
//! section is shown; `worktree.add` and `worktree.remove` give it back with their result.

use crate::{Done, ErrorKind, Request, Tab, messages_text};
use project_domain::{HEADS_PREFIX, Oid, RefKind};
use project_engine::{Message, WorktreeInfo, WorktreeRemove};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::Path;

/// The id of the command that lists the worktrees.
pub const LIST: &str = "worktree.list";
/// The id of the command that creates one.
pub const ADD: &str = "worktree.add";
/// The id of the command that removes one.
pub const REMOVE: &str = "worktree.remove";

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Opens the "New worktree" dialog; with a local branch (short name) from its menu: that
    /// branch checked out there, or a new branch starting from it if it is checked out already.
    OpenAdd(Option<String>),
    /// The fields of the dialog, as edited.
    Fields(AddFields),
    /// "Create": sends `worktree.add`.
    Submit,
    /// Closes the dialog.
    CloseAdd,
    /// "Remove…" of that worktree (its path as listed): the engine checks first.
    Remove(String),
    /// Confirms the removal: resends with `"confirm": true`.
    ConfirmRemove,
    /// Closes the removal dialog without removing anything.
    CancelRemove,
}

/// What the new worktree checks out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// An existing local branch, checked out in no worktree.
    Existing,
    /// A new branch at `start` (HEAD if empty).
    #[default]
    New,
    /// A detached HEAD at `start` (HEAD if empty).
    Detached,
}

/// The fields of the "New worktree" dialog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddFields {
    /// The folder: absolute, or relative to the root of the open worktree.
    pub path: String,
    pub mode: Mode,
    /// The existing branch ([`Mode::Existing`]), short name.
    pub branch: String,
    /// The name of the new branch ([`Mode::New`]).
    pub new_branch: String,
    /// Where the new branch or the detached HEAD starts; empty: HEAD.
    pub start: String,
}

/// The "New worktree" dialog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AddDialog {
    pub fields: AddFields,
    /// `worktree.add` was sent and has not answered yet.
    pub running: bool,
    /// Why the last attempt failed (the engine's text).
    pub error: Option<String>,
}

impl AddDialog {
    /// What is missing before "Create", if anything.
    pub fn problem(&self) -> Option<String> {
        let f = &self.fields;
        if f.path.trim().is_empty() {
            return Some(tr!("worktree-add-need-path"));
        }
        match f.mode {
            Mode::Existing if f.branch.is_empty() => Some(tr!("worktree-add-need-branch")),
            Mode::New if f.new_branch.trim().is_empty() => Some(tr!("worktree-add-need-name")),
            _ => None,
        }
    }

    /// True if "Create" can be pressed.
    pub fn can_submit(&self) -> bool {
        !self.running && self.problem().is_none()
    }

    /// The parameters of `worktree.add`.
    pub fn params(&self) -> Value {
        let f = &self.fields;
        let mut params = json!({ "path": f.path.trim() });
        let start = f.start.trim();
        match f.mode {
            Mode::Existing => params["branch"] = json!(f.branch),
            Mode::New => params["new_branch"] = json!(f.new_branch.trim()),
            Mode::Detached => {}
        }
        if f.mode != Mode::Existing && !start.is_empty() {
            params["start"] = json!(start);
        }
        params
    }
}

/// Where a removal stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemovePhase {
    /// Sent without `confirm`: the engine checks.
    Checking,
    /// The confirmation dialog is open with the engine's reason (whether a backup is saved).
    Confirm(String),
    /// Sent with `"confirm": true`.
    Sending,
    /// Refused or failed: the dialog shows the engine's text, with only "Close".
    Refused(String),
}

/// A removal in progress, to confirm or refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remove {
    /// The path as `worktree.list` gives it.
    pub path: String,
    pub phase: RemovePhase,
}

/// Result of `worktree.list`.
#[derive(Debug, Deserialize)]
struct ListResult {
    worktrees: Vec<WorktreeInfo>,
}

/// Result of `worktree.add` (the fields the UI reads).
#[derive(Debug, Deserialize)]
pub struct AddResult {
    pub path: String,
    #[serde(default)]
    pub warnings: Vec<Message>,
    pub branch: Option<String>,
    pub created_branch: bool,
    pub head: Oid,
    pub worktrees: Option<Vec<WorktreeInfo>>,
    #[serde(default)]
    pub worktrees_error: Option<String>,
    #[serde(flatten)]
    pub journal: project_engine::JournalState,
}

/// Result of `worktree.remove` (the fields the UI reads).
#[derive(Debug, Deserialize)]
pub struct RemoveResult {
    pub path: String,
    pub backup: Option<String>,
    pub worktrees: Option<Vec<WorktreeInfo>>,
    #[serde(default)]
    pub worktrees_error: Option<String>,
}

/// The texts of a worktree shown by the UI.
pub trait WorktreeText {
    /// The last part of the path (the folder name), for the row.
    fn short_path(&self) -> String;
    /// The branch checked out (short name) or "detached at `<id>`".
    fn what(&self) -> String;
    /// The row tooltip: full path, what is checked out, the states.
    fn tooltip(&self) -> String;
}

impl WorktreeText for WorktreeInfo {
    fn short_path(&self) -> String {
        let trimmed = self.path.trim_end_matches(['/', '\\']);
        let name = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
        if name.is_empty() {
            self.path.clone()
        } else {
            name.to_owned()
        }
    }

    fn what(&self) -> String {
        match (&self.branch, self.head) {
            _ if self.bare => tr!("worktree-bare"),
            (Some(branch), _) => branch
                .strip_prefix(HEADS_PREFIX)
                .unwrap_or(branch)
                .to_owned(),
            (None, Some(head)) => tr!("worktree-detached", id = head.short()),
            (None, None) => tr!("worktree-no-commit"),
        }
    }

    fn tooltip(&self) -> String {
        let mut lines = vec![self.path.clone(), self.what()];
        if self.main {
            lines.push(tr!("worktree-tooltip-main"));
        }
        if self.current {
            lines.push(tr!("worktree-tooltip-current"));
        }
        if self.locked {
            lines.push(match &self.lock_reason {
                Some(reason) => tr!("worktree-tooltip-locked-reason", reason = reason),
                None => tr!("worktree-tooltip-locked"),
            });
        }
        if self.prunable {
            lines.push(tr!("worktree-tooltip-prunable"));
        }
        lines.join("\n")
    }
}

/// Why "Remove…" is off for this worktree, if it is (the engine would refuse it at once).
pub fn remove_blocked(info: &WorktreeInfo) -> Option<String> {
    if info.main {
        Some(tr!("worktree-remove-main"))
    } else if info.current {
        Some(tr!("worktree-remove-current"))
    } else {
        None
    }
}

/// A folder next to the repo for a new worktree: `<parent>/<repo folder>-<name>` (`/` of a
/// branch name becomes `-`); empty if the repo path is not absolute (a relative proposal would
/// land inside the repo) or has no parent.
pub fn proposed_path(repo: &str, name: &str) -> String {
    let repo = Path::new(repo);
    if !repo.is_absolute() {
        return String::new();
    }
    let (Some(parent), Some(folder)) = (repo.parent(), repo.file_name()) else {
        return String::new();
    };
    let name: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\') { '-' } else { c })
        .collect();
    let folder = format!("{}-{name}", folder.to_string_lossy());
    parent.join(folder).to_string_lossy().into_owned()
}

/// What came back from `worktree.add`.
#[derive(Debug)]
pub enum Added {
    /// Created: the notice for the status bar and the path to open.
    Done(Box<AddResult>),
    /// Failed: the dialog shows why.
    Failed,
}

/// What came back from `worktree.remove`.
#[derive(Debug)]
pub enum Removed {
    /// The confirmation dialog opens (or the refusal is shown): nothing else to do.
    Shown,
    /// Removed.
    Done(Box<RemoveResult>),
    /// Failed after the confirmation: the list must be read again.
    Failed,
}

/// The worktrees of the repository, the "New worktree" dialog and the removal.
#[derive(Debug, Default)]
pub struct WorktreeBox {
    /// `None` until the first answer.
    list: Option<Vec<WorktreeInfo>>,
    list_error: Option<String>,
    /// A `worktree.list` was sent and has not answered yet.
    loading: bool,
    dialog: Option<AddDialog>,
    remove: Option<Remove>,
}

impl WorktreeBox {
    /// The worktrees, once read.
    pub fn list(&self) -> Option<&[WorktreeInfo]> {
        self.list.as_deref()
    }

    /// True if the section is shown: more than one worktree.
    pub fn shown(&self) -> bool {
        self.list.as_ref().is_some_and(|l| l.len() > 1)
    }

    /// Why the list could not be read.
    pub fn list_error(&self) -> Option<&str> {
        self.list_error.as_deref()
    }

    /// The worktree (path) where the local branch `name` is checked out, if any.
    pub fn branch_in_use(&self, name: &str) -> Option<&str> {
        let full = format!("{HEADS_PREFIX}{name}");
        self.list
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find(|w| w.branch.as_deref() == Some(full.as_str()))
            .map(|w| w.path.as_str())
    }

    /// The open "New worktree" dialog.
    pub fn dialog(&self) -> Option<&AddDialog> {
        self.dialog.as_ref()
    }

    /// The removal dialog, when it has something to show (confirmation or refusal).
    pub fn remove_dialog(&self) -> Option<&Remove> {
        self.remove
            .as_ref()
            .filter(|r| matches!(r.phase, RemovePhase::Confirm(_) | RemovePhase::Refused(_)))
    }

    /// True if one of the dialogs is open.
    pub fn modal_open(&self) -> bool {
        self.dialog.is_some() || self.remove.is_some()
    }

    /// Marks `worktree.list` as sent.
    pub fn list_sent(&mut self) {
        self.loading = true;
    }

    /// True if a `worktree.list` is on its way.
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// The answer of `worktree.list`. A failure keeps the last list (with the error).
    pub fn receive_list(&mut self, result: Result<Value, String>) {
        self.loading = false;
        match result.and_then(|value| crate::decode::<ListResult>(LIST, value)) {
            Ok(ListResult { worktrees }) => self.set_list(Some(worktrees), None),
            Err(e) => self.list_error = Some(e),
        }
    }

    /// The list returned by `worktree.add`/`worktree.remove` (`None` with the error).
    fn set_list(&mut self, list: Option<Vec<WorktreeInfo>>, error: Option<String>) {
        match list {
            Some(list) => {
                self.list = Some(list);
                self.list_error = None;
            }
            None => self.list_error = error,
        }
    }

    /// Opens the dialog with `fields`, unless one is open.
    pub fn open(&mut self, fields: AddFields) {
        if self.dialog.is_none() {
            self.dialog = Some(AddDialog {
                fields,
                ..AddDialog::default()
            });
        }
    }

    /// The fields edited in the dialog (not while it is sending).
    pub fn set_fields(&mut self, fields: AddFields) {
        if let Some(dialog) = self.dialog.as_mut().filter(|d| !d.running) {
            dialog.fields = fields;
        }
    }

    /// The folder chosen with "Browse…".
    pub fn set_folder(&mut self, folder: &str) {
        if let Some(dialog) = self.dialog.as_mut().filter(|d| !d.running) {
            dialog.fields.path = folder.to_owned();
        }
    }

    /// Closes the dialog (not while it is sending).
    pub fn close(&mut self) {
        if self.dialog.as_ref().is_some_and(|d| !d.running) {
            self.dialog = None;
        }
    }

    /// "Create": the parameters of `worktree.add`, if they are complete.
    pub fn submit(&mut self) -> Option<Value> {
        let dialog = self.dialog.as_mut().filter(|d| d.can_submit())?;
        dialog.running = true;
        dialog.error = None;
        Some(dialog.params())
    }

    /// The answer of `worktree.add`; `detail`: git's text, if any.
    pub fn receive_add(&mut self, result: Result<Value, String>, detail: Option<String>) -> Added {
        let error = match result.and_then(|v| crate::decode::<AddResult>(ADD, v)) {
            Ok(result) => {
                self.dialog = None;
                self.set_list(result.worktrees.clone(), result.worktrees_error.clone());
                return Added::Done(Box::new(result));
            }
            Err(error) => with_detail(error, detail),
        };
        match &mut self.dialog {
            Some(dialog) => {
                dialog.running = false;
                dialog.error = Some(error);
            }
            None => {
                self.dialog = Some(AddDialog {
                    error: Some(error),
                    ..AddDialog::default()
                });
            }
        }
        Added::Failed
    }

    /// "Remove…": the parameters of the check (without `confirm`), unless a removal is open.
    pub fn start_remove(&mut self, path: String) -> Option<Value> {
        if self.remove.is_some() {
            return None;
        }
        let params = remove_params(&path, false);
        self.remove = Some(Remove {
            path,
            phase: RemovePhase::Checking,
        });
        Some(params)
    }

    /// The user confirmed: the parameters with `"confirm": true`.
    pub fn confirm_remove(&mut self) -> Option<Value> {
        let remove = self.remove.as_mut()?;
        if !matches!(remove.phase, RemovePhase::Confirm(_)) {
            return None;
        }
        remove.phase = RemovePhase::Sending;
        Some(remove_params(&remove.path, true))
    }

    /// Closes the removal dialog (confirmation or refusal) without removing anything.
    pub fn cancel_remove(&mut self) {
        if self.remove_dialog().is_some() {
            self.remove = None;
        }
    }

    /// The answer of `worktree.remove`, in the phase it was in.
    pub fn receive_remove(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Removed {
        let Some(remove) = self.remove.take() else {
            return Removed::Failed;
        };
        let checking = remove.phase == RemovePhase::Checking;
        let error = match result {
            Err(e) if kind == Some(ErrorKind::ConfirmRequired) && checking => {
                let reason = detail.filter(|d| !d.trim().is_empty()).unwrap_or(e);
                self.remove = Some(Remove {
                    path: remove.path,
                    phase: RemovePhase::Confirm(reason),
                });
                return Removed::Shown;
            }
            Err(e) => with_detail(e, detail),
            Ok(value) => match crate::decode::<RemoveResult>(REMOVE, value) {
                Ok(result) => {
                    self.set_list(result.worktrees.clone(), result.worktrees_error.clone());
                    return Removed::Done(Box::new(result));
                }
                Err(e) => e,
            },
        };
        self.remove = Some(Remove {
            path: remove.path,
            phase: RemovePhase::Refused(error),
        });
        if checking {
            Removed::Shown
        } else {
            Removed::Failed
        }
    }
}

/// The parameters of `worktree.remove`.
fn remove_params(path: &str, confirm: bool) -> Value {
    crate::params(WorktreeRemove {
        path: path.to_owned(),
        confirm,
    })
}

/// `error` followed by git's text, if it adds something.
fn with_detail(error: String, detail: Option<String>) -> String {
    match detail {
        Some(d) if !d.trim().is_empty() && !error.contains(d.trim()) => {
            format!("{error}: {}", d.trim())
        }
        _ => error,
    }
}

/// The notice after `worktree.add`.
pub fn added_notice(result: &AddResult) -> String {
    let mut text = match (&result.branch, result.created_branch) {
        (Some(branch), true) => tr!(
            "worktree-added-new-branch",
            path = &result.path,
            branch = branch.strip_prefix(HEADS_PREFIX).unwrap_or(branch)
        ),
        (Some(branch), false) => tr!(
            "worktree-added-branch",
            path = &result.path,
            branch = branch.strip_prefix(HEADS_PREFIX).unwrap_or(branch)
        ),
        (None, _) => tr!(
            "worktree-added-detached",
            path = &result.path,
            id = result.head.short()
        ),
    };
    text.push_str(&messages_text(&result.warnings));
    text
}

/// The notice after `worktree.remove`.
pub fn removed_notice(result: &RemoveResult) -> String {
    match &result.backup {
        Some(backup) => tr!(
            "worktree-removed-backup",
            path = &result.path,
            backup = backup
        ),
        None => tr!("worktree-removed", path = &result.path),
    }
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_worktree(&mut self, action: Action) {
        let generation = self.generation;
        let writing = self.executor.writing().is_some();
        let Some(repo) = &mut self.repo else {
            return;
        };
        match action {
            Action::OpenAdd(branch) => {
                if writing || repo.modal_open() {
                    return;
                }
                let fields = add_fields(repo, branch);
                repo.worktrees.open(fields);
            }
            Action::Fields(fields) => repo.worktrees.set_fields(fields),
            Action::Submit => {
                if writing {
                    return;
                }
                if let Some(params) = repo.worktrees.submit() {
                    repo.write_error = None;
                    repo.write_label = Some(tr!("worktree-add-label"));
                    self.submit(ADD, params, Request::Worktree(generation));
                }
            }
            Action::CloseAdd => repo.worktrees.close(),
            Action::Remove(path) => {
                if writing || repo.modal_open() {
                    return;
                }
                let label = tr!("worktree-remove-check-label", path = &path);
                if let Some(params) = repo.worktrees.start_remove(path) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(REMOVE, params, Request::Worktree(generation));
                }
            }
            Action::ConfirmRemove => {
                if writing {
                    return;
                }
                let label = repo
                    .worktrees
                    .remove_dialog()
                    .map(|r| tr!("worktree-remove-label", path = &r.path));
                if let Some(params) = repo.worktrees.confirm_remove() {
                    repo.write_label = label;
                    self.submit(REMOVE, params, Request::Worktree(generation));
                }
            }
            Action::CancelRemove => repo.worktrees.cancel_remove(),
        }
    }

    /// The folder chosen with "Browse…" of the "New worktree" dialog.
    pub(crate) fn worktree_folder(&mut self, folder: &str) {
        if let Some(repo) = &mut self.repo {
            repo.worktrees.set_folder(folder);
        }
    }

    /// Asks for `worktree.list`, unless one is on its way.
    pub(crate) fn request_worktrees(&mut self, generation: u64) {
        if let Some(repo) = self.current(generation)
            && !repo.worktrees.loading()
        {
            repo.worktrees.list_sent();
            self.submit(LIST, json!({}), Request::Worktrees(generation));
        }
    }

    /// Response of `worktree.add` or `worktree.remove`.
    pub(crate) fn receive_worktree(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        if done.command == ADD {
            match repo.worktrees.receive_add(done.result, done.error_detail) {
                Added::Done(result) => {
                    log::info!("{ADD}: {}", result.path);
                    repo.history = result.journal.clone().into();
                    self.notice = added_notice(&result);
                    self.error.clear();
                    // `App` opens it in a new tab.
                    self.open_worktree = Some(result.path.clone());
                    if result.created_branch {
                        self.submit("repo.refs", json!({}), Request::Refs(generation));
                    }
                }
                Added::Failed => {
                    // The dialog shows why; the list may have changed (a hook failed after it).
                    self.request_worktrees(generation);
                }
            }
            return;
        }
        match repo
            .worktrees
            .receive_remove(done.result, done.error_kind, done.error_detail)
        {
            Removed::Shown => {}
            Removed::Done(result) => {
                log::info!("{REMOVE}: {}", result.path);
                self.notice = removed_notice(&result);
                self.error.clear();
            }
            Removed::Failed => {
                log::warn!("{REMOVE}: failed");
                self.request_worktrees(generation);
            }
        }
    }
}

/// The fields of the dialog opened from the menu of the local branch `branch` (`None`: the
/// section header): that branch if it is checked out nowhere, otherwise a new branch from it;
/// the folder proposed next to the repo.
fn add_fields(repo: &crate::RepoView, branch: Option<String>) -> AddFields {
    let Some(branch) = branch else {
        return AddFields::default();
    };
    let exists = repo
        .refs
        .iter()
        .any(|r| r.kind == RefKind::LocalBranch && r.short_name() == branch);
    let in_use = repo.worktrees.branch_in_use(&branch).is_some()
        || crate::view::current_branch(&repo.head) == Some(branch.as_str());
    let path = proposed_path(&repo.path, &branch);
    if exists && !in_use {
        AddFields {
            path,
            mode: Mode::Existing,
            branch,
            ..AddFields::default()
        }
    } else {
        AddFields {
            path,
            mode: Mode::New,
            start: branch,
            ..AddFields::default()
        }
    }
}

#[cfg(test)]
#[path = "worktree_tests.rs"]
mod tests;
