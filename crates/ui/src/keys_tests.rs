//! The keys on a tab with the sample repo open: the shortcuts become the buttons' actions, a
//! menu open keeps Esc for itself (U31), Space stages or unstages the file shown.

use crate::diff::{self, DiffSource, DiffTarget};
use crate::executor::Executor;
use crate::keys::{Command, Pressed};
use crate::stage::{self, StageOp};
use crate::{Action, Tab};
use project_domain::RepoPath;
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

/// A tab with the sample repo open and loaded.
fn tab() -> Tab {
    let executor = Executor::new(
        Session::new(Arc::new(MemoryOpener::sample())),
        Registry::builtin(),
        Arc::new(Inline),
        Arc::new(|| {}),
    );
    let mut tab = Tab::new(0, executor);
    tab.open(MemoryOpener::SAMPLE_PATH.to_owned());
    while tab.executor.is_busy() {
        tab.receive();
    }
    assert!(tab.repo.is_some());
    tab
}

fn open_diff(tab: &mut Tab, source: DiffSource, path: &str) {
    let target = DiffTarget {
        source,
        path: RepoPath::from(path),
    };
    tab.apply(Action::Diff(diff::Action::Open(target)));
}

/// The tab's actions of one frame with `pressed` and the raw `events`.
fn frame(tab: &Tab, pressed: &Pressed, events: Vec<egui::Event>) -> Vec<Action> {
    let ctx = egui::Context::default();
    let mut actions = Vec::new();
    let input = egui::RawInput {
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| tab.keyboard(ui.ctx(), pressed, &mut actions));
    output.textures_delta.clear();
    actions
}

fn escape() -> egui::Event {
    egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

fn keys(commands: &[Command]) -> Pressed {
    Pressed {
        commands: commands.to_vec(),
        popup: false,
    }
}

#[test]
fn esc_with_a_menu_open_leaves_the_diff_open() {
    let mut tab = tab();
    open_diff(&mut tab, DiffSource::Worktree, "src/lib.rs");
    let menu = Pressed {
        commands: Vec::new(),
        popup: true,
    };
    assert!(frame(&tab, &menu, vec![escape()]).is_empty());
    let actions = frame(&tab, &keys(&[]), vec![escape()]);
    assert!(matches!(
        actions.as_slice(),
        [Action::Diff(diff::Action::Close)]
    ));
}

#[test]
fn space_stages_or_unstages_the_file_shown() {
    let mut tab = tab();
    open_diff(&mut tab, DiffSource::Worktree, "src/lib.rs");
    let actions = frame(&tab, &keys(&[Command::ToggleStage]), Vec::new());
    assert!(matches!(
        actions.as_slice(),
        [Action::Stage(stage::Action::Write(StageOp::Stage, paths))] if paths[..] == [RepoPath::from("src/lib.rs")]
    ));
    open_diff(&mut tab, DiffSource::Index, "docs/guide.md");
    let actions = frame(&tab, &keys(&[Command::ToggleStage]), Vec::new());
    assert!(matches!(
        actions.as_slice(),
        [Action::Stage(stage::Action::Write(StageOp::Unstage, _))]
    ));
    // A file not in that list (and a commit's diff) does nothing.
    open_diff(&mut tab, DiffSource::Index, "src/lib.rs");
    assert!(frame(&tab, &keys(&[Command::ToggleStage]), Vec::new()).is_empty());
}

#[test]
fn toolbar_shortcuts_become_the_buttons_actions() {
    let tab = tab();
    let actions = frame(
        &tab,
        &keys(&[
            Command::Refresh,
            Command::Fetch,
            Command::Pull,
            Command::Push,
        ]),
        Vec::new(),
    );
    assert!(matches!(
        actions.as_slice(),
        [
            Action::Refresh,
            Action::Fetch(_),
            Action::Sync(crate::sync::Action::Pull(None, None)),
            Action::Sync(crate::sync::Action::Push(None)),
        ]
    ));
    // Nothing to undo in a repo just opened: Ctrl+Z does nothing, like the disabled button.
    assert!(frame(&tab, &keys(&[Command::Undo]), Vec::new()).is_empty());
}

fn press(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// The diff the frame asks to open from the keyboard, if it is its only action.
fn walked(actions: &[Action]) -> Option<&DiffTarget> {
    match actions {
        [Action::Diff(diff::Action::Walk(target))] => Some(target),
        _ => None,
    }
}

#[test]
fn the_arrows_walk_the_stage_files_while_one_is_open() {
    use egui::Key;
    let mut tab = tab();
    tab.apply(Action::SelectWorktree);
    let files = tab.repo.as_ref().unwrap().shown_files();
    let staged = tab.repo.as_ref().unwrap().stage.staged.len();
    assert!(staged >= 1 && files.len() > staged, "both lists have files");
    assert_eq!(files[staged - 1].source, DiffSource::Index);
    assert_eq!(files[staged].source, DiffSource::Worktree);
    // From the last staged file down to the first unstaged one, and back.
    tab.apply(Action::Diff(diff::Action::Open(files[staged - 1].clone())));
    let actions = frame(&tab, &keys(&[]), vec![press(Key::ArrowDown)]);
    assert_eq!(walked(&actions), Some(&files[staged]));
    tab.apply(actions.into_iter().next().unwrap());
    let repo = tab.repo.as_ref().unwrap();
    assert_eq!(repo.diff.target(), Some(&files[staged]), "the diff follows");
    assert_eq!(repo.diff.revealed(), Some(&files[staged]), "and its row");
    let actions = frame(&tab, &keys(&[]), vec![press(Key::ArrowUp)]);
    assert_eq!(walked(&actions), Some(&files[staged - 1]));
    // Two presses in one frame are two steps; Home and End go to the ends.
    let last = files.last().unwrap();
    let actions = frame(
        &tab,
        &keys(&[]),
        vec![press(Key::ArrowDown), press(Key::ArrowDown)],
    );
    assert_eq!(walked(&actions), files.get(staged + 2).or(Some(last)));
    let actions = frame(&tab, &keys(&[]), vec![press(Key::Home)]);
    assert_eq!(walked(&actions), Some(&files[0]));
    let actions = frame(&tab, &keys(&[]), vec![press(Key::End)]);
    assert_eq!(walked(&actions), Some(last));
    // At the end the selection stays; the commit list does not move under the diff.
    tab.apply(Action::Diff(diff::Action::Open(last.clone())));
    assert!(frame(&tab, &keys(&[]), vec![press(Key::ArrowDown)]).is_empty());
    assert!(frame(&tab, &keys(&[]), vec![press(Key::End)]).is_empty());
    // Esc closes the diff: the arrows are the commit list's again.
    let actions = frame(&tab, &keys(&[]), vec![escape()]);
    assert!(matches!(
        actions.as_slice(),
        [Action::Diff(diff::Action::Close)]
    ));
    tab.apply(Action::Diff(diff::Action::Close));
    let actions = frame(&tab, &keys(&[]), vec![press(Key::ArrowDown)]);
    assert!(matches!(actions.as_slice(), [Action::Move(1)]));
}

#[test]
fn the_arrows_walk_the_files_of_a_commit() {
    use egui::Key;
    let mut tab = tab();
    tab.apply(Action::Select(0));
    tab.after_actions(&egui::Context::default());
    while tab.executor.is_busy() {
        tab.receive();
    }
    let files = tab.repo.as_ref().unwrap().shown_files();
    assert!(
        files.len() >= 2,
        "the first commit of the sample has two files"
    );
    assert!(matches!(files[0].source, DiffSource::Commit(_)));
    // Enter on the selected commit opens its first file.
    let actions = frame(&tab, &keys(&[]), vec![press(Key::Enter)]);
    assert_eq!(walked(&actions), Some(&files[0]));
    tab.apply(actions.into_iter().next().unwrap());
    let actions = frame(&tab, &keys(&[]), vec![press(Key::ArrowDown)]);
    assert_eq!(walked(&actions), Some(&files[1]));
    assert!(frame(&tab, &keys(&[]), vec![press(Key::ArrowUp)]).is_empty());
}

#[test]
fn enter_opens_the_first_file_of_the_changes() {
    let mut tab = tab();
    tab.apply(Action::SelectWorktree);
    let first = tab.repo.as_ref().unwrap().shown_files().remove(0);
    let actions = frame(&tab, &keys(&[]), vec![press(egui::Key::Enter)]);
    assert_eq!(walked(&actions), Some(&first));
}

/// [`frame`] while a widget (not a text field) has the keyboard.
fn frame_with_focus(tab: &Tab, events: Vec<egui::Event>) -> Vec<Action> {
    let ctx = egui::Context::default();
    let id = egui::Id::unique("a focused button");
    // The arrows would move the focus away: here it stays, as on a lone widget.
    let filter = egui::EventFilter {
        vertical_arrows: true,
        ..Default::default()
    };
    ctx.memory_mut(|m| {
        m.request_focus(id);
        m.set_focus_lock_filter(id, filter);
    });
    let mut actions = Vec::new();
    let input = egui::RawInput {
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        assert!(ui.memory(|m| m.has_focus(id)));
        tab.keyboard(ui.ctx(), &keys(&[]), &mut actions);
    });
    output.textures_delta.clear();
    actions
}

#[test]
fn with_a_focused_widget_the_arrows_and_enter_are_its_own() {
    use egui::Key;
    let mut tab = tab();
    tab.apply(Action::SelectWorktree);
    let events = || {
        vec![
            press(Key::ArrowDown),
            press(Key::ArrowUp),
            press(Key::Enter),
        ]
    };
    assert!(frame_with_focus(&tab, events()).is_empty(), "commit list");
    let files = tab.repo.as_ref().unwrap().shown_files();
    tab.apply(Action::Diff(diff::Action::Open(files[0].clone())));
    assert!(frame_with_focus(&tab, events()).is_empty(), "file list");
    // Page up/down, Home and End are not keys of the focus.
    let actions = frame_with_focus(&tab, vec![press(Key::End)]);
    assert_eq!(walked(&actions), files.last(), "{actions:?}");
}

#[test]
fn ctrl_b_opens_the_new_branch_dialog_from_head() {
    use crate::branch;
    let mut tab = tab();
    let actions = frame(&tab, &keys(&[Command::NewBranch]), Vec::new());
    let [Action::Branch(branch::Action::NewBranch(start, name))] = actions.as_slice() else {
        panic!("{actions:?}");
    };
    let repo = tab.repo.as_ref().unwrap();
    assert_eq!(Some(start.target), repo.head.target());
    assert_eq!(
        Some(start.label.as_str()),
        crate::view::current_branch(&repo.head)
    );
    assert!(name.is_empty());
    // The dialog opens; with it open the shortcut is not even taken (`App::shortcuts`).
    tab.apply(actions.into_iter().next().unwrap());
    assert!(tab.repo.as_ref().unwrap().modal_open());
}
