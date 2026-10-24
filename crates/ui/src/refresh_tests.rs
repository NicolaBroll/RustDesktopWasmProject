//! Automatic refresh: changes of the app's own writes dropped, deferral while hidden or busy,
//! focus transitions, and on the `MemoryBackend` a change from outside re-read without losing the
//! selection, the log read again only when the refs changed.

use super::*;
use crate::executor::Executor;
use project_engine::{Capabilities, MemoryBackend, MemoryOpener, OnChange, Registry, Session};
use std::sync::Mutex;

fn signal(auto: &AutoRefresh) {
    auto.signal().store(true, Ordering::Release);
}

#[test]
fn a_change_from_outside_makes_the_tab_dirty() {
    let now = Instant::now();
    let mut auto = AutoRefresh::default();
    auto.observe(now, false);
    assert!(!auto.is_dirty());
    signal(&auto);
    auto.observe(now, false);
    assert!(auto.is_dirty());
    assert!(auto.take_due(true, false));
    assert!(!auto.is_dirty());
    assert!(!auto.take_due(true, false), "only once");
}

#[test]
fn changes_during_and_right_after_an_own_write_are_dropped() {
    let start = Instant::now();
    let mut auto = AutoRefresh::default();
    auto.observe(start, true);
    signal(&auto);
    auto.observe(start, true);
    assert!(!auto.is_dirty(), "during the write");
    // The write's result arrived: its debounced events come a little later.
    let end = start + Duration::from_secs(3);
    auto.observe(end, false);
    signal(&auto);
    auto.observe(end + QUIET_AFTER_WRITE / 2, false);
    assert!(!auto.is_dirty(), "right after the write");
    signal(&auto);
    auto.observe(end + QUIET_AFTER_WRITE, false);
    assert!(auto.is_dirty(), "a later change is from outside");
}

#[test]
fn a_hidden_or_blocked_tab_stays_dirty_until_it_can_reread() {
    let mut auto = AutoRefresh::default();
    auto.mark();
    assert!(!auto.take_due(false, false), "hidden");
    assert!(!auto.take_due(true, true), "dialog open or command running");
    assert!(auto.is_dirty());
    assert!(auto.take_due(true, false));
    auto.mark();
    auto.clear();
    assert!(
        !auto.take_due(true, false),
        "a manual refresh read everything"
    );
}

#[test]
fn focus_counts_only_when_gained() {
    let mut last = None;
    assert!(!focus_gained(&mut last, Some(true)), "startup");
    assert!(!focus_gained(&mut last, Some(true)));
    assert!(
        !focus_gained(&mut last, None),
        "unknown keeps the last state"
    );
    assert!(!focus_gained(&mut last, Some(false)));
    assert!(!focus_gained(&mut last, None));
    assert!(focus_gained(&mut last, Some(true)));
    assert!(!focus_gained(&mut last, Some(true)));
}

#[test]
fn the_log_is_stale_only_if_refs_or_head_changed() {
    let a = MemoryBackend::fake_oid(1);
    let b = MemoryBackend::fake_oid(2);
    let r = |target| RefInfo {
        name: "refs/heads/main".to_owned(),
        kind: project_domain::RefKind::LocalBranch,
        target,
        tag_object: None,
        upstream: None,
    };
    assert!(!log_stale(&[r(a)], &[r(a)], Some(Some(a)), Some(a)));
    assert!(log_stale(&[r(a)], &[r(b)], Some(Some(a)), Some(a)));
    assert!(log_stale(&[r(a)], &[r(a), r(b)], Some(Some(a)), Some(a)));
    assert!(
        log_stale(&[r(a)], &[r(a)], Some(Some(a)), Some(b)),
        "detached"
    );
    assert!(log_stale(&[r(a)], &[r(a)], None, Some(a)), "unknown");
}

/// Jobs run inside `spawn`; `watch` keeps the callback for the test.
#[derive(Default)]
struct Manual {
    on_change: Mutex<Option<OnChange>>,
}

impl Platform for Manual {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }
    fn watch(&self, _: &str, on_change: OnChange) -> Option<Watch> {
        *self.on_change.lock().unwrap() = Some(on_change);
        Some(Box::new(()))
    }
}

/// A tab with the sample repo open and loaded, watched through `platform`.
fn tab(platform: &Arc<Manual>) -> Tab {
    let executor = Executor::new(
        Session::new(Arc::new(MemoryOpener::sample())),
        Registry::builtin(),
        Arc::clone(platform) as Arc<dyn Platform>,
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(0, executor);
    tab.open(MemoryOpener::SAMPLE_PATH.to_owned());
    settle(&mut tab);
    let path = tab.repo_path().unwrap().to_owned();
    tab.auto.start(&**platform, &path, Arc::new(|| {}));
    tab
}

/// Collects results until nothing is running (the jobs run inline); returns the commands seen.
fn settle(tab: &mut Tab) -> Vec<String> {
    let mut seen = Vec::new();
    while tab.executor.is_busy() {
        seen.extend(tab.executor.running().iter().map(|j| j.command.clone()));
        tab.receive();
    }
    seen
}

/// A change the tab does not hear about: as from a terminal.
fn from_outside(tab: &mut Tab, command: &str, params: serde_json::Value) {
    tab.executor.submit(command, params);
    let done = tab.executor.poll();
    assert!(done.iter().all(|d| d.result.is_ok()), "{command}");
}

#[test]
fn a_new_branch_from_outside_rereads_refs_and_log_keeping_the_selection() {
    let platform = Arc::new(Manual::default());
    let mut tab = tab(&platform);
    tab.repo.as_mut().unwrap().list.select(1);
    let selected = tab
        .repo
        .as_ref()
        .unwrap()
        .list
        .selected_commit()
        .unwrap()
        .id;
    from_outside(&mut tab, "branch.create", json!({ "name": "outside" }));

    let notify = platform.on_change.lock().unwrap().clone().unwrap();
    notify();
    let now = Instant::now();
    tab.tick(now, true);
    let seen = settle(&mut tab);
    for command in [
        "repo.status",
        "repo.refs",
        "edit.history",
        "stash.list",
        "repo.log",
    ] {
        assert!(seen.iter().any(|c| c == command), "{command}: {seen:?}");
    }
    let repo = tab.repo.as_ref().unwrap();
    assert!(repo.refs.iter().any(|r| r.name == "refs/heads/outside"));
    assert_eq!(repo.list.selected_commit().unwrap().id, selected);

    // A change that moves no ref (the index, the worktree): no log.
    notify();
    tab.tick(now, true);
    let seen = settle(&mut tab);
    assert!(seen.iter().any(|c| c == "repo.status"), "{seen:?}");
    assert!(!seen.iter().any(|c| c == "repo.log"), "{seen:?}");
}

#[test]
fn an_open_dialog_defers_the_refresh_until_it_closes() {
    let platform = Arc::new(Manual::default());
    let mut tab = tab(&platform);
    assert!(
        tab.repo
            .as_mut()
            .unwrap()
            .branch
            .open_rename("main".to_owned())
    );
    signal(&tab.auto);
    let now = Instant::now();
    tab.tick(now, true);
    assert!(!tab.executor.is_busy(), "nothing read with the dialog open");
    assert!(tab.auto.is_dirty());
    tab.repo.as_mut().unwrap().branch = crate::BranchBox::default();
    tab.tick(now, true);
    assert!(settle(&mut tab).iter().any(|c| c == "repo.status"));
    assert!(!tab.auto.is_dirty());
}
