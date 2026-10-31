//! Submodules (S62): the "Submodules" section of the refs column, shown only when the repo has
//! any, with each submodule's state from `submodule.list`; "Update", "Initialize and update" and
//! "Update all" send `submodule.update` in the background like the fetch (progress, "Stop", the
//! credential dialog), then the status and the list are read again. Data only, no egui: tested
//! with unit tests. Command contract in `impl-docs/git/submodules.md`.
//!
//! The list is read when the repo opens and on "Refresh"; afterwards, with every new status, only
//! if the repo has submodules or the status shows `.gitmodules` or a submodule changed: a repo
//! without submodules never pays for it again.
//!
//! After a command that can move the recorded commits (checkout, pull, merge, reset, stash
//! apply/pop, rebase, cherry-pick, revert, undo/redo: [`moves_submodules`]) the list is read
//! again (S62b): the submodules that now are on another commit than the recorded one, and were
//! not like that already, give the notice "N submodules are not at the recorded commit" with
//! "Update" of exactly those. One notice per command, until dismissed or updated.

use crate::running::Running;
use crate::{Done, ErrorKind, Request, Tab, Ticket, messages_text, params};
use project_domain::{RepoPath, Status};
use project_engine::{
    SubmoduleInfo, SubmoduleRefusal, SubmoduleState, SubmoduleUpdate, SubmoduleUpdated,
};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// `submodule.update` of that submodule (`None`: all); with `init` the uninitialized ones are
    /// cloned too.
    Update { path: Option<RepoPath>, init: bool },
    /// "Update" of the notice: `submodule.update` of the submodules it names.
    UpdateOutdated,
    /// Closes the notice.
    Dismiss,
    /// "Stop" the update in progress.
    Cancel,
}

/// The commands after which the recorded commits of the submodules may have changed while the
/// submodules stayed where they were (we never pass `--recurse-submodules`).
const MOVERS: &[&str] = &[
    "branch.checkout",
    "branch.reset",
    "commit.checkout",
    "commit.cherry_pick",
    "commit.revert",
    "remote.pull",
    "merge.start",
    "merge.continue",
    "merge.abort",
    "stash.apply",
    "stash.pop",
    "rebase.start",
    "rebase.continue",
    "rebase.abort",
    "cherry_pick.continue",
    "cherry_pick.abort",
    "revert.continue",
    "revert.abort",
    "edit.undo",
    "edit.redo",
];

/// True if `command` can leave submodules on another commit than the recorded one.
pub(crate) fn moves_submodules(command: &str) -> bool {
    MOVERS.contains(&command)
}

/// The id of the command that lists the submodules.
pub const LIST: &str = "submodule.list";
/// The id of the command that updates them.
pub const UPDATE: &str = "submodule.update";

/// The `.gitmodules` file: when the status shows it changed, the list is read again.
const GITMODULES: &[u8] = b".gitmodules";

/// Result of `submodule.list`.
#[derive(Debug, Deserialize)]
struct ListResult {
    submodules: Vec<SubmoduleInfo>,
}

/// The texts of a submodule shown by the UI.
pub trait SubmoduleText {
    /// The path, as text.
    fn path_text(&self) -> String;
    /// The short state label of the badge.
    fn badge(&self) -> String;
    /// The row tooltip: URL, recorded and checked out commit, what is in the way of an update.
    fn tooltip(&self) -> String;
    /// True if "Initialize and update" is the action (not checked out yet).
    fn needs_init(&self) -> bool;
}

impl SubmoduleText for SubmoduleInfo {
    fn path_text(&self) -> String {
        self.path.to_string()
    }

    fn badge(&self) -> String {
        match self.state {
            SubmoduleState::Conflict => tr!("submodule-state-conflict"),
            SubmoduleState::Uninitialized => tr!("submodule-state-uninitialized"),
            SubmoduleState::LocalChanges => tr!("submodule-state-local-changes"),
            SubmoduleState::DifferentCommit => tr!("submodule-state-different-commit"),
            SubmoduleState::InSync => tr!("submodule-state-in-sync"),
        }
    }

    fn tooltip(&self) -> String {
        let mut lines = vec![tr!("submodule-tooltip-path", path = self.path_text())];
        lines.push(match &self.url {
            Some(url) => tr!("submodule-tooltip-url", url = url),
            None => tr!("submodule-tooltip-no-url"),
        });
        lines.push(match self.recorded {
            Some(id) => tr!("submodule-tooltip-recorded", id = id.short()),
            None => tr!("submodule-tooltip-recorded-conflict"),
        });
        let head = self.head.filter(|_| !self.needs_init());
        lines.push(match (head, self.detached) {
            (Some(id), true) => tr!("submodule-tooltip-head-detached", id = id.short()),
            (Some(id), false) => tr!("submodule-tooltip-head-branch", id = id.short()),
            (None, _) => tr!("submodule-tooltip-not-checked-out"),
        });
        lines.push(match self.state {
            SubmoduleState::Conflict => tr!("submodule-explain-conflict"),
            SubmoduleState::Uninitialized => tr!("submodule-explain-uninitialized"),
            SubmoduleState::LocalChanges => tr!("submodule-explain-local-changes"),
            SubmoduleState::DifferentCommit => tr!("submodule-explain-different-commit"),
            SubmoduleState::InSync => tr!("submodule-explain-in-sync"),
        });
        if self.changes {
            lines.push(tr!("submodule-tooltip-changes"));
        }
        if self.untracked {
            lines.push(tr!("submodule-tooltip-untracked"));
        }
        if let Some(operation) = self.operation {
            lines.push(tr!(
                "submodule-tooltip-operation",
                operation = crate::merge::operation_name(operation)
            ));
        }
        if self.unreferenced {
            lines.push(tr!("submodule-tooltip-unreferenced"));
        }
        if self.ignored_overwritten {
            lines.push(tr!("submodule-tooltip-ignored"));
        }
        if self.name.is_none() {
            lines.push(tr!("submodule-tooltip-no-mapping"));
        }
        lines.join("\n")
    }

    fn needs_init(&self) -> bool {
        self.state == SubmoduleState::Uninitialized
    }
}

/// What came back from `submodule.update`.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    /// Ran: the notice with the counts, and the refused submodules and warnings (empty if none),
    /// for the status bar.
    Done { notice: String, problems: String },
    /// Stopped with "Stop" or the credential question cancelled: the notice.
    Cancelled(String),
    /// Failed or refused: the message for the status bar.
    Failed(String),
}

/// The submodules of the repo and the update in progress.
#[derive(Debug, Default)]
pub struct SubmoduleBox {
    /// `None` until the first answer.
    list: Option<Vec<SubmoduleInfo>>,
    list_error: Option<String>,
    /// A `submodule.list` was sent and has not answered yet.
    loading: bool,
    /// The update in progress, with its description for the bar.
    running: Option<Running<String>>,
    /// What the last update could not do (refused submodules, warnings, the failure): shown in
    /// the section until the next update, since the reads that follow clear the status bar error.
    problems: Option<String>,
    /// The submodules of the notice: left on another commit than the recorded one by the last
    /// command that moves them; empty: no notice.
    outdated: Vec<RepoPath>,
    /// The list as it was when such a command answered, for the check that follows (the
    /// command's own refresh may read the list again before it).
    baseline: Option<Vec<SubmoduleInfo>>,
}

impl SubmoduleBox {
    /// The submodules, once read.
    pub fn list(&self) -> Option<&[SubmoduleInfo]> {
        self.list.as_deref()
    }

    /// True if the section is shown: the repo has submodules (or the list could not be read
    /// after having had some).
    pub fn shown(&self) -> bool {
        self.list.as_ref().is_some_and(|l| !l.is_empty())
    }

    /// Why the list could not be read.
    pub fn list_error(&self) -> Option<&str> {
        self.list_error.as_deref()
    }

    /// What the last update could not do, if anything.
    pub fn problems(&self) -> Option<&str> {
        self.problems.as_deref()
    }

    /// The submodules of the "not at the recorded commit" notice (empty: no notice).
    pub fn outdated(&self) -> &[RepoPath] {
        &self.outdated
    }

    /// Closes the notice.
    pub fn dismiss(&mut self) {
        self.outdated.clear();
    }

    /// The update in progress, if any.
    pub fn running(&self) -> Option<&Running<String>> {
        self.running.as_ref()
    }

    /// Marks `submodule.list` as sent.
    pub fn list_sent(&mut self) {
        self.loading = true;
    }

    /// True if the list must be read again after this new status: the repo has submodules, or
    /// the status shows `.gitmodules` or a gitlink changed (one was added); not while a read
    /// is already on its way.
    pub fn wants_list(&self, status: &Status) -> bool {
        if self.loading {
            return false;
        }
        self.shown()
            || status
                .entries
                .iter()
                .any(|e| e.path.as_bytes() == GITMODULES || e.submodule().is_some())
    }

    /// The answer of `submodule.list`. A failure keeps the last list (with the error). With
    /// `check` (read after a command of [`moves_submodules`]) the submodules that now are on
    /// another commit than the recorded one, and were not on that same pair before, become the
    /// notice; every list drops from the notice the ones no longer out of date.
    pub fn receive_list(&mut self, result: Result<Value, String>, check: bool) {
        self.loading = false;
        match result.and_then(|value| crate::decode::<ListResult>(LIST, value)) {
            Ok(ListResult { submodules }) => {
                let outdated = |s: &&SubmoduleInfo| s.state == SubmoduleState::DifferentCommit;
                if check {
                    let baseline = self.baseline.take();
                    let before = baseline.as_deref().or(self.list.as_deref());
                    let before = before.unwrap_or_default();
                    let new: Vec<RepoPath> = submodules
                        .iter()
                        .filter(outdated)
                        .filter(|s| {
                            !before.iter().any(|b| {
                                b.path == s.path
                                    && b.state == SubmoduleState::DifferentCommit
                                    && b.recorded == s.recorded
                                    && b.head == s.head
                            })
                        })
                        .map(|s| s.path.clone())
                        .collect();
                    if !new.is_empty() {
                        self.outdated = new;
                    }
                }
                self.outdated
                    .retain(|p| submodules.iter().filter(outdated).any(|s| &s.path == p));
                self.list = Some(submodules);
                self.list_error = None;
            }
            Err(e) => self.list_error = Some(e),
        }
    }

    /// A command of [`moves_submodules`] answered: the list as it is now is what the check
    /// compares with.
    pub fn command_done(&mut self) {
        self.baseline = self.list.clone();
    }

    /// The parameters of `submodule.update` for `paths` (`None`: all), if no update is running.
    pub fn request(&self, paths: Option<Vec<RepoPath>>, init: bool) -> Option<Value> {
        if self.running.is_some() {
            return None;
        }
        Some(params(SubmoduleUpdate {
            paths,
            init,
            recursive: false,
        }))
    }

    /// The description of an update for the bar.
    pub fn label(path: Option<&RepoPath>, init: bool) -> String {
        match (path, init) {
            (Some(path), true) => tr!("submodule-init-label", path = path.to_string()),
            (Some(path), false) => tr!("submodule-update-label", path = path.to_string()),
            (None, _) => tr!("submodule-update-all-label"),
        }
    }

    /// The update requested with [`SubmoduleBox::request`] started with `ticket`.
    pub fn started(&mut self, ticket: Ticket, label: String) {
        self.running = Some(Running::with(ticket, label));
        self.problems = None;
    }

    /// "Stop": the ticket to cancel, only once.
    pub fn cancel(&mut self) -> Option<Ticket> {
        self.running.as_mut()?.cancel()
    }

    /// The answer of `submodule.update` for `ticket`; `detail`: git's text, if any.
    pub fn receive(
        &mut self,
        ticket: Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let received = self.received(ticket, result, kind, detail);
        self.problems = match &received {
            Received::Done { problems, .. } if !problems.is_empty() => Some(problems.clone()),
            Received::Failed(message) => Some(message.clone()),
            _ => None,
        };
        received
    }

    fn received(
        &mut self,
        ticket: Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let label = match self.running.take() {
            Some(running) if running.ticket == ticket => running.extra,
            other => {
                self.running = other;
                return Received::Failed(tr!("common-unexpected-response", command = UPDATE));
            }
        };
        let error = match result {
            Ok(value) => {
                return match serde_json::from_value::<SubmoduleUpdated>(value) {
                    Ok(result) => outcome(&result),
                    Err(e) => Received::Failed(crate::unexpected_result(UPDATE, e)),
                };
            }
            Err(error) => error,
        };
        match kind {
            Some(ErrorKind::Cancelled) => {
                Received::Cancelled(tr!("submodule-update-cancelled", label = &label))
            }
            Some(ErrorKind::AuthCancelled) => {
                Received::Cancelled(crate::fetch::auth_cancelled(detail.as_deref()))
            }
            _ => {
                let error = match detail {
                    Some(d) if !d.trim().is_empty() && !error.contains(d.trim()) => {
                        format!("{error}: {}", d.trim())
                    }
                    _ => error,
                };
                Received::Failed(tr!("common-failed", label = &label, error = &error))
            }
        }
    }
}

/// The notice and the problems of a completed update.
fn outcome(result: &SubmoduleUpdated) -> Received {
    let notice = tr!(
        "submodule-update-done",
        updated = result.updated.len(),
        unchanged = result.unchanged.len()
    );
    let mut problems = String::new();
    if !result.refused.is_empty() {
        problems = tr!(
            "submodule-update-refused",
            n = result.refused.len(),
            list = refused_text(&result.refused)
        );
    }
    problems.push_str(&messages_text(&result.warnings));
    Received::Done {
        notice,
        problems: problems.trim_start_matches("; ").to_owned(),
    }
}

/// `lib: it has changes…; vendor: …`: each refused submodule with the engine's reason.
fn refused_text(refused: &[SubmoduleRefusal]) -> String {
    refused
        .iter()
        .map(|r| r.message.text())
        .collect::<Vec<_>>()
        .join("; ")
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_submodule(&mut self, action: Action) {
        match action {
            Action::Update { path, init } => {
                let label = SubmoduleBox::label(path.as_ref(), init);
                self.start_update(path.map(|p| vec![p]), init, label);
            }
            Action::UpdateOutdated => {
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let paths = repo.submodules.outdated().to_vec();
                if paths.is_empty() {
                    return;
                }
                let label = tr!("submodule-outdated-label", n = paths.len());
                if self.start_update(Some(paths), false, label)
                    && let Some(repo) = &mut self.repo
                {
                    repo.submodules.dismiss();
                }
            }
            Action::Dismiss => {
                if let Some(repo) = &mut self.repo {
                    repo.submodules.dismiss();
                }
            }
            Action::Cancel => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.submodules.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
        }
    }

    /// Sends `submodule.update` of `paths` (`None`: all) unless a write or a dialog is in the
    /// way; true if sent.
    fn start_update(&mut self, paths: Option<Vec<RepoPath>>, init: bool, label: String) -> bool {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return false;
        };
        if self.executor.writing().is_some() || repo.modal_open() {
            return false;
        }
        let Some(params) = repo.submodules.request(paths, init) else {
            return false;
        };
        repo.write_error = None;
        repo.write_label = Some(label.clone());
        let ticket = self.executor.submit(UPDATE, params);
        self.pending.insert(ticket, Request::Submodule(generation));
        if let Some(repo) = &mut self.repo {
            repo.submodules.started(ticket, label);
        }
        true
    }

    /// Asks for `submodule.list`; with `check`, for the notice of [`moves_submodules`].
    pub(crate) fn request_submodules(&mut self, generation: u64, check: bool) {
        if let Some(repo) = self.current(generation) {
            repo.submodules.list_sent();
            self.submit(LIST, json!({}), Request::Submodules(generation, check));
        }
    }

    /// A command of [`moves_submodules`] succeeded: if the repo has submodules, the list as it
    /// is now is kept and the list read again for the notice. Called before the command's own
    /// answer is handled (which may read the list again too).
    pub(crate) fn check_submodules(&mut self, generation: u64) {
        if let Some(repo) = self.current(generation)
            && repo.submodules.shown()
        {
            repo.submodules.command_done();
            self.request_submodules(generation, true);
        }
    }

    /// Response of `submodule.update`: the notice (with the refused ones and the warnings), then
    /// status and list read again whatever happened (git may have checked out some before failing).
    pub(crate) fn receive_submodule(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received =
            repo.submodules
                .receive(done.ticket, done.result, done.error_kind, done.error_detail);
        match received {
            Received::Done { notice, problems } => {
                log::info!("{}: {notice} {problems}", done.command);
                // The refused ones also stay in the section (the reads below clear the error).
                self.notice = if problems.is_empty() {
                    notice
                } else {
                    format!("{notice}; {problems}")
                };
                self.error.clear();
            }
            Received::Cancelled(notice) => {
                self.notice = notice;
                self.error.clear();
            }
            Received::Failed(message) => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
            }
        }
        self.submit("repo.status", json!({}), Request::Status(generation));
        self.request_submodules(generation, false);
    }
}

#[cfg(test)]
#[path = "submodule_tests.rs"]
mod tests;
