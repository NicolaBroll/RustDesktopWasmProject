//! The failure dialog (U62): an operation that failed or was refused for a reason to explain
//! (fetch, remote, pull and push, merge, checkout) shows a modal dialog with what happened,
//! what to do and git's text. Data only, no egui: tested with unit tests.
//!
//! Each area keeps its own [`Failure`]; here they become one dialog at a time. With two
//! failures waiting, the one that came first stays on screen until it is dismissed, then the
//! next one is shown: never two dialogs on top of each other. While one waits the tab has a
//! modal dialog open ([`RepoView::modal_open`]): no write starts, the keys are the dialog's and
//! the automatic refresh and fetch wait.

use crate::fetch::Failure;
use crate::{Action, RepoView, branch, fetch, merge, remotes, sync};

/// The area a failure to explain comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    Fetch,
    Remotes,
    /// Pull and push ([`sync::Window::Failure`]).
    Sync,
    /// Merge, cherry-pick and revert.
    Merge,
    /// A checkout refused for local changes.
    Checkout,
}

impl Source {
    /// The order of failures that arrive together.
    const ALL: [Source; 5] = [
        Source::Fetch,
        Source::Remotes,
        Source::Sync,
        Source::Merge,
        Source::Checkout,
    ];

    /// The action that closes the failure of this area.
    pub(crate) fn dismiss(self) -> Action {
        match self {
            Source::Fetch => Action::Fetch(fetch::Action::DismissError),
            Source::Remotes => Action::Remotes(remotes::Action::DismissError),
            Source::Sync => Action::Sync(sync::Action::Dismiss),
            Source::Merge => Action::Merge(merge::Action::DismissError),
            Source::Checkout => Action::Branch(branch::Action::DismissRefusal),
        }
    }
}

/// The areas with a failure waiting, the oldest first: what keeps the dialog on screen in its
/// place when another failure arrives.
#[derive(Debug, Default)]
pub(crate) struct Queue(Vec<Source>);

impl RepoView {
    /// The failure of `source` waiting to be dismissed, if any.
    fn failure_of(&self, source: Source) -> Option<&Failure> {
        match source {
            Source::Fetch => self.fetch.failure(),
            Source::Remotes => self.remotes.failure(),
            Source::Sync => match self.sync.window() {
                Some(sync::Window::Failure(failure)) => Some(failure),
                _ => None,
            },
            Source::Merge => self.merge.failure(),
            Source::Checkout => self.branch.refusal(),
        }
    }

    /// The failure dialog to show, with the area that closes it: the oldest one waiting. One at
    /// a time: the others follow when this one is dismissed.
    pub(crate) fn failure(&self) -> Option<(Source, &Failure)> {
        let waiting = |source: &Source| self.failure_of(*source).map(|f| (*source, f));
        self.failures
            .0
            .iter()
            .find_map(waiting)
            .or_else(|| Source::ALL.iter().find_map(waiting))
    }

    /// Notes the order of the failures waiting: the dismissed ones leave, the new ones go to
    /// the end. Called before drawing, so a failure that arrives later does not take the place
    /// of the one on screen.
    pub(crate) fn queue_failures(&mut self) {
        let mut queue = std::mem::take(&mut self.failures.0);
        queue.retain(|source| self.failure_of(*source).is_some());
        for source in Source::ALL {
            if self.failure_of(source).is_some() && !queue.contains(&source) {
                queue.push(source);
            }
        }
        self.failures.0 = queue;
    }
}

#[cfg(test)]
#[path = "failure_tests.rs"]
mod tests;
