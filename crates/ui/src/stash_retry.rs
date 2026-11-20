//! "Stash and retry" (U68): a merge, cherry-pick, revert, pull or checkout refused for local
//! changes is sent again right after the stash that sets those changes aside. Data only, no
//! egui: tested with unit tests.
//!
//! - **Recorded**: the area of the refused command puts a [`Retry`] in its failure
//!   ([`crate::fetch::Failure::retry`]): the command id and the parameters as they were sent
//!   the first time (nothing added: no `confirm`, no `force`). A push is never recorded. After
//!   every answer the tab notes HEAD as it shows it then ([`RepoView::note_refused_head`]).
//! - **Offered**: "Stash…" in that failure dialog moves the retry into the stash dialog
//!   ([`RepoView::take_refused`]), whose default button becomes "Stash and …". It lives only
//!   there: closing the dialog drops it, "Stash…" from the toolbar or the stage panel opens the
//!   dialog without one, nothing is saved.
//! - **Sent**: the answer of `stash.push` hands the retry back only when the stash succeeded and
//!   "Stash and …" was the button pressed ([`crate::stash::StashBox::receive`]). The handler
//!   of that answer sends it at once ([`Tab::retry_after_stash`]), so no other write can start
//!   in between, exactly once; or drops it with a message when [`Retry::check`] says the tab is
//!   no longer where the refusal came from. The stash is never applied again by itself.
//! - **Answered**: the command answers in its own area as always (a second refusal opens the
//!   usual failure dialog); [`Tab::retry_answered`] adds to the status bar where the changes
//!   are.

use crate::failure::Source;
use crate::{RepoView, Request, Tab, Ticket, branch, merge, sync};
use project_domain::{Head, Status};
use project_i18n::tr;
use serde_json::Value;

/// Where the answer of the retried command goes: what its area keeps for a command in
/// progress, as it was when the command was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `remote.pull`.
    Pull(sync::Request),
    /// `merge.start`, `commit.cherry_pick`, `commit.revert`.
    Merge(merge::Op, merge::Starting),
    /// `branch.checkout`, `commit.checkout`.
    Checkout(branch::Op, branch::Checkout),
}

/// A command refused for local changes, to send again after a stash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retry {
    /// The parameters as they were sent.
    params: Value,
    /// The description of the command (`Pull of “main”`), for the dialog and the status bar.
    label: String,
    /// The default button of the stash dialog (`Stash and pull`).
    button: String,
    route: Route,
    /// HEAD as the tab showed it when the command was refused; `None` until noted.
    head: Option<Head>,
}

/// Why a retry was not sent after its stash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dropped {
    /// The current branch, or its commit, is not the one the command was refused on.
    Head,
    /// A merge, rebase, cherry-pick... is in progress.
    Operation,
    /// The status did not come back with the stash, or HEAD at the refusal is not known.
    Unknown,
    /// Another write is running or a dialog is open.
    Busy,
}

impl Retry {
    /// The command refused with `params`, described by `label`; `button`: "Stash and …".
    pub fn new(params: Value, label: String, button: String, route: Route) -> Self {
        Self {
            params,
            label,
            button,
            route,
            head: None,
        }
    }

    /// HEAD as the tab shows it when the refusal arrives: the first one noted stays.
    pub fn seen_at(&mut self, head: &Head) {
        if self.head.is_none() {
            self.head = Some(head.clone());
        }
    }

    /// The id of the Registry command: the one of the operation that was refused.
    pub fn command(&self) -> &'static str {
        match &self.route {
            Route::Pull(request) => request.op.command(),
            Route::Merge(op, _) => op.command(),
            Route::Checkout(op, _) => op.command(),
        }
    }

    /// The parameters to send: the ones that were refused.
    pub fn params(&self) -> &Value {
        &self.params
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn button(&self) -> &str {
        &self.button
    }

    /// Whether the command can be sent again after the stash. `status`: the one that came back
    /// with the answer of `stash.push` (`None`: not reread). Only on the same branch and commit
    /// as when it was refused, and with no operation in progress.
    pub fn check(&self, status: Option<&Status>) -> Result<(), Dropped> {
        let (Some(head), Some(status)) = (&self.head, status) else {
            return Err(Dropped::Unknown);
        };
        if status.operation.is_some() {
            Err(Dropped::Operation)
        } else if status.head != *head {
            Err(Dropped::Head)
        } else {
            Ok(())
        }
    }
}

impl Dropped {
    /// The message for the status bar: the stash `stash` was made, `label` was not sent again.
    pub fn message(self, label: &str, stash: &str) -> String {
        let reason = match self {
            Dropped::Head => tr!("stash-retry-dropped-head"),
            Dropped::Operation => tr!("stash-retry-dropped-operation"),
            Dropped::Unknown => tr!("stash-retry-dropped-unknown"),
            Dropped::Busy => tr!("stash-retry-dropped-busy"),
        };
        tr!(
            "stash-retry-dropped",
            name = stash,
            label = label,
            reason = reason
        )
    }
}

/// The retry sent and not yet answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub ticket: Ticket,
    /// The stash made before it (`stash@{0}`).
    pub stash: String,
    /// The description of the command.
    pub label: String,
}

impl RepoView {
    /// After every answer: a command just refused for local changes gets HEAD as the tab shows
    /// it now, which [`Retry::check`] compares with the status after the stash.
    pub(crate) fn note_refused_head(&mut self) {
        let failures = [
            self.sync.failure_mut(),
            self.merge.failure_mut(),
            self.branch.refusal_mut(),
        ];
        for failure in failures.into_iter().flatten() {
            if let Some(retry) = &mut failure.retry {
                retry.seen_at(&self.head);
            }
        }
    }

    /// "Stash…" in the failure dialog of `source`: the command it refused for local changes,
    /// taken out of that failure (the dialog is then closed by its own action). `None` for a
    /// failure that has nothing to retry.
    pub(crate) fn take_refused(&mut self, source: Source) -> Option<Retry> {
        let failure = match source {
            Source::Sync => self.sync.failure_mut(),
            Source::Merge => self.merge.failure_mut(),
            Source::Checkout => self.branch.refusal_mut(),
            Source::Fetch | Source::Remotes | Source::Identity => None,
        }?;
        failure.retry.take().map(|retry| *retry)
    }
}

impl Tab {
    /// The stash of "Stash and …" succeeded (`stash`: its name): sends `retry` again, with the
    /// command id and the parameters it was refused with, or drops it and says why in the
    /// status bar. Called by
    /// the handler of the `stash.push` answer, before any other action of the user is applied.
    /// `status_read`: the status came back with that answer.
    pub(crate) fn retry_after_stash(
        &mut self,
        generation: u64,
        retry: Retry,
        stash: String,
        status_read: bool,
    ) {
        if generation != self.generation {
            return;
        }
        let writing = self.executor.writing().is_some();
        let Some(repo) = &mut self.repo else {
            return;
        };
        let busy = writing
            || repo.modal_open()
            || repo.sync.running().is_some()
            || repo.merge.busy()
            || repo.branch.busy();
        let checked = if busy {
            Err(Dropped::Busy)
        } else {
            retry.check(repo.status.as_ref().filter(|_| status_read))
        };
        if let Err(dropped) = checked {
            // In place of the notice of the stash: a notice too, which the reads that follow
            // the stash do not clear (they clear the error).
            let message = dropped.message(retry.label(), &stash);
            log::warn!("{}: {message}", retry.command());
            self.notice.set(message);
            return;
        }
        let command = retry.command();
        log::info!("{command}: sent again after {stash}");
        let params = retry.params().clone();
        let Retry { label, route, .. } = retry;
        repo.write_error = None;
        repo.write_label = Some(label.clone());
        let ticket = self.executor.submit(command, params);
        let request = match route {
            Route::Pull(request) => {
                repo.sync.started(ticket, request);
                Request::Sync(generation)
            }
            Route::Merge(op, starting) => {
                repo.merge.restart(starting);
                Request::Merge(generation, op)
            }
            Route::Checkout(op, checkout) => {
                repo.branch.restart_checkout(checkout);
                Request::Branch(generation, op)
            }
        };
        self.pending.insert(ticket, request);
        repo.stash.retry_sent(Sent {
            ticket,
            stash,
            label,
        });
    }

    /// The retried command answered and its area handled the answer (`ok`: it succeeded): the
    /// status bar also says where the changes are. Done: the stash first, then what the
    /// command says. Refused or failed: the command did not complete (refused, nothing changed;
    /// failed midway, files or a ref may have) and the stash stays in the
    /// list (the area's failure dialog, if any, explains why).
    pub(crate) fn retry_answered(&mut self, sent: &Sent, ok: bool) {
        if ok {
            // Also when the command itself says nothing: where the changes went is said anyway.
            let notice = tr!(
                "stash-retry-done",
                name = &sent.stash,
                notice = &*self.notice
            );
            self.notice.set(notice.trim_end().to_owned());
        } else {
            // Beside the error of the area, and as the notice that stays when a read that
            // follows clears that error.
            let kept = tr!("stash-retry-kept", label = &sent.label, name = &sent.stash);
            if !self.error.is_empty() {
                self.error = format!("{kept} {}", self.error);
            }
            self.notice.set(kept);
        }
    }
}

#[cfg(test)]
#[path = "stash_retry_tests.rs"]
mod tests;
