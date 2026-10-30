//! The commit-graph offer (S36): when `repo.open` says the repo has no commit-graph and the first
//! `repo.log` page took long, a notice under the bar proposes `repo.optimize` (in the background,
//! with progress and "Stop", like the fetch). Data only, no egui: tested with unit tests.
//!
//! `repo.optimize` is a write ([`CommandSpec::writes`](project_engine::CommandSpec)): while it
//! runs the other writes and "Refresh" are disabled; it is not offered while another write runs.

use crate::running::Running;
use crate::{Done, ErrorKind, Request, Tab, Ticket};
use project_engine::RepoOptimizeResult;
use project_i18n::tr;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// "Optimize" in the commit-graph notice: `repo.optimize` in the background.
    Optimize,
    /// "Stop" the `repo.optimize` in progress.
    Cancel,
    /// "Not now" in the commit-graph notice: hidden for this repo view.
    Dismiss,
}

/// The id of the Registry command.
pub const COMMAND: &str = "repo.optimize";

/// How long the first `repo.log` page may take (from submit to answer) before the notice is
/// shown. Without a commit-graph a small repo answers in milliseconds, the Linux kernel in ~14 s.
pub const SLOW_FIRST_PAGE: Duration = Duration::from_secs(1);

/// Whether to show the notice: no commit-graph, the first page took more than
/// [`SLOW_FIRST_PAGE`] (`None`: not arrived yet) and the user did not hide it.
pub fn should_offer(commit_graph: bool, first_page: Option<Duration>, dismissed: bool) -> bool {
    !commit_graph && !dismissed && first_page.is_some_and(|took| took > SLOW_FIRST_PAGE)
}

/// The timing of the first `repo.log` page of the repo.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum FirstPage {
    #[default]
    NotAsked,
    Waiting {
        ticket: Ticket,
        since: Instant,
    },
    Took(Duration),
}

/// The offer of `repo.optimize` for the repo of the tab.
#[derive(Debug, Default)]
pub struct OptimizeBox {
    /// From `repo.open` (then from `repo.optimize`).
    commit_graph: bool,
    first_page: FirstPage,
    /// "Not now", or the graph written: the notice stays hidden for this repo view.
    dismissed: bool,
    running: Option<Running>,
    /// The last failure, shown in the notice (which stays, to retry).
    error: Option<String>,
}

/// What came back from `repo.optimize`.
#[derive(Debug, PartialEq, Eq)]
pub enum Received {
    /// Written: log and the rest are reread.
    Done,
    /// Stopped with "Stop": the previous state stays.
    Cancelled,
    /// Failed or refused: the message for the status bar (also kept in the notice).
    Failed(String),
}

impl OptimizeBox {
    /// For a repo just opened; `commit_graph` from `repo.open`.
    pub fn new(commit_graph: bool) -> Self {
        Self {
            commit_graph,
            ..Self::default()
        }
    }

    /// A `repo.log` page from the start was sent with `ticket` at `now`: only the first one of
    /// the repo is timed.
    pub fn log_submitted(&mut self, ticket: Ticket, now: Instant) {
        if self.first_page == FirstPage::NotAsked {
            self.first_page = FirstPage::Waiting { ticket, since: now };
        }
    }

    /// The answer of a `repo.log` request arrived at `now`; `ok` false: failed, the next page
    /// from the start is timed instead.
    pub fn log_received(&mut self, ticket: Ticket, now: Instant, ok: bool) {
        if let FirstPage::Waiting { ticket: t, since } = self.first_page
            && t == ticket
        {
            self.first_page = if ok {
                FirstPage::Took(now.saturating_duration_since(since))
            } else {
                FirstPage::NotAsked
            };
        }
    }

    /// How long the first page took, once it arrived.
    pub fn first_page(&self) -> Option<Duration> {
        match self.first_page {
            FirstPage::Took(took) => Some(took),
            _ => None,
        }
    }

    /// True if the notice is to be drawn (always while the command runs).
    pub fn shown(&self) -> bool {
        self.running.is_some() || should_offer(self.commit_graph, self.first_page(), self.dismissed)
    }

    /// "Not now": hidden for this repo view.
    pub fn dismiss(&mut self) {
        if self.running.is_none() {
            self.dismissed = true;
            self.error = None;
        }
    }

    /// The command in progress, if any.
    pub fn running(&self) -> Option<&Running> {
        self.running.as_ref()
    }

    /// The last failure, if any.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The parameters of `repo.optimize`, if it is offered and not already running.
    pub fn request(&self) -> Option<Value> {
        (self.running.is_none() && self.shown()).then(|| json!({}))
    }

    /// The command requested with [`OptimizeBox::request`] started with `ticket`.
    pub fn started(&mut self, ticket: Ticket) {
        self.error = None;
        self.running = Some(Running::new(ticket));
    }

    /// "Stop": the ticket to cancel, only once.
    pub fn cancel(&mut self) -> Option<Ticket> {
        self.running.as_mut()?.cancel()
    }

    /// The response of `repo.optimize` for `ticket`; `detail`: git's text, if any.
    pub fn receive(
        &mut self,
        ticket: Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        match self.running.take() {
            Some(running) if running.ticket == ticket => {}
            other => {
                self.running = other;
                return Received::Failed(tr!("common-unexpected-response", command = COMMAND));
            }
        }
        let error = match result {
            Ok(value) => match serde_json::from_value::<RepoOptimizeResult>(value) {
                Ok(result) => {
                    self.commit_graph = result.commit_graph;
                    // Hidden even if git wrote nothing (a repo without commits).
                    self.dismissed = true;
                    return Received::Done;
                }
                Err(e) => crate::unexpected_result(COMMAND, e),
            },
            Err(_) if kind == Some(ErrorKind::Cancelled) => return Received::Cancelled,
            Err(error) => match detail {
                Some(detail) if !detail.trim().is_empty() && !error.contains(detail.trim()) => {
                    format!("{error}: {}", detail.trim())
                }
                _ => error,
            },
        };
        let message = tr!("optimize-failed", error = &error);
        self.error = Some(message.clone());
        Received::Failed(message)
    }
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_optimize(&mut self, action: Action) {
        match action {
            Action::Optimize => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let Some(params) = repo.optimize.request() else {
                    return;
                };
                repo.write_error = None;
                repo.write_label = Some(tr!("optimize-label"));
                let ticket = self.executor.submit(COMMAND, params);
                self.pending.insert(ticket, Request::Optimize(generation));
                if let Some(repo) = &mut self.repo {
                    repo.optimize.started(ticket);
                }
            }
            Action::Cancel => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.optimize.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::Dismiss => {
                if let Some(repo) = &mut self.repo {
                    repo.optimize.dismiss();
                }
            }
        }
    }

    /// Response of `repo.optimize`: on success everything is reread (the log is now fast).
    pub(crate) fn receive_optimize(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received =
            repo.optimize
                .receive(done.ticket, done.result, done.error_kind, done.error_detail);
        match received {
            Received::Done => {
                self.notice = tr!("optimize-done");
                self.error.clear();
                self.load();
            }
            Received::Cancelled => {
                self.notice = tr!("optimize-cancelled");
                self.error.clear();
            }
            Received::Failed(message) => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
            }
        }
    }
}

#[cfg(test)]
#[path = "optimize_tests.rs"]
mod tests;
