//! Automatic refresh (S40): a tab re-reads its repo when the git directory changes outside the app
//! (a terminal, an agent through `project-cli mcp`) and when the window gains focus (edits in the
//! worktree, which is not watched). See `impl-docs/ARCHITECTURE-git.md` §8.
//!
//! The watch is [`Platform::watch`]: its callback only raises a flag and repaints. Every frame
//! [`AutoRefresh::observe`] turns the flag into "dirty" unless the change is the tab's own write
//! (during it and for [`QUIET_AFTER_WRITE`] after its result: the write already re-reads what it
//! changed), and [`AutoRefresh::take_due`] says when to re-read: only for the tab shown and not
//! while a dialog is open or a command is running; otherwise it waits.

use crate::{Request, Tab};
use project_domain::{Oid, RefInfo};
use project_engine::{Platform, Watch};
use serde_json::json;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// After a write of the app, the events of the git directory are its own: the platform delivers
/// them debounced (300 ms on desktop) after the last change, which comes before the result.
pub const QUIET_AFTER_WRITE: Duration = Duration::from_millis(600);

/// The automatic refresh state of one tab.
#[derive(Default)]
pub struct AutoRefresh {
    /// Raised by the watch callback, from any thread.
    signal: Arc<AtomicBool>,
    /// The watch of the open repo; dropped with the tab or when another repo opens.
    watch: Option<Watch>,
    /// Something changed and the tab has not re-read it yet.
    dirty: bool,
    /// The tab was writing in the last frame.
    was_writing: bool,
    /// Events up to this time belong to the write that just ended.
    quiet_until: Option<Instant>,
}

impl AutoRefresh {
    /// Watches `repo` (the root given by `repo.open`); `wake` repaints the window.
    pub fn start(&mut self, platform: &dyn Platform, repo: &str, wake: crate::Wake) {
        self.watch = None;
        self.signal = Arc::default();
        self.dirty = false;
        let signal = Arc::clone(&self.signal);
        self.watch = platform.watch(
            repo,
            Arc::new(move || {
                signal.store(true, Ordering::Release);
                wake();
            }),
        );
    }

    /// The flag the watch raises (tests raise it by hand).
    #[cfg(test)]
    pub fn signal(&self) -> &AtomicBool {
        &self.signal
    }

    /// Something changed that the watch does not see (the window gained focus): re-read.
    pub fn mark(&mut self) {
        self.dirty = true;
    }

    /// Re-read in full (manual Refresh): nothing left to do.
    pub fn clear(&mut self) {
        self.dirty = false;
    }

    #[cfg(test)]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Once per frame, after the results were collected: `writing`, the tab has a write in
    /// progress. A change seen while writing or within [`QUIET_AFTER_WRITE`] of its end is
    /// the write's own and is dropped.
    pub fn observe(&mut self, now: Instant, writing: bool) {
        if self.was_writing && !writing {
            self.quiet_until = Some(now + QUIET_AFTER_WRITE);
        }
        self.was_writing = writing;
        if !self.signal.swap(false, Ordering::AcqRel) {
            return;
        }
        let own = writing || self.quiet_until.is_some_and(|until| now < until);
        if !own {
            self.dirty = true;
        }
    }

    /// True (and no longer dirty) if the tab must re-read now: it is dirty, `shown` and not
    /// `blocked` (a dialog open, a command running). A hidden or blocked tab stays dirty.
    pub fn take_due(&mut self, shown: bool, blocked: bool) -> bool {
        if self.dirty && shown && !blocked {
            self.dirty = false;
            return true;
        }
        false
    }
}

/// The window gained focus: `focused` went from false to true. The first value only records the
/// state (the window gets focus at startup, when the tabs are loading anyway).
pub fn focus_gained(last: &mut Option<bool>, focused: Option<bool>) -> bool {
    let Some(focused) = focused else {
        return false;
    };
    let gained = *last == Some(false) && focused;
    *last = Some(focused);
    gained
}

/// The log must be read again if a ref moved, appeared or disappeared (commit, checkout `-b`,
/// fetch, stash) or HEAD points elsewhere (detached checkout); `head_before` `None`: unknown.
pub fn log_stale(
    refs_before: &[RefInfo],
    refs_after: &[RefInfo],
    head_before: Option<Option<Oid>>,
    head_after: Option<Oid>,
) -> bool {
    refs_before != refs_after || head_before != Some(head_after)
}

impl Tab {
    /// Once per frame for every tab, after its results were collected: `shown`, the tab is the
    /// one in view. Re-reads if a change is waiting and nothing is in the way.
    pub(crate) fn tick(&mut self, now: Instant, shown: bool) {
        self.auto.observe(now, self.executor.writing().is_some());
        let blocked = self.executor.is_busy() || self.repo.as_ref().is_none_or(|r| r.modal_open());
        if self.auto.take_due(shown, blocked) {
            self.refresh_light();
        }
    }

    /// The automatic refresh: status (and with it the operation in progress), undo history,
    /// remotes and stashes; the refs after the status and the log only if they changed
    /// (see [`Tab::receive_auto_refs`]). Selection, scroll and open views stay.
    pub(crate) fn refresh_light(&mut self) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        repo.refresh_head = Some(repo.head.target());
        self.submit("repo.status", json!({}), Request::AutoStatus(generation));
        self.submit("edit.history", json!({}), Request::History(generation));
        self.request_remotes(generation);
        self.submit(crate::stash::LIST, json!({}), Request::Stashes(generation));
    }

    /// The status of the automatic refresh arrived: now the refs, compared with the old ones.
    pub(crate) fn after_auto_status(&mut self, generation: u64) {
        self.submit("repo.refs", json!({}), Request::AutoRefs(generation));
    }

    /// The refs of the automatic refresh: if they or HEAD changed the log is read again from the
    /// start, as many commits as were loaded, so the selected commit and the scroll stay.
    pub(crate) fn receive_auto_refs(&mut self, generation: u64, refs: Vec<RefInfo>) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        let stale = log_stale(
            &repo.refs,
            &refs,
            repo.refresh_head.take(),
            repo.head.target(),
        );
        repo.refs = refs;
        repo.update_labels();
        if stale {
            let page = repo.list.reload();
            self.request_page(page);
        }
    }
}

#[cfg(test)]
#[path = "refresh_tests.rs"]
mod tests;
