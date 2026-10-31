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
