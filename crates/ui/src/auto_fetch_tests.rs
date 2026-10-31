//! Automatic fetch with a fake clock (instants built by hand, no sleeps): interval, skips,
//! backoff, suspension and "Retry", hidden tab, manual fetch, one fetch at a time; on the
//! `MemoryBackend` the fetch runs for real and the notice counts the new commits.

use super::*;
use crate::Action;
use crate::executor::Executor;
use crate::fetch;
use project_engine::{
    Capabilities, FetchFailure, MemoryBackend, MemoryOpener, Platform, Registry, Session,
};
use std::sync::{Arc, Mutex};

const MINUTE: Duration = Duration::from_secs(60);

/// Jobs run inside `spawn` unless `hold` is set: then they wait in `held` until `release`.
#[derive(Default)]
struct Jobs {
    hold: Mutex<bool>,
    held: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
}

impl Jobs {
    fn release(&self) {
        *self.hold.lock().unwrap() = false;
        let jobs: Vec<_> = self.held.lock().unwrap().drain(..).collect();
        for job in jobs {
            job();
        }
    }
}

impl Platform for Jobs {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        if *self.hold.lock().unwrap() {
            self.held.lock().unwrap().push(job);
        } else {
            job();
        }
    }
}

/// A tab with `repo` open and loaded.
fn tab_with(repo: MemoryBackend, platform: &Arc<Jobs>) -> Tab {
    let mut opener = MemoryOpener::new();
    opener.insert("r", repo);
    let executor = Executor::new(
        Session::new(Arc::new(opener)),
        Registry::builtin(),
        Arc::clone(platform) as Arc<dyn Platform>,
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(0, executor);
    tab.open("r".to_owned());
    settle(&mut tab);
    tab
}

fn tab() -> Tab {
    tab_with(MemoryBackend::sample(), &Arc::default())
}

/// Collects results until nothing is running; returns the commands seen.
fn settle(tab: &mut Tab) -> Vec<String> {
    let mut seen = Vec::new();
    while tab.executor.is_busy() {
        seen.extend(tab.executor.running().iter().map(|j| j.command.clone()));
        tab.receive();
    }
    seen
}

fn fetching(tab: &Tab) -> usize {
    tab.executor
        .running()
        .iter()
        .filter(|j| j.command == fetch::COMMAND)
        .count()
}

/// One frame of the tab, shown, with a 5-minute interval.
fn frame(tab: &mut Tab, now: Instant) {
    tab.tick_fetch(now, true, Some(5 * MINUTE), false);
}

/// Whether the fetch in progress may open the credential dialog.
fn asks(tab: &Tab) -> bool {
    tab.executor
        .running()
        .iter()
        .find(|j| j.command == fetch::COMMAND)
        .expect("a fetch")
        .asks
}

fn auto(tab: &Tab) -> &AutoFetch {
    &tab.repo.as_ref().unwrap().auto_fetch
}

#[test]
fn the_wait_doubles_on_network_errors_up_to_the_cap_and_resets() {
    let t0 = Instant::now();
    let mut a = AutoFetch::default();
    a.observe(t0, Some(5 * MINUTE), false);
    assert_eq!(a.wait(), Some(5 * MINUTE));
    assert_eq!(a.due(), Some(t0 + 5 * MINUTE));
    assert!(!a.is_due(t0 + 4 * MINUTE));
    assert!(a.is_due(t0 + 5 * MINUTE));
    for expected in [10, 20, 30, 30, 30] {
        a.unreachable("fatal: unable to access".into());
        assert_eq!(a.wait(), Some(expected * MINUTE));
    }
    assert_eq!(a.offline(), Some("fatal: unable to access"));
    a.succeeded();
    assert_eq!(a.wait(), Some(5 * MINUTE), "back to the interval");
    assert_eq!(a.offline(), None);
    // Many errors in a row do not overflow.
    for _ in 0..100 {
        a.unreachable(String::new());
    }
    assert_eq!(a.wait(), Some(MAX_WAIT));
    // A 1-minute interval doubles too.
    a.succeeded();
    a.observe(t0, Some(MINUTE), false);
    a.unreachable(String::new());
    assert_eq!(a.wait(), Some(2 * MINUTE));
}

#[test]
fn off_means_no_timer_and_turning_it_on_starts_counting_then() {
    let t0 = Instant::now();
    let mut a = AutoFetch::default();
    a.observe(t0, None, false);
    assert_eq!(a.due(), None);
    assert_eq!(a.next_wake(t0), None);
    a.observe(t0 + 60 * MINUTE, Some(MINUTE), false);
    assert_eq!(a.due(), Some(t0 + 61 * MINUTE));
    assert_eq!(a.next_wake(t0 + 60 * MINUTE), Some(MINUTE));
    // Overdue but blocked: a look again a little later, not a frame at once.
    assert_eq!(a.next_wake(t0 + 70 * MINUTE), Some(RETRY_BLOCKED));
}

#[test]
fn the_interval_starts_a_fetch_with_the_parameters_of_the_button() {
    let mut tab = tab();
    let t0 = Instant::now();
    frame(&mut tab, t0);
    frame(&mut tab, t0 + 4 * MINUTE);
    assert_eq!(fetching(&tab), 0, "not yet");
    frame(&mut tab, t0 + 5 * MINUTE);
    assert_eq!(fetching(&tab), 1);
    let running = tab.repo.as_ref().unwrap().fetch.running().unwrap().clone();
    assert!(running.extra.auto);
    assert_eq!(running.extra.remote, None, "the remote the engine chooses");
    // Same parameters as "Fetch": `prune` as the button.
    assert_eq!(
        tab.repo.as_ref().unwrap().fetch.request(None),
        None,
        "one in progress"
    );
    assert_eq!(
        fetch::FetchBox::default().request(None),
        Some(serde_json::json!({ "prune": true }))
    );
    let seen = settle(&mut tab);
    // The light refresh: status, refs and the log read again, nothing that writes.
    for command in ["repo.status", "repo.refs", "repo.log"] {
        assert!(seen.iter().any(|c| c == command), "{command}: {seen:?}");
    }
    assert!(
        !seen
            .iter()
            .any(|c| c.starts_with("remote.pull") || c.starts_with("branch.")),
        "{seen:?}"
    );
    // The next one counts from the end of this fetch.
    let t1 = t0 + 5 * MINUTE + Duration::from_secs(1);
    frame(&mut tab, t1);
    assert_eq!(auto(&tab).due(), Some(t1 + 5 * MINUTE));
}

#[test]
fn after_a_fetch_the_selection_stays_and_a_notice_counts_the_new_commits() {
    let mut tab = tab();
    tab.repo.as_mut().unwrap().list.select(1);
    let selected = tab
        .repo
        .as_ref()
        .unwrap()
        .list
        .selected_commit()
        .unwrap()
        .id;
    tab.notice.clear();
    let t0 = Instant::now();
    frame(&mut tab, t0);
    frame(&mut tab, t0 + 5 * MINUTE);
    settle(&mut tab);
    frame(&mut tab, t0 + 5 * MINUTE);
    assert_eq!(
        tab.notice,
        "Automatic fetch: 1 new commit on origin/feature/graph"
    );
    assert!(tab.error.is_empty(), "{}", tab.error);
    let repo = tab.repo.as_ref().unwrap();
    assert_eq!(repo.list.selected_commit().unwrap().id, selected);
    assert!(repo.fetch.failure().is_none());
    // Nothing new the second time: no notice.
    tab.notice.clear();
    frame(&mut tab, t0 + 11 * MINUTE);
    settle(&mut tab);
    frame(&mut tab, t0 + 11 * MINUTE);
    assert_eq!(tab.notice, "");
}

#[test]
fn a_dialog_a_running_command_or_an_operation_skip_the_fetch_until_the_next_tick() {
    let mut tab = tab();
    let t0 = Instant::now();
    frame(&mut tab, t0);
    let due = t0 + 5 * MINUTE;

    // A dialog open.
    assert!(
        tab.repo
            .as_mut()
            .unwrap()
            .branch
            .open_rename("main".to_owned())
    );
    frame(&mut tab, due);
    assert_eq!(fetching(&tab), 0, "dialog open");
    tab.repo.as_mut().unwrap().branch = crate::BranchBox::default();
    // A window of the app (settings, clone).
    tab.tick_fetch(due, true, Some(5 * MINUTE), true);
    assert_eq!(fetching(&tab), 0, "settings window open");

    // An operation in progress.
    tab.repo
        .as_mut()
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .operation = Some(project_domain::Operation::Merge);
    frame(&mut tab, due + MINUTE);
    assert_eq!(fetching(&tab), 0, "merge in progress");
    tab.repo
        .as_mut()
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .operation = None;

    // A command running.
    tab.submit(
        "repo.status",
        serde_json::json!({}),
        crate::Request::Status(tab.generation),
    );
    frame(&mut tab, due + 2 * MINUTE);
    assert_eq!(fetching(&tab), 0, "command running");
    settle(&mut tab);

    // The next tick with nothing in the way fetches.
    frame(&mut tab, due + 2 * MINUTE);
    assert_eq!(fetching(&tab), 1);
}

#[test]
fn a_hidden_tab_fetches_when_shown_again_if_its_interval_passed() {
    let mut tab = tab();
    let t0 = Instant::now();
    tab.tick_fetch(t0, false, Some(5 * MINUTE), false);
    tab.tick_fetch(t0 + 20 * MINUTE, false, Some(5 * MINUTE), false);
    assert_eq!(fetching(&tab), 0, "hidden");
    tab.tick_fetch(t0 + 21 * MINUTE, true, Some(5 * MINUTE), false);
    assert_eq!(fetching(&tab), 1, "shown again");
    settle(&mut tab);
    // Shown before its interval: nothing.
    let mut other = self::tab();
    other.tick_fetch(t0, false, Some(5 * MINUTE), false);
    other.tick_fetch(t0 + 2 * MINUTE, true, Some(5 * MINUTE), false);
    assert_eq!(fetching(&other), 0);
}

#[test]
fn a_manual_fetch_restarts_the_timer() {
    let mut tab = tab();
    let t0 = Instant::now();
    frame(&mut tab, t0);
    tab.apply(Action::Fetch(fetch::Action::Fetch(None)));
    assert!(
        !tab.repo
            .as_ref()
            .unwrap()
            .fetch
            .running()
            .unwrap()
            .extra
            .auto
    );
    let t1 = t0 + 4 * MINUTE;
    frame(&mut tab, t1);
    settle(&mut tab);
    frame(&mut tab, t1);
    assert_eq!(auto(&tab).due(), Some(t1 + 5 * MINUTE));
    frame(&mut tab, t0 + 5 * MINUTE);
    assert_eq!(fetching(&tab), 0, "the old deadline no longer counts");
    frame(&mut tab, t1 + 5 * MINUTE);
    assert_eq!(fetching(&tab), 1);
}

#[test]
fn never_two_fetches_of_the_same_tab() {
    let platform = Arc::new(Jobs::default());
    let mut tab = tab_with(MemoryBackend::sample(), &platform);
    let t0 = Instant::now();
    frame(&mut tab, t0);
    *platform.hold.lock().unwrap() = true;
    frame(&mut tab, t0 + 5 * MINUTE);
    assert_eq!(fetching(&tab), 1);
    // Still running long after: no second one, and the bar's Fetch does nothing either.
    tab.receive();
    frame(&mut tab, t0 + 60 * MINUTE);
    tab.apply(Action::Fetch(fetch::Action::Fetch(None)));
    assert_eq!(fetching(&tab), 1);
    platform.release();
    settle(&mut tab);
    assert!(tab.repo.as_ref().unwrap().fetch.running().is_none());
}

#[test]
fn an_authentication_error_suspends_the_timer_until_retry() {
    let mut repo = MemoryBackend::sample();
    repo.fail_fetch(
        "origin",
        Some(FetchFailure::Auth(
            "fatal: Authentication failed for 'https://example.com'".into(),
        )),
    );
    let platform = Arc::new(Jobs::default());
    let mut tab = tab_with(repo, &platform);
    let t0 = Instant::now();
    frame(&mut tab, t0);
    *platform.hold.lock().unwrap() = true;
    frame(&mut tab, t0 + 5 * MINUTE);
    // The tick's fetch never opens the credential dialog: git gets no answer (`Auth`).
    assert_eq!(fetching(&tab), 1);
    assert!(!asks(&tab), "the automatic fetch never asks");
    platform.release();
    settle(&mut tab);
    frame(&mut tab, t0 + 5 * MINUTE);
    let repo = tab.repo.as_ref().unwrap();
    assert!(repo.fetch.failure().is_none(), "no window");
    assert!(tab.error.is_empty(), "nothing in the status bar");
    let suspended = repo.auto_fetch.suspended().expect("the notice");
    assert_eq!(
        suspended.detail.as_deref(),
        Some("fatal: Authentication failed for 'https://example.com'")
    );
    // No fetch at the following ticks.
    for minutes in [10, 60, 600] {
        frame(&mut tab, t0 + minutes * MINUTE);
        assert_eq!(fetching(&tab), 0);
    }
    assert_eq!(
        auto(&tab).next_wake(t0),
        None,
        "no frames for a suspended timer"
    );
    // Turned off the notice goes away; on again it is back.
    tab.tick_fetch(t0, true, None, false);
    assert!(auto(&tab).suspended().is_none());
    frame(&mut tab, t0 + 601 * MINUTE);
    assert!(auto(&tab).suspended().is_some());
    // "Retry": a fetch at once, which may ask for credentials (the dialog); suspended again
    // since the remote still refuses.
    tab.apply(Action::RetryAutoFetch);
    assert!(auto(&tab).suspended().is_none());
    frame(&mut tab, t0 + 601 * MINUTE);
    assert_eq!(fetching(&tab), 1);
    assert!(asks(&tab), "after Retry the fetch may ask");
    settle(&mut tab);
    frame(&mut tab, t0 + 601 * MINUTE);
    assert!(auto(&tab).suspended().is_some());
    assert!(tab.repo.as_ref().unwrap().fetch.failure().is_none());
    // Only that one: the next ticks are quiet again.
    tab.apply(Action::RetryAutoFetch);
    *platform.hold.lock().unwrap() = true;
    frame(&mut tab, t0 + 602 * MINUTE);
    assert!(asks(&tab));
    platform.release();
    settle(&mut tab);
    assert!(!auto(&tab).ask);
}

#[test]
fn an_unreachable_remote_backs_off_with_only_an_icon() {
    let mut repo = MemoryBackend::sample();
    repo.fail_fetch(
        "origin",
        Some(FetchFailure::Unreachable(
            "fatal: Could not resolve host: example.com".into(),
        )),
    );
    let mut tab = tab_with(repo, &Arc::default());
    let mut now = Instant::now();
    frame(&mut tab, now);
    for wait in [5, 10, 20, 30, 30] {
        now += wait * MINUTE - Duration::from_secs(1);
        frame(&mut tab, now);
        assert_eq!(fetching(&tab), 0, "before {wait} minutes");
        now += Duration::from_secs(1);
        frame(&mut tab, now);
        assert_eq!(fetching(&tab), 1, "after {wait} minutes");
        settle(&mut tab);
        frame(&mut tab, now);
    }
    let repo = tab.repo.as_ref().unwrap();
    assert_eq!(
        repo.auto_fetch.offline(),
        Some("fatal: Could not resolve host: example.com")
    );
    assert!(repo.fetch.failure().is_none(), "no window");
    assert!(repo.auto_fetch.suspended().is_none());
    assert!(tab.error.is_empty());
}

#[test]
fn a_manual_fetch_that_works_resets_the_backoff() {
    let mut tab = tab();
    let t0 = Instant::now();
    frame(&mut tab, t0);
    {
        let a = &mut tab.repo.as_mut().unwrap().auto_fetch;
        a.unreachable("x".into());
        a.unreachable("x".into());
        a.suspend(None);
    }
    tab.apply(Action::Fetch(fetch::Action::Fetch(None)));
    settle(&mut tab);
    let a = auto(&tab);
    assert_eq!(a.wait(), Some(5 * MINUTE));
    assert_eq!(a.offline(), None);
    assert!(a.suspended().is_none());
}

#[test]
fn new_commits_are_counted_in_the_loaded_log() {
    let id = MemoryBackend::fake_oid;
    let who = project_domain::Signature {
        name: "A".into(),
        email: "a@x".into(),
        time: 0,
        offset_min: 0,
    };
    let c = |n: u64, parents: &[u64]| Commit {
        id: id(n),
        parents: parents.iter().map(|&p| id(p)).collect(),
        author: who.clone(),
        committer: who.clone(),
        summary: String::new(),
        body: String::new(),
    };
    // 1 <- 2 <- 3 <- 5 (merge of 3 and 4), 4 <- 1
    let log = [c(5, &[3, 4]), c(4, &[1]), c(3, &[2]), c(2, &[1]), c(1, &[])];
    assert_eq!(new_commits(&log, id(2), id(5)), Some(3));
    assert_eq!(new_commits(&log, id(5), id(5)), Some(0));
    assert_eq!(new_commits(&log, id(5), id(2)), Some(0), "moved back");
    // `before` not loaded: the walk stops at it, if it meets it.
    assert_eq!(new_commits(&log[..3], id(2), id(3)), Some(1));
    // The walk leaves the loaded commits: unknown.
    assert_eq!(new_commits(&log[..2], id(2), id(5)), None);
    assert_eq!(new_commits(&log, id(2), id(9)), None);

    let u = |name: &str, before: Option<u64>, after: Option<u64>| RefUpdate {
        name: format!("refs/remotes/{name}"),
        before: before.map(id),
        after: after.map(id),
    };
    let moved = [
        u("origin/main", Some(2), Some(5)),
        u("origin/new", None, Some(4)),
        u("origin/old", Some(1), None),
        u("origin/far", Some(2), Some(9)),
    ];
    assert_eq!(
        news_text(&moved, &log),
        "Automatic fetch: 3 new commits on origin/main, new branch origin/new, \
         origin/old removed, and 1 more"
    );
    assert_eq!(
        news_text(&moved[3..], &log),
        "Automatic fetch: origin/far updated"
    );
}

#[test]
fn a_tab_without_remotes_never_fetches() {
    let mut tab = tab_with(MemoryBackend::new(), &Arc::default());
    let t0 = Instant::now();
    frame(&mut tab, t0);
    frame(&mut tab, t0 + 60 * MINUTE);
    assert_eq!(fetching(&tab), 0);
}
