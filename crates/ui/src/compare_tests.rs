//! Comparing two revisions on a tab with the sample repo: Ctrl+click order, files, a file's
//! diff, swap, back with Esc or a plain click, "Compare with the current branch".

use super::*;
use crate::diff::Pane;
use crate::executor::Executor;
use crate::keys::Pressed;
use crate::{Action as TabAction, RepoView};
use project_domain::Signature;
use project_engine::{Capabilities, MemoryOpener, Platform, Registry, Session};
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

fn tab() -> Tab {
    let executor = Executor::new(
        Session::new(Arc::new(MemoryOpener::sample())),
        Registry::builtin(),
        Arc::new(Inline),
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(0, executor);
    tab.open(MemoryOpener::SAMPLE_PATH.to_owned());
    settle(&mut tab);
    assert!(tab.repo.is_some());
    tab
}

/// Applies `action` as a frame does and waits for the answers.
fn act(tab: &mut Tab, action: TabAction) {
    tab.apply(action);
    tab.close_stale_diff();
    tab.request_details();
    tab.request_compare();
    settle(tab);
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

fn compared(tab: &Tab) -> &Compared {
    match &repo(tab).compare.view().expect("comparison open").answer {
        Some(Ok(c)) => c,
        other => panic!("not loaded: {other:?}"),
    }
}

fn commit(n: u8, parents: &[u8]) -> Commit {
    let sig = Signature {
        name: "Ada".to_owned(),
        email: "ada@example.com".to_owned(),
        time: 0,
        offset_min: 0,
    };
    Commit {
        id: Oid::from_bytes([n; 20]),
        parents: parents.iter().map(|&p| Oid::from_bytes([p; 20])).collect(),
        author: sig.clone(),
        committer: sig,
        summary: format!("c{n}"),
        body: String::new(),
    }
}

#[test]
fn the_older_commit_is_from_when_one_is_an_ancestor() {
    // 4 -> 3 -> 1, 2 -> 1: rows children first.
    let commits = [
        commit(4, &[3]),
        commit(2, &[1]),
        commit(3, &[1]),
        commit(1, &[]),
    ];
    // Selected 4, then 1 (its ancestor), and the other way round: 1 is `from`.
    assert_eq!(order(&commits, 0, 3), (3, 0));
    assert_eq!(order(&commits, 3, 0), (3, 0));
    assert_eq!(order(&commits, 2, 0), (2, 0));
    // Unrelated (2 and 3): the commit selected first is `from`.
    assert_eq!(order(&commits, 1, 2), (1, 2));
    assert_eq!(order(&commits, 2, 1), (2, 1));
}

#[test]
fn ctrl_click_compares_and_a_file_opens_its_range_diff() {
    let mut tab = tab();
    let newer = 0;
    let older = repo(&tab).list.commits().len() - 1;
    act(&mut tab, TabAction::Select(newer));
    act(&mut tab, TabAction::Compare(Action::With(older)));
    let commits = repo(&tab).list.commits();
    let (old_id, new_id) = (commits[older].id, commits[newer].id);
    let c = compared(&tab).clone();
    assert_eq!((c.from, c.to, c.base), (old_id, new_id, None));
    assert!(!c.files.is_empty());
    assert!(repo(&tab).compare.marks(old_id) && repo(&tab).compare.marks(new_id));
    // The selection stays on the first commit.
    assert_eq!(repo(&tab).list.selected(), Some(newer));

    let path = c.files[0].path.clone();
    let target = DiffTarget {
        source: c.source(false),
        path: path.clone(),
    };
    assert_eq!(target.request().0, "diff.range");
    act(&mut tab, TabAction::Diff(crate::diff::Action::Open(target)));
    assert!(matches!(repo(&tab).diff.pane(), Pane::Loaded(t, doc)
        if t.path == path && !doc.diff.files.is_empty()));

    // Swap: the sides exchange, the file's diff closes.
    act(&mut tab, TabAction::Compare(Action::Swap));
    assert_eq!(repo(&tab).diff.target(), None);
    let c = compared(&tab);
    assert_eq!((c.from, c.to), (new_id, old_id));

    // Esc goes back to the selected commit.
    let actions = esc_frame(&tab);
    assert!(matches!(
        actions.as_slice(),
        [TabAction::Compare(Action::Close)]
    ));
    act(&mut tab, TabAction::Compare(Action::Close));
    assert!(repo(&tab).compare.view().is_none());
    assert_eq!(repo(&tab).list.selected(), Some(newer));
}

#[test]
fn a_plain_click_ends_the_comparison_and_ctrl_click_alone_selects() {
    let mut tab = tab();
    // Nothing selected but the changes row: Ctrl+click is a plain selection.
    act(&mut tab, TabAction::SelectWorktree);
    act(&mut tab, TabAction::Compare(Action::With(1)));
    assert!(repo(&tab).compare.view().is_none());
    assert_eq!(repo(&tab).list.selected(), Some(1));
    act(&mut tab, TabAction::Compare(Action::With(2)));
    assert!(repo(&tab).compare.view().is_some());
    act(&mut tab, TabAction::Select(3));
    assert!(repo(&tab).compare.view().is_none());
}

#[test]
fn compare_with_the_current_branch_shows_what_it_brings() {
    let mut tab = tab();
    act(
        &mut tab,
        TabAction::Compare(Action::Branch {
            rev: "refs/heads/feature/graph".to_owned(),
            name: "feature/graph".to_owned(),
        }),
    );
    let comparison = repo(&tab).compare.view().expect("open");
    assert!(comparison.merge_base);
    assert_eq!(comparison.from.rev, "HEAD");
    assert_eq!(comparison.from.label, "main");
    let c = compared(&tab).clone();
    let main = repo(&tab).head.target();
    assert_eq!(Some(c.from), main);
    assert!(c.base.is_some());
    assert_eq!(
        c.source(true),
        DiffSource::Range {
            from: c.from,
            to: c.to,
            merge_base: true
        }
    );

    // The other way round: what main brings since feature/graph.
    act(&mut tab, TabAction::Compare(Action::Swap));
    let swapped = compared(&tab);
    assert_eq!((swapped.from, swapped.to), (c.to, c.from));
    assert!(!swapped.files.is_empty());
}

#[test]
fn an_unknown_revision_is_an_error_that_can_be_retried() {
    let mut tab = tab();
    act(
        &mut tab,
        TabAction::Compare(Action::Branch {
            rev: "refs/heads/nope".to_owned(),
            name: "nope".to_owned(),
        }),
    );
    let comparison = repo(&tab).compare.view().expect("open");
    assert!(matches!(&comparison.answer, Some(Err(e)) if !e.is_empty()));
    act(&mut tab, TabAction::Compare(Action::Retry));
    assert!(matches!(
        &repo(&tab).compare.view().expect("open").answer,
        Some(Err(_))
    ));
}

#[test]
fn a_stale_answer_is_dropped() {
    let mut b = CompareBox::default();
    let side = |r: &str| Side::rev(r.to_owned(), r.to_owned());
    b.open(side("a"), side("b"), false);
    let first = b.want().expect("request").seq;
    assert_eq!(b.want(), None);
    b.swap();
    let second = b.want().expect("request").seq;
    assert_ne!(first, second);
    let result = DiffRangeResult {
        from: Oid::from_bytes([1; 20]),
        to: Oid::from_bytes([2; 20]),
        base: None,
        diff: project_domain::Diff { files: Vec::new() },
    };
    assert!(!b.receive(first, Ok(result.clone())));
    assert!(b.receive(second, Ok(result)));
}

fn esc_frame(tab: &Tab) -> Vec<TabAction> {
    let ctx = egui::Context::default();
    let mut actions = Vec::new();
    let input = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    let pressed = Pressed {
        commands: Vec::new(),
        popup: false,
    };
    let mut output = ctx.run_ui(input, |ui| tab.keyboard(ui.ctx(), &pressed, &mut actions));
    output.textures_delta.clear();
    actions
}
