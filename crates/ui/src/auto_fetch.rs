//! Automatic fetch (S50): every open tab runs `remote.fetch` on its own timer, the interval of
//! the settings (off, 1, 5, 15 minutes). Only fetch, never pull: local branches and files are not
//! touched. The parameters are those of the bar's "Fetch" (`prune` included: the button always
//! asks for it, see [`FetchBox::request`](crate::fetch::FetchBox::request)).
//!
//! [`AutoFetch`] is the timer, data only and with the time passed in (`now`), so the tests use a
//! fake clock. [`Tab::tick_fetch`] runs once per frame for every tab: a fetch starts only for the
//! tab shown, with nothing in the way (a dialog, a command running, an operation in progress); a
//! skipped fetch is tried again at the next tick, a hidden tab fetches when shown again. A manual
//! fetch restarts the timer (it counts from the end of any fetch).
//!
//! Errors never open a window: authentication suspends the timer of the tab and leaves a notice
//! under the bar with "Retry"; an unreachable remote doubles the wait (up to [`MAX_WAIT`], back to
//! the interval after a fetch that works) and shows only an icon in the bar.

use crate::fetch::{self, RefUpdate};
use crate::{Done, ErrorKind, RepoView, Tab};
use project_domain::remote_short;
use project_domain::{Commit, Oid};
use project_i18n::tr;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// The longest wait after network errors.
pub const MAX_WAIT: Duration = Duration::from_secs(30 * 60);

/// A fetch due but blocked (an operation in progress, a dialog): the next look, if nothing
/// else repaints the window before.
pub const RETRY_BLOCKED: Duration = Duration::from_secs(10);

/// How many remote branches the notice names before "and N more".
const NAMES_SHOWN: usize = 3;

/// The automatic fetch was suspended by an authentication error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suspended {
    /// git's text, if any.
    pub detail: Option<String>,
}

/// The timer of one repo view.
#[derive(Debug, Default)]
pub struct AutoFetch {
    /// Where the wait counts from: the end of the last fetch (automatic or manual), the opening
    /// of the repo or the moment the automatic fetch was turned on.
    since: Option<Instant>,
    /// The interval of the settings at the last tick; `None`: off.
    interval: Option<Duration>,
    /// Network errors in a row.
    failures: u32,
    /// git's text of the last network error, while the remote is unreachable.
    offline: Option<String>,
    suspended: Option<Suspended>,
    /// "Retry": fetch at the next tick that can.
    now: bool,
    /// A fetch started since the last tick (it may have already ended).
    restart: bool,
    /// The remote branches moved by the last automatic fetch: the notice is written once the log
    /// has been read again (it counts the new commits in it).
    news: Option<Vec<RefUpdate>>,
}

impl AutoFetch {
    /// Once per frame: `interval` from the settings, `fetching` a fetch of the tab (any, also
    /// the manual one) is in progress: the wait starts again from its end.
    pub fn observe(&mut self, now: Instant, interval: Option<Duration>, fetching: bool) {
        if interval.is_none() {
            self.since = None;
        } else if fetching || std::mem::take(&mut self.restart) || self.since.is_none() {
            self.since = Some(now);
        }
        self.interval = interval;
    }

    /// True if off.
    pub fn off(&self) -> bool {
        self.interval.is_none()
    }

    /// The current wait: the interval, doubled for every network error in a row, at most
    /// [`MAX_WAIT`] (never less than the interval).
    pub fn wait(&self) -> Option<Duration> {
        let interval = self.interval?;
        let doubled = interval.saturating_mul(1u32 << self.failures.min(16));
        Some(doubled.min(MAX_WAIT).max(interval))
    }

    /// When the next fetch is due; `None`: off or suspended.
    pub fn due(&self) -> Option<Instant> {
        if self.suspended.is_some() {
            return None;
        }
        let since = self.since?;
        if self.now {
            return Some(since);
        }
        Some(since + self.wait()?)
    }

    pub fn is_due(&self, now: Instant) -> bool {
        self.due().is_some_and(|due| now >= due)
    }

    /// How long the window can sleep before this timer needs a frame.
    pub fn next_wake(&self, now: Instant) -> Option<Duration> {
        let due = self.due()?;
        let left = due.saturating_duration_since(now);
        Some(if left.is_zero() { RETRY_BLOCKED } else { left })
    }

    /// A fetch started (automatic or manual): the wait starts again from its end.
    pub fn started(&mut self) {
        self.now = false;
        self.restart = true;
    }

    /// A fetch (automatic or manual) worked: back to the interval.
    pub fn succeeded(&mut self) {
        self.failures = 0;
        self.offline = None;
        self.suspended = None;
    }

    /// The remote could not be reached: the wait doubles.
    pub fn unreachable(&mut self, detail: String) {
        self.failures = self.failures.saturating_add(1);
        self.offline = Some(detail);
    }

    /// The remote rejected the credentials (or would ask for them): no more automatic fetches
    /// until "Retry".
    pub fn suspend(&mut self, detail: Option<String>) {
        self.suspended = Some(Suspended { detail });
    }

    /// "Retry" in the notice: the next tick fetches.
    pub fn retry(&mut self) {
        self.suspended = None;
        self.now = true;
    }

    /// The notice under the bar, unless the automatic fetch is off.
    pub fn suspended(&self) -> Option<&Suspended> {
        self.suspended.as_ref().filter(|_| !self.off())
    }

    /// git's text of the network error, for the icon in the bar (not while off).
    pub fn offline(&self) -> Option<&str> {
        self.offline.as_deref().filter(|_| !self.off())
    }

    pub fn set_news(&mut self, moved: Vec<RefUpdate>) {
        self.news = (!moved.is_empty()).then_some(moved);
    }

    pub fn take_news(&mut self) -> Option<Vec<RefUpdate>> {
        self.news.take()
    }
}

/// How many commits `after` has that `before` has not, walking the loaded commits; `None` if
/// the walk leaves them (the count would be a guess).
pub fn new_commits(commits: &[Commit], before: Oid, after: Oid) -> Option<usize> {
    let parents: HashMap<Oid, &[Oid]> = commits
        .iter()
        .map(|c| (c.id, c.parents.as_slice()))
        .collect();
    let mut old = HashSet::new();
    let mut stack = vec![before];
    while let Some(id) = stack.pop() {
        if let Some(ps) = parents.get(&id)
            && old.insert(id)
        {
            stack.extend(ps.iter().copied());
        }
    }
    old.insert(before);
    let mut new = HashSet::new();
    let mut stack = vec![after];
    while let Some(id) = stack.pop() {
        if old.contains(&id) || new.contains(&id) {
            continue;
        }
        let ps = parents.get(&id)?;
        new.insert(id);
        stack.extend(ps.iter().copied());
    }
    Some(new.len())
}

/// The discreet notice after an automatic fetch: "3 new commits on origin/main".
pub fn news_text(moved: &[RefUpdate], commits: &[Commit]) -> String {
    let mut parts: Vec<String> = moved
        .iter()
        .take(NAMES_SHOWN)
        .map(|u| {
            let branch = remote_short(&u.name);
            match (u.before, u.after) {
                (None, _) => tr!("fetch-auto-new-branch", branch = branch),
                (Some(_), None) => tr!("fetch-auto-branch-gone", branch = branch),
                (Some(before), Some(after)) => match new_commits(commits, before, after) {
                    Some(n) if n > 0 => tr!("fetch-auto-new-commits", n = n, branch = branch),
                    _ => tr!("fetch-auto-branch-moved", branch = branch),
                },
            }
        })
        .collect();
    if moved.len() > NAMES_SHOWN {
        parts.push(tr!("fetch-and-more", n = moved.len() - NAMES_SHOWN));
    }
    tr!("fetch-auto-news", news = parts.join(", "))
}

impl RepoView {
    /// Something the automatic fetch waits for: a dialog or an error window open, an operation in
    /// progress (merge, rebase, cherry-pick, revert), the status not read yet.
    pub(crate) fn auto_fetch_blocked(&self) -> bool {
        self.modal_open()
            || self.fetch.failure().is_some()
            || self.remotes.failure().is_some()
            || self.merge.failure().is_some()
            || self.sync.window().is_some()
            || self.stash.conflicts().is_some()
            || self.conflict.editor().is_some()
            || self.status.as_ref().is_none_or(|s| s.operation.is_some())
    }
}

impl Tab {
    /// Once per frame for every tab, after its results: `shown`, the tab in view; `interval`, the
    /// settings' (`None`: off); `app_blocked`, a window of the app is open (settings, clone).
    pub(crate) fn tick_fetch(
        &mut self,
        now: Instant,
        shown: bool,
        interval: Option<Duration>,
        app_blocked: bool,
    ) {
        let busy = self.executor.is_busy();
        let Some(repo) = &mut self.repo else {
            return;
        };
        let fetching = repo.fetch.running().is_some();
        repo.auto_fetch.observe(now, interval, fetching);
        // Everything after the fetch has been read again (the log too): the new commits.
        if !busy && let Some(moved) = repo.auto_fetch.take_news() {
            self.notice = news_text(&moved, repo.list.commits());
        }
        if !shown
            || app_blocked
            || busy
            || !repo.auto_fetch.is_due(now)
            || repo.auto_fetch_blocked()
            || repo.remotes.names().is_empty()
        {
            return;
        }
        self.start_fetch(None, true);
    }

    /// When the tab shown needs a frame for its timer.
    pub(crate) fn fetch_wake(&self, now: Instant) -> Option<Duration> {
        self.repo.as_ref()?.auto_fetch.next_wake(now)
    }

    /// "Retry" in the notice of the suspended automatic fetch.
    pub(crate) fn retry_auto_fetch(&mut self) {
        if let Some(repo) = &mut self.repo {
            repo.auto_fetch.retry();
        }
    }

    /// Response of the automatic `remote.fetch`: no window and no message in the bar for the
    /// errors it handles; after a fetch the light refresh (selection and scroll stay).
    pub(crate) fn receive_auto_fetch(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let kind = done.error_kind;
        let detail = done.error_detail.clone();
        let received = repo
            .fetch
            .receive(done.ticket, done.result, kind, done.error_detail);
        match received {
            fetch::Received::Done(outcome) => {
                log::info!("{} (auto): {}", done.command, outcome.notice);
                if let Some(warning) = &outcome.warning {
                    log::warn!("{} (auto): {warning}", done.command);
                }
                repo.auto_fetch.succeeded();
                repo.auto_fetch.set_news(outcome.moved);
                if outcome.changed {
                    self.refresh_light();
                }
            }
            fetch::Received::Cancelled { notice } => {
                self.notice = notice;
                self.refresh_light();
            }
            fetch::Received::Failed { message, reload } => {
                log::warn!("{} (auto): {message}", done.command);
                match kind {
                    Some(ErrorKind::Auth) => repo.auto_fetch.suspend(detail),
                    Some(ErrorKind::RemoteUnreachable) => {
                        repo.auto_fetch.unreachable(detail.unwrap_or(message));
                    }
                    // Refused (no remote to choose): tried again at the next interval, quietly.
                    Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => {}
                    _ => {
                        self.error = message;
                    }
                }
                if reload {
                    self.refresh_light();
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "auto_fetch_tests.rs"]
mod tests;
