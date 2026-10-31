//! Submodules on the `MemoryBackend`: the section shown only with submodules, the list read again
//! only when it can have changed, texts of the states, the update on a tab (counts, refused ones
//! with the engine's reason, status and list read again), "Stop" once and a cancelled update.

use super::*;
use crate::executor::Executor;
use crate::{Action as TabAction, RepoView};
use project_domain::{Change, EntryKind, Head, Oid, StatusEntry, TrackedInfo};
use project_engine::{
    Capabilities, MemoryBackend, MemoryOpener, Platform, Registry, Session, submodule_state,
};
use std::sync::Arc;

/// The job runs inside `spawn`, on the same thread.
struct Inline;

impl Platform for Inline {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }
}

fn oid(n: u8) -> Oid {
    Oid::from_bytes([n; Oid::LEN])
}

/// A submodule at `path` recorded on `oid(1)`, checked out on `head`, changed by `f`.
fn sub(path: &str, head: Option<u8>, f: impl FnOnce(&mut SubmoduleInfo)) -> SubmoduleInfo {
    let mut info = SubmoduleInfo {
        path: RepoPath::from(path),
        name: Some(path.to_owned()),
        url: Some(format!("../{path}")),
        recorded: Some(oid(1)),
        head: head.map(oid),
        detached: true,
        state: SubmoduleState::InSync,
        changes: false,
        untracked: false,
        operation: None,
        unreferenced: false,
        ignored_overwritten: false,
    };
    f(&mut info);
    info.state = submodule_state(
        info.recorded.is_none(),
        info.head.is_some(),
        info.changes || info.untracked,
        info.head,
        info.recorded,
    );
    info
}

fn tab_with(subs: Vec<SubmoduleInfo>) -> Tab {
    let mut repo = MemoryBackend::sample();
    repo.set_submodules(subs);
    let mut opener = MemoryOpener::new();
    opener.insert("repo", repo);
    let executor = Executor::new(
        Session::new(Arc::new(opener)),
        Registry::builtin(),
        Arc::new(Inline),
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(0, executor);
    tab.open("repo".to_owned());
    settle(&mut tab);
    tab
}

fn settle(tab: &mut Tab) {
    while tab.executor.is_busy() {
        tab.receive();
    }
    tab.receive();
}

fn repo(tab: &Tab) -> &RepoView {
    tab.repo.as_ref().expect("open repo")
}

fn states(tab: &Tab) -> Vec<(String, SubmoduleState)> {
    repo(tab)
        .submodules
        .list()
        .unwrap()
        .iter()
        .map(|s| (s.path_text(), s.state))
        .collect()
}

fn update(tab: &mut Tab, path: Option<&str>, init: bool) {
    tab.apply(TabAction::Submodule(Action::Update {
        path: path.map(RepoPath::from),
        init,
    }));
    settle(tab);
}

#[test]
fn the_section_is_shown_only_with_submodules() {
    let tab = tab_with(Vec::new());
    assert_eq!(repo(&tab).submodules.list(), Some(&[][..]), "read on open");
    assert!(!repo(&tab).submodules.shown());
    let tab = tab_with(vec![sub("lib", Some(1), |_| {})]);
    assert!(repo(&tab).submodules.shown());
    assert_eq!(states(&tab), [("lib".to_owned(), SubmoduleState::InSync)]);
}

#[test]
fn the_list_is_read_again_only_when_it_can_have_changed() {
    let status = |paths: &[&str]| Status {
        head: Head::Detached(oid(1)),
        upstream: None,
        entries: paths
            .iter()
            .map(|p| StatusEntry {
                path: RepoPath::from(*p),
                index: Change::Unmodified,
                worktree: Change::Modified,
                kind: EntryKind::Ordinary(TrackedInfo {
                    submodule: None,
                    mode_head: 0o100644,
                    mode_index: 0o100644,
                    mode_worktree: 0o100644,
                    oid_head: oid(1),
                    oid_index: oid(1),
                }),
            })
            .collect(),
        operation: None,
    };
    let mut b = SubmoduleBox::default();
    b.receive_list(Ok(json!({"submodules": []})));
    assert!(
        !b.wants_list(&status(&["a.txt"])),
        "no submodules: never again"
    );
    assert!(
        b.wants_list(&status(&[".gitmodules"])),
        "one may have been added"
    );
    b.list_sent();
    assert!(
        !b.wants_list(&status(&[".gitmodules"])),
        "already on its way"
    );
    let one = serde_json::to_value(vec![sub("lib", Some(1), |_| {})]).unwrap();
    b.receive_list(Ok(json!({ "submodules": one })));
    assert!(b.wants_list(&status(&[])), "with submodules: every status");
    // A failed read keeps the last list, with the error.
    b.receive_list(Err("boom".into()));
    assert!(b.shown());
    assert_eq!(b.list_error(), Some("boom"));
}

#[test]
fn badges_and_tooltips_explain_the_state() {
    let lost = sub("lost", Some(2), |i| i.unreferenced = true);
    assert_eq!(lost.state, SubmoduleState::DifferentCommit);
    let tooltip = lost.tooltip();
    assert!(tooltip.contains(&oid(1).short()), "recorded: {tooltip}");
    assert!(tooltip.contains(&oid(2).short()), "checked out: {tooltip}");
    assert!(tooltip.contains(&tr!("submodule-tooltip-unreferenced")));
    let dirty = sub("dirty", Some(1), |i| {
        i.changes = true;
        i.untracked = true;
    });
    assert_eq!(dirty.badge(), tr!("submodule-state-local-changes"));
    let tooltip = dirty.tooltip();
    assert!(tooltip.contains(&tr!("submodule-tooltip-changes")));
    assert!(tooltip.contains(&tr!("submodule-tooltip-untracked")));
    let empty = sub("empty", None, |_| {});
    assert!(empty.needs_init());
    assert!(
        empty
            .tooltip()
            .contains(&tr!("submodule-tooltip-not-checked-out"))
    );
}

#[test]
fn update_all_reports_counts_and_readable_refusals_then_rereads() {
    let mut tab = tab_with(vec![
        sub("behind", Some(2), |_| {}),
        sub("dirty", Some(2), |i| i.changes = true),
        sub("empty", None, |_| {}),
        sub("same", Some(1), |_| {}),
    ]);
    update(&mut tab, None, true);
    assert!(repo(&tab).submodules.running().is_none());
    assert!(repo(&tab).write_label.is_none());
    // "dirty" refused: the counts and the reason go together in the status bar.
    let done = tr!("submodule-update-done", updated = 2, unchanged = 1);
    assert!(tab.notice.starts_with(&done), "{}", tab.notice);
    assert!(tab.notice.contains("dirty"), "{}", tab.notice);
    assert!(
        tab.notice.contains("stash"),
        "the engine's reason: {}",
        tab.notice
    );
    // They also stay in the section.
    let problems = repo(&tab).submodules.problems().unwrap();
    assert!(
        problems.contains("dirty") && !problems.contains(&done),
        "{problems}"
    );
    // The list was read again.
    assert_eq!(
        states(&tab),
        [
            ("behind".to_owned(), SubmoduleState::InSync),
            ("dirty".to_owned(), SubmoduleState::LocalChanges),
            ("empty".to_owned(), SubmoduleState::InSync),
            ("same".to_owned(), SubmoduleState::InSync),
        ]
    );
}

#[test]
fn update_of_one_submodule_is_a_notice() {
    let mut tab = tab_with(vec![
        sub("behind", Some(2), |_| {}),
        sub("other", Some(3), |_| {}),
    ]);
    update(&mut tab, Some("behind"), false);
    assert!(tab.error.is_empty(), "{}", tab.error);
    assert_eq!(
        tab.notice,
        tr!("submodule-update-done", updated = 1, unchanged = 0)
    );
    assert_eq!(
        states(&tab),
        [
            ("behind".to_owned(), SubmoduleState::InSync),
            ("other".to_owned(), SubmoduleState::DifferentCommit),
        ]
    );
}

#[test]
fn without_init_an_uninitialized_one_is_refused() {
    let mut tab = tab_with(vec![sub("empty", None, |_| {})]);
    update(&mut tab, Some("empty"), false);
    assert!(tab.notice.contains("empty"), "{}", tab.notice);
    assert_eq!(states(&tab)[0].1, SubmoduleState::Uninitialized);
    update(&mut tab, Some("empty"), true);
    assert_eq!(states(&tab)[0].1, SubmoduleState::InSync);
    assert_eq!(
        repo(&tab).submodules.problems(),
        None,
        "cleared by the next update"
    );
}

#[test]
fn stop_once_and_a_cancelled_update_is_a_notice() {
    let mut b = SubmoduleBox::default();
    let params = b.request(None, true).unwrap();
    assert_eq!(params, json!({"init": true}));
    b.started(Ticket::for_test(4), SubmoduleBox::label(None, true));
    assert_eq!(b.request(None, false), None, "one at a time");
    assert_eq!(b.cancel(), Some(Ticket::for_test(4)));
    assert_eq!(b.cancel(), None, "only once");
    let received = b.receive(
        Ticket::for_test(4),
        Err("cancelled".into()),
        Some(ErrorKind::Cancelled),
        None,
    );
    assert!(matches!(received, Received::Cancelled(_)), "{received:?}");
    assert!(b.running().is_none());
    // One path: the parameters name it.
    let params = b.request(Some(&RepoPath::from("lib")), false).unwrap();
    assert_eq!(params, json!({"paths": ["lib"]}));
}
