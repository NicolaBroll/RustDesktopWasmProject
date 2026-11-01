//! Linked worktrees on the `MemoryBackend`: the section shown only with more than one worktree,
//! the dialog from a branch menu (free branch, branch already checked out), `worktree.add` that
//! asks the app to open the new worktree, a failure kept in the dialog, the removal in two steps
//! with the engine's reason, the refusal shown with the engine's text, and the texts.

use super::*;
use crate::Action as TabAction;
use crate::executor::Executor;
use project_engine::{
    Capabilities, MEMORY_MAIN_WORKTREE, MemoryBackend, MemoryOpener, MemoryWorktree, Platform,
    Registry, Session, WorktreeContents,
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

/// A linked worktree at `path` on `branch` (`None`: detached at `oid(9)`), with `contents`
/// (`None`: its folder is gone).
fn linked(path: &str, branch: Option<&str>, contents: Option<WorktreeContents>) -> MemoryWorktree {
    MemoryWorktree {
        info: WorktreeInfo {
            path: path.to_owned(),
            main: false,
            current: false,
            bare: false,
            head: Some(oid(9)),
            branch: branch.map(|b| format!("{HEADS_PREFIX}{b}")),
            detached: branch.is_none(),
            locked: false,
            lock_reason: None,
            prunable: false,
        },
        contents,
        unreferenced: false,
    }
}

fn tab_with(worktrees: Vec<MemoryWorktree>, f: impl FnOnce(&mut MemoryBackend)) -> Tab {
    let mut repo = MemoryBackend::sample();
    for w in worktrees {
        repo.add_linked_worktree(w);
    }
    f(&mut repo);
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

fn run(tab: &mut Tab, action: Action) {
    tab.apply(TabAction::Worktree(action));
    settle(tab);
}

fn worktrees(tab: &Tab) -> &WorktreeBox {
    &tab.repo.as_ref().expect("repo open").worktrees
}

#[test]
fn the_section_is_shown_only_with_more_than_one_worktree() {
    let tab = tab_with(Vec::new(), |_| {});
    let list = worktrees(&tab).list().expect("list read on open");
    assert_eq!(list.len(), 1);
    assert!(list[0].main && list[0].current);
    assert!(!worktrees(&tab).shown());

    let tab = tab_with(
        vec![linked("/wt/graph", Some("feature/graph"), None)],
        |_| {},
    );
    assert!(worktrees(&tab).shown());
    let list = worktrees(&tab).list().unwrap();
    assert_eq!(list[1].short_path(), "graph");
    assert_eq!(list[1].what(), "feature/graph");
    assert!(list[1].prunable);
    assert_eq!(
        worktrees(&tab).branch_in_use("feature/graph"),
        Some("/wt/graph")
    );
    assert_eq!(worktrees(&tab).branch_in_use("other"), None);
}

#[test]
fn the_dialog_from_a_branch_proposes_that_branch_or_a_new_one_from_it() {
    let mut tab = tab_with(Vec::new(), |_| {});
    run(&mut tab, Action::OpenAdd(Some("feature/graph".to_owned())));
    let fields = &worktrees(&tab).dialog().expect("dialog open").fields;
    assert_eq!(fields.mode, Mode::Existing);
    assert_eq!(fields.branch, "feature/graph");
    // The memory repo's path is relative: no folder proposed.
    assert_eq!(fields.path, "");
    run(&mut tab, Action::CloseAdd);
    assert!(worktrees(&tab).dialog().is_none());

    // The current branch cannot be checked out twice: a new branch from it.
    run(&mut tab, Action::OpenAdd(Some("main".to_owned())));
    let fields = &worktrees(&tab).dialog().unwrap().fields;
    assert_eq!(fields.mode, Mode::New);
    assert_eq!(fields.start, "main");
    assert!(fields.new_branch.is_empty());
    assert!(!worktrees(&tab).dialog().unwrap().can_submit());
}

#[test]
fn add_opens_the_new_worktree_and_reads_the_list_again() {
    let mut tab = tab_with(Vec::new(), |_| {});
    run(&mut tab, Action::OpenAdd(None));
    let fields = AddFields {
        path: "/wt/topic".to_owned(),
        mode: Mode::New,
        new_branch: "topic".to_owned(),
        ..AddFields::default()
    };
    run(&mut tab, Action::Fields(fields));
    run(&mut tab, Action::Submit);
    assert!(worktrees(&tab).dialog().is_none());
    assert_eq!(tab.open_worktree.as_deref(), Some("/wt/topic"));
    assert!(tab.notice.contains("topic"), "{}", tab.notice);
    assert!(worktrees(&tab).shown());
    // The new branch is in the refs and undo deletes it.
    let repo = tab.repo.as_ref().unwrap();
    assert!(repo.refs.iter().any(|r| r.name == "refs/heads/topic"));
    assert!(repo.history.can_undo);
}

#[test]
fn a_failed_add_stays_in_the_dialog_with_the_engines_text() {
    let mut tab = tab_with(Vec::new(), |repo| repo.occupy_path("/wt/full"));
    run(&mut tab, Action::OpenAdd(None));
    let fields = AddFields {
        path: "/wt/full".to_owned(),
        mode: Mode::Detached,
        ..AddFields::default()
    };
    run(&mut tab, Action::Fields(fields));
    run(&mut tab, Action::Submit);
    let dialog = worktrees(&tab).dialog().expect("still open");
    assert!(!dialog.running);
    assert!(dialog.error.is_some());
    assert!(tab.open_worktree.is_none());
}

#[test]
fn remove_asks_first_then_removes() {
    let wt = linked(
        "/wt/clean",
        Some("feature/graph"),
        Some(WorktreeContents::default()),
    );
    let mut tab = tab_with(vec![wt], |_| {});
    run(&mut tab, Action::Remove("/wt/clean".to_owned()));
    let remove = worktrees(&tab).remove_dialog().expect("confirmation");
    assert!(matches!(&remove.phase, RemovePhase::Confirm(reason) if !reason.is_empty()));
    assert!(tab.repo.as_ref().unwrap().modal_open());

    run(&mut tab, Action::ConfirmRemove);
    assert!(worktrees(&tab).remove_dialog().is_none());
    assert!(!worktrees(&tab).shown());
    assert!(tab.notice.contains("/wt/clean"), "{}", tab.notice);
}

#[test]
fn cancel_removes_nothing() {
    let wt = linked("/wt/clean", None, Some(WorktreeContents::default()));
    let mut tab = tab_with(vec![wt], |_| {});
    run(&mut tab, Action::Remove("/wt/clean".to_owned()));
    run(&mut tab, Action::CancelRemove);
    assert!(worktrees(&tab).remove_dialog().is_none());
    assert_eq!(worktrees(&tab).list().unwrap().len(), 2);
}

#[test]
fn a_refusal_shows_the_engines_text() {
    let contents = WorktreeContents {
        changes: vec!["a.txt".to_owned()],
        ..WorktreeContents::default()
    };
    let mut tab = tab_with(vec![linked("/wt/busy", None, Some(contents))], |_| {});
    run(&mut tab, Action::Remove("/wt/busy".to_owned()));
    let remove = worktrees(&tab).remove_dialog().expect("refusal shown");
    let RemovePhase::Refused(message) = &remove.phase else {
        panic!("{remove:?}");
    };
    assert!(message.contains("a.txt"), "{message}");
    // A refusal has nothing to confirm.
    run(&mut tab, Action::ConfirmRemove);
    assert_eq!(worktrees(&tab).list().unwrap().len(), 2);
    run(&mut tab, Action::CancelRemove);
    assert!(!tab.repo.as_ref().unwrap().modal_open());
}

#[test]
fn the_main_and_current_worktrees_are_not_removable_from_the_menu() {
    let tab = tab_with(vec![linked("/wt/x", None, None)], |_| {});
    let list = worktrees(&tab).list().unwrap();
    assert_eq!(list[0].path, MEMORY_MAIN_WORKTREE);
    assert!(remove_blocked(&list[0]).is_some());
    assert!(remove_blocked(&list[1]).is_none());
}

#[test]
fn params_follow_the_mode() {
    let mut dialog = AddDialog {
        fields: AddFields {
            path: " ../wt ".to_owned(),
            mode: Mode::Existing,
            branch: "dev".to_owned(),
            new_branch: "ignored".to_owned(),
            start: "v1".to_owned(),
        },
        ..AddDialog::default()
    };
    assert_eq!(dialog.params(), json!({"path": "../wt", "branch": "dev"}));
    dialog.fields.mode = Mode::New;
    assert_eq!(
        dialog.params(),
        json!({"path": "../wt", "new_branch": "ignored", "start": "v1"})
    );
    dialog.fields.mode = Mode::Detached;
    dialog.fields.start.clear();
    assert_eq!(dialog.params(), json!({"path": "../wt"}));
    dialog.fields.path.clear();
    assert!(!dialog.can_submit());
}

#[test]
fn texts() {
    assert_eq!(
        proposed_path("/home/u/repo", "feat/x"),
        "/home/u/repo-feat-x"
    );
    let mut info = linked("/a/b/", None, None).info;
    assert_eq!(info.short_path(), "b");
    assert!(info.what().contains(&oid(9).short()));
    info.locked = true;
    info.lock_reason = Some("usb disk".to_owned());
    assert!(info.tooltip().contains("usb disk"));
}
