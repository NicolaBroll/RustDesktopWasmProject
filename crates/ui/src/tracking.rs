//! Tracking branches and upstream (S51): "Check out" on a remote branch (the local branch that
//! tracks it, created with `branch.create` `track` if needed, then `branch.checkout`), and the
//! upstream of a local branch with `branch.set_upstream` ("Set upstream…" window, "Unset
//! upstream"). Data only, no egui, in the same [`BranchBox`] as the other branch dialogs: one at a
//! time. Removing the upstream asks no confirmation: it is undoable.

use crate::branch::{
    self, Action, BranchBox, Dialog, NameKind, Op, Outcome, Received, Start, done, refs_warning,
    unexpected,
};
use crate::running::Running;
use crate::{ErrorKind, Request, Tab, Ticket, messages_text, params};
use project_domain::{REMOTES_PREFIX, RefInfo, RefKind, branch_short};
use project_engine::{BranchCreateParams, BranchSetUpstreamParams, BranchSetUpstreamResult};
use project_i18n::tr;
use serde_json::Value;

/// What "Check out" on a remote branch does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// A local branch already tracks it: switch to that one (short name).
    Checkout(String),
    /// Create the local branch (short name) tracking it, then switch to it.
    Create(String),
    /// The name is taken by a local branch that does not track it: ask for another one.
    Taken(String),
}

/// The plan for the remote branch `full` (`refs/remotes/<remote>/<branch>`): the local branch name
/// is `<branch>` (`origin/feat/x` -> `feat/x`), with the configured `remotes` (whose names may
/// contain `/`); without the list, up to the first `/`. `None` for `<remote>/HEAD` or a ref that
/// is not a remote branch.
pub fn plan(full: &str, refs: &[RefInfo], remotes: &[String]) -> Option<Plan> {
    let short = full.strip_prefix(REMOTES_PREFIX)?;
    let branch = if remotes.is_empty() {
        let (_, branch) = short.split_once('/')?;
        (!branch.is_empty() && branch != "HEAD").then(|| branch.to_owned())?
    } else {
        crate::remotes::split_remote_branch(short, remotes)?.1
    };
    let tracking: Vec<&str> = refs
        .iter()
        .filter(|r| r.kind == RefKind::LocalBranch)
        .filter(|r| r.upstream.as_ref().is_some_and(|u| u.name == full))
        .map(RefInfo::short_name)
        .collect();
    // More branches may track it: the one with the same name first.
    if let Some(local) = tracking.iter().find(|l| **l == branch).or(tracking.first()) {
        return Some(Plan::Checkout((*local).to_owned()));
    }
    let local = format!("refs/heads/{branch}");
    if refs.iter().any(|r| r.name == local) {
        Some(Plan::Taken(branch))
    } else {
        Some(Plan::Create(branch))
    }
}

/// The remote branches that can be an upstream (full names, in the order of the refs): no
/// `<remote>/HEAD`, and with the `remotes` list only the refs of a configured remote.
pub fn upstream_choices(refs: &[RefInfo], remotes: &[String]) -> Vec<String> {
    refs.iter()
        .filter(|r| r.kind == RefKind::RemoteBranch)
        .filter(|r| {
            let short = r.short_name();
            if remotes.is_empty() {
                !short.ends_with("/HEAD")
            } else {
                crate::remotes::split_remote_branch(short, remotes).is_some()
            }
        })
        .map(|r| r.name.clone())
        .collect()
}

/// The choice to preselect for the branch `name`: its upstream if it has one, otherwise
/// `origin/<name>`, otherwise `<remote>/<name>` of the first remote that has it.
pub fn preselect(
    name: &str,
    current: Option<&str>,
    choices: &[String],
    remotes: &[String],
) -> Option<String> {
    if let Some(current) = current.filter(|c| choices.iter().any(|x| x == c)) {
        return Some(current.to_owned());
    }
    let origin = std::iter::once("origin").chain(remotes.iter().map(String::as_str));
    origin
        .map(|remote| format!("{REMOTES_PREFIX}{remote}/{name}"))
        .find(|full| choices.contains(full))
}

/// The "Set upstream" window of a local branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpstreamDialog {
    /// The short name of the local branch.
    pub branch: String,
    /// Its upstream now (full name), if any.
    pub current: Option<String>,
    /// The remote branches (full names).
    pub choices: Vec<String>,
    /// The typed filter.
    pub filter: String,
    /// The picked remote branch (full name).
    pub selected: Option<String>,
    /// The engine error from the last submit.
    pub error: Option<String>,
    pub running: Option<Running>,
}

impl UpstreamDialog {
    /// The choices that match the filter (case-insensitive, on the short name).
    pub fn visible(&self) -> Vec<&str> {
        let filter = self.filter.trim().to_lowercase();
        self.choices
            .iter()
            .map(String::as_str)
            .filter(|c| filter.is_empty() || short(c).to_lowercase().contains(&filter))
            .collect()
    }

    /// True if a branch other than the current upstream is picked and nothing is running.
    pub fn can_submit(&self) -> bool {
        self.running.is_none()
            && self
                .selected
                .as_ref()
                .is_some_and(|s| Some(s) != self.current.as_ref())
    }
}

/// The short name of a remote branch: `refs/remotes/origin/x` -> `origin/x`.
pub fn short(full: &str) -> &str {
    full.strip_prefix(REMOTES_PREFIX).unwrap_or(full)
}

impl BranchBox {
    /// Starts the creation of `name` tracking the remote branch `remote` (full name), to be checked
    /// out afterwards; no dialog.
    pub fn start_track(&mut self, name: String, remote: &str) -> Option<Value> {
        if self.busy() {
            return None;
        }
        let params = params(BranchCreateParams {
            name: name.clone(),
            start: Some(remote.to_owned()),
            track: true,
        });
        self.track = Some(name);
        Some(params)
    }

    /// The name of the tracking branch is taken: the new-branch dialog, prefilled, says so and
    /// checks out the branch once created.
    pub fn open_taken(&mut self, start: Start, name: String) -> bool {
        let message = tr!("branch-track-name-taken", name = &name);
        if !self.open_name(NameKind::Create(start), name) {
            return false;
        }
        if let Some(Dialog::Name(dialog)) = &mut self.dialog {
            dialog.checkout_after = true;
            dialog.error = Some(message);
        }
        true
    }

    /// Opens the "Set upstream" window for the local branch `branch`.
    pub fn open_upstream(
        &mut self,
        branch: String,
        current: Option<String>,
        choices: Vec<String>,
        selected: Option<String>,
    ) -> bool {
        if self.busy() {
            return false;
        }
        self.dialog = Some(Dialog::Upstream(UpstreamDialog {
            branch,
            current,
            choices,
            filter: String::new(),
            selected,
            error: None,
            running: None,
        }));
        true
    }

    fn upstream_dialog(&mut self) -> Option<&mut UpstreamDialog> {
        match &mut self.dialog {
            Some(Dialog::Upstream(d)) if d.running.is_none() => Some(d),
            _ => None,
        }
    }

    /// The filter of the window changed.
    pub fn set_upstream_filter(&mut self, filter: String) {
        if let Some(d) = self.upstream_dialog() {
            d.filter = filter;
        }
    }

    /// A remote branch was picked in the window.
    pub fn pick_upstream(&mut self, full: String) {
        if let Some(d) = self.upstream_dialog()
            && d.choices.contains(&full)
        {
            d.selected = Some(full);
        }
    }

    /// Submit of the window: parameters and description. `None` if it cannot be done.
    pub fn submit_upstream(&mut self) -> Option<(Value, String)> {
        let d = self.upstream_dialog()?;
        if !d.can_submit() {
            return None;
        }
        let upstream = d.selected.clone()?;
        d.error = None;
        let label = tr!(
            "branch-upstream-label",
            name = &d.branch,
            upstream = short(&upstream)
        );
        Some((
            params(BranchSetUpstreamParams {
                name: d.branch.clone(),
                upstream: Some(upstream),
            }),
            label,
        ))
    }

    /// The command of [`BranchBox::submit_upstream`] started with `ticket`.
    pub fn upstream_started(&mut self, ticket: Ticket) {
        if let Some(Dialog::Upstream(d)) = &mut self.dialog {
            d.running = Some(Running::new(ticket));
        }
    }

    /// Starts removing the upstream of `name` (short); no dialog.
    pub fn start_unset(&mut self, name: String) -> Option<Value> {
        if self.busy() {
            return None;
        }
        let params = params(BranchSetUpstreamParams {
            name: name.clone(),
            upstream: None,
        });
        self.unset = Some(name);
        Some(params)
    }

    /// The answer of `branch.create` started by [`BranchBox::start_track`].
    pub(crate) fn receive_track(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
    ) -> Received {
        let Some(name) = self.track.take() else {
            return unexpected(Op::Create);
        };
        match result {
            Err(e) => {
                let refused =
                    matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams));
                Received::Failed {
                    message: tr!("branch-track-failed", name = &name, error = &e),
                    reload: !refused,
                }
            }
            Ok(value) => done(
                crate::decode(Op::Create.command(), value).map(|r| branch::create_outcome(r, true)),
            ),
        }
    }

    /// The answer of `branch.set_upstream`: from the window (an error stays in it) or from
    /// "Unset upstream".
    pub(crate) fn receive_upstream(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
    ) -> Received {
        let from_window = matches!(&self.dialog, Some(Dialog::Upstream(d)) if d.running.is_some());
        if from_window {
            if let (Err(e), Some(Dialog::Upstream(d))) = (&result, &mut self.dialog) {
                d.running = None;
                d.error = Some(e.clone());
                return Received::Inline;
            }
            self.dialog = None;
        } else if let Some(name) = self.unset.take() {
            if let Err(e) = &result {
                return Received::Failed {
                    message: tr!("branch-upstream-unset-failed", name = &name, error = e),
                    reload: !matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams)),
                };
            }
        } else {
            return unexpected(Op::SetUpstream);
        }
        match result {
            Ok(value) => done(
                crate::decode::<BranchSetUpstreamResult>(Op::SetUpstream.command(), value)
                    .map(upstream_outcome),
            ),
            Err(e) => Received::Failed {
                message: e,
                reload: true,
            },
        }
    }
}

/// The outcome of `branch.set_upstream`.
fn upstream_outcome(r: BranchSetUpstreamResult) -> Outcome {
    let branch = branch_short(&r.branch);
    let notice = match (&r.upstream, &r.previous) {
        _ if !r.changed => tr!("branch-upstream-unchanged", branch = branch),
        (Some(upstream), _) => tr!("branch-upstream-set", branch = branch, upstream = upstream),
        (None, Some(previous)) => {
            tr!(
                "branch-upstream-unset",
                branch = branch,
                previous = previous
            )
        }
        (None, None) => tr!("branch-upstream-unchanged", branch = branch),
    };
    Outcome {
        op: Op::SetUpstream,
        notice: notice + &messages_text(&r.warnings),
        refs: r.refs,
        status: None,
        warning: r.refs_error.map(refs_warning),
        history: r.journal.into(),
        then_checkout: None,
    }
}

impl Tab {
    /// The tracking and upstream actions, dispatched by [`Tab::apply_branch`].
    pub(crate) fn apply_tracking(&mut self, action: Action) {
        let generation = self.generation;
        let writing = self.executor.writing().is_some();
        let Some(repo) = &mut self.repo else {
            return;
        };
        match action {
            Action::CheckoutRemote(full) => {
                if writing || repo.modal_open() {
                    return;
                }
                let remotes = repo.remotes.names();
                match plan(&full, &repo.refs, &remotes) {
                    None => {}
                    Some(Plan::Checkout(local)) => self.apply_branch(Action::Checkout(local)),
                    Some(Plan::Taken(name)) => {
                        let Some(info) = repo.refs.iter().find(|r| r.name == full) else {
                            return;
                        };
                        let (start, _) = Start::from_ref(info);
                        repo.branch.open_taken(start, name);
                    }
                    Some(Plan::Create(name)) => {
                        let label =
                            tr!("branch-track-label", name = &name, upstream = short(&full));
                        if let Some(params) = repo.branch.start_track(name, &full) {
                            repo.write_error = None;
                            repo.write_label = Some(label);
                            let op = Op::Create;
                            self.submit(op.command(), params, Request::Branch(generation, op));
                        }
                    }
                }
            }
            Action::SetUpstream(name) => {
                if writing || repo.modal_open() {
                    return;
                }
                let remotes = repo.remotes.names();
                let local = format!("refs/heads/{name}");
                let current = repo
                    .refs
                    .iter()
                    .find(|r| r.name == local)
                    .and_then(|r| r.upstream.as_ref())
                    .map(|u| u.name.clone());
                let choices = upstream_choices(&repo.refs, &remotes);
                let selected = preselect(&name, current.as_deref(), &choices, &remotes);
                repo.branch.open_upstream(name, current, choices, selected);
            }
            Action::UpstreamFilter(filter) => repo.branch.set_upstream_filter(filter),
            Action::UpstreamPick(full) => repo.branch.pick_upstream(full),
            Action::SubmitUpstream => {
                if writing {
                    return;
                }
                if let Some((params, label)) = repo.branch.submit_upstream() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::SetUpstream;
                    let ticket = self.submit(op.command(), params, Request::Branch(generation, op));
                    if let Some(repo) = &mut self.repo {
                        repo.branch.upstream_started(ticket);
                    }
                }
            }
            Action::UnsetUpstream(name) => {
                if writing || repo.modal_open() {
                    return;
                }
                let label = tr!("branch-upstream-unset-label", name = &name);
                if let Some(params) = repo.branch.start_unset(name) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = Op::SetUpstream;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "tracking_tests.rs"]
mod tests;
