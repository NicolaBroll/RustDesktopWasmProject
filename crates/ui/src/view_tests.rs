//! Smoke tests of the drawing code: the whole [`App`] is drawn without a window, one state after
//! the other, and the texts painted are read back. They catch what the tests of the state cannot:
//! a view that panics, that is not drawn in the state it is for, or two widgets sharing an id.

use super::*;
use crate::Action as A;
use crate::diff::{DiffSource, DiffTarget};
use crate::test_util::oid;
use egui::PointerButton::{Primary, Secondary};
use project_domain::{Change, ChangedFile, Commit, Oid, Signature};
use project_engine::{Capabilities, MemoryBackend, MemoryOpener};

/// The job runs inside `spawn`, on the same thread: an action is answered before the next frame.
struct Inline;

impl Platform for Inline {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            open_folder: true,
            external_apps: true,
        }
    }
    fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
        on_done(None);
    }
    fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
        job();
    }
}

/// A text painted in a frame and where.
#[derive(Debug, Clone)]
struct Painted {
    text: String,
    pos: egui::Pos2,
    width: f32,
    /// Cut with "…" (`text` is the whole one).
    cut: bool,
}

/// The app on a screen that nobody sees.
struct Screen {
    ctx: egui::Context,
    app: App,
    size: egui::Vec2,
    time: f64,
}

const PATH: &str = "repo";

impl Screen {
    /// The app with `repo` open in its tab.
    fn open(repo: MemoryBackend) -> Self {
        let mut opener = MemoryOpener::new();
        opener.insert(PATH, repo);
        let mut screen = Self::with(opener, Some(PATH));
        screen.frames(3);
        assert!(screen.tab().repo.is_some(), "{}", screen.tab().error);
        screen
    }

    fn with(opener: MemoryOpener, repo: Option<&str>) -> Self {
        let ctx = egui::Context::default();
        crate::icons::install(&ctx);
        crate::theme::install_text_styles(&ctx);
        let app = App::restore(
            Arc::new(Inline),
            Arc::new(opener),
            Arc::new(|| {}),
            Saved::default(),
            repo.map(str::to_owned),
        );
        Self {
            ctx,
            app,
            size: egui::vec2(1280.0, 800.0),
            time: 0.0,
        }
    }

    fn tab(&self) -> &Tab {
        &self.app.tabs[self.app.active]
    }

    fn repo(&self) -> &crate::RepoView {
        self.tab().repo.as_ref().expect("open repo")
    }

    /// [`Screen::act`], and the texts that must be on screen afterwards.
    #[track_caller]
    fn shows(&mut self, action: crate::Action, wanted: &[&str]) -> Vec<Painted> {
        let painted = self.act(action);
        assert_shows(&painted, wanted);
        painted
    }

    /// Two frames without input, and the texts that must be on screen.
    #[track_caller]
    fn shows_frames(&mut self, wanted: &[&str]) -> Vec<Painted> {
        let painted = self.frames(2);
        assert_shows(&painted, wanted);
        painted
    }

    /// One frame with these input events: the texts it painted.
    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<Painted> {
        self.time += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            time: Some(self.time),
            focused: true,
            events,
            ..Default::default()
        };
        let mut frame = eframe::Frame::_new_kittest();
        let app = &mut self.app;
        let mut output = self
            .ctx
            .run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        // Nobody draws the textures (the font atlas).
        output.textures_delta.clear();
        let mut painted = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut painted);
        }
        let clash = id_clash(&painted);
        assert!(clash.is_none(), "two widgets share an id: {clash:?}");
        painted
    }

    /// `n` frames without input: the last one's texts.
    fn frames(&mut self, n: usize) -> Vec<Painted> {
        let mut painted = Vec::new();
        for _ in 0..n {
            painted = self.frame(Vec::new());
        }
        painted
    }

    /// A click with `button` at `pos`: move, press, release, then the screen as it settles.
    fn click(&mut self, pos: egui::Pos2, button: egui::PointerButton) -> Vec<Painted> {
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame(vec![egui::Event::PointerMoved(pos)]);
        self.frame(vec![press(true)]);
        self.frame(vec![press(false)]);
        self.frames(2)
    }

    /// Clicks with `button` on the first text equal to `text` of `painted`.
    #[track_caller]
    fn click_on(
        &mut self,
        painted: &[Painted],
        text: &str,
        button: egui::PointerButton,
    ) -> Vec<Painted> {
        let Some(target) = painted.iter().find(|p| p.text == text) else {
            panic!("{text:?} is not on screen");
        };
        self.click(target.pos + egui::vec2(4.0, 6.0), button)
    }

    /// What the user asked for, applied as at the end of a frame; then the screen as it settles.
    fn act(&mut self, action: crate::Action) -> Vec<Painted> {
        let ctx = self.ctx.clone();
        self.app.apply(&ctx, action);
        self.app.tabs[self.app.active].after_actions(&ctx);
        self.frames(3)
    }
}

/// A click far from everything: closes the menu that is open.
const ELSEWHERE: egui::Pos2 = egui::pos2(640.0, 720.0);

fn collect(shape: &egui::Shape, out: &mut Vec<Painted>) {
    match shape {
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
        egui::Shape::Text(text) => out.push(Painted {
            text: text.galley.text().to_owned(),
            pos: text.pos,
            width: text.galley.size().x,
            cut: text.galley.elided,
        }),
        _ => {}
    }
}

/// egui's warning for an id used by two widgets in one frame, if it painted one (debug builds).
fn id_clash(painted: &[Painted]) -> Option<&Painted> {
    painted.iter().find(|p| p.text.starts_with('🔥'))
}

/// Every one of `wanted` is in some text of `painted`.
#[track_caller]
fn assert_shows(painted: &[Painted], wanted: &[&str]) {
    for want in wanted {
        assert!(
            painted.iter().any(|p| p.text.contains(want)),
            "{want:?} is not on screen: {:#?}",
            painted.iter().map(|p| &p.text).collect::<Vec<_>>()
        );
    }
}

fn sample() -> Screen {
    Screen::open(MemoryBackend::sample())
}

fn sig(n: u64) -> Signature {
    Signature {
        name: "Ada".into(),
        email: "ada@example.com".into(),
        time: 1_700_000_000 + i64::try_from(n).unwrap() * 60,
        offset_min: 0,
    }
}

/// Commit `n` with parent `parent` that takes `a.txt` from the parent's content to `text`.
fn commit(repo: &mut MemoryBackend, n: u64, parent: Option<u64>, text: &str) {
    let old = parent.map(|p| oid(100 + p));
    repo.set_blob(oid(100 + n), text.to_owned());
    repo.insert_commit(Commit {
        id: oid(n),
        parents: parent.map(oid).into_iter().collect(),
        author: sig(n),
        committer: sig(n),
        summary: format!("c{n}"),
        body: String::new(),
    });
    repo.set_changes(
        oid(n),
        vec![ChangedFile {
            path: RepoPath::from("a.txt"),
            change: if old.is_some() {
                Change::Modified
            } else {
                Change::Added
            },
            original: None,
            score: None,
            old_mode: if old.is_some() { 0o100644 } else { 0 },
            new_mode: 0o100644,
            old_oid: old.unwrap_or(Oid::from_bytes([0; Oid::LEN])),
            new_oid: oid(100 + n),
        }],
    );
}

/// `main` on `c2` and `other` on `c3`, both from `c1` and both changing `a.txt`.
fn diverging() -> MemoryBackend {
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(&mut repo, 1, None, "base\n");
    commit(&mut repo, 2, Some(1), "ours\n");
    commit(&mut repo, 3, Some(1), "theirs\n");
    repo.set_branch("main", oid(2));
    repo.set_branch("other", oid(3));
    repo.set_head_branch("main");
    repo
}

/// The sample without local changes, with `origin/main` on a commit (20) that `main` does not have.
fn diverged() -> MemoryBackend {
    let mut repo = remote_ahead();
    repo.set_entries(Vec::new());
    repo
}

/// The sample, with `origin/main` on a commit (20) that `main` does not have.
fn remote_ahead() -> MemoryBackend {
    let mut repo = MemoryBackend::sample();
    repo.insert_remote_commit(
        "origin",
        Commit {
            id: oid(20),
            parents: vec![oid(11)],
            author: sig(20),
            committer: sig(20),
            summary: "From the remote".into(),
            body: String::new(),
        },
    );
    repo.set_branch_on_remote("origin", "main", oid(20));
    repo
}

/// The harness would see two widgets with one id: the check of every frame is not empty.
#[test]
fn an_id_used_twice_is_seen() {
    let ctx = egui::Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        for (text, y) in [("one", 20.0), ("two", 200.0)] {
            ui.label(text);
            let rect = egui::Rect::from_min_size(egui::pos2(20.0, y), egui::vec2(40.0, 20.0));
            ui.ctx()
                .check_for_id_clash(egui::Id::unique("same"), rect, "test");
        }
    });
    output.textures_delta.clear();
    let mut painted = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut painted);
    }
    assert_shows(&painted, &["one", "two"]);
    assert!(id_clash(&painted).is_some(), "{painted:?}");
}

#[test]
fn welcome_without_a_repo() {
    let mut screen = Screen::with(MemoryOpener::sample(), None);
    let painted = screen.frames(2);
    assert_shows(
        &painted,
        &[
            "No repository open",
            "Open a repository…",
            "Clone repository…",
        ],
    );
}

#[test]
fn the_repo_with_its_panels_at_any_size() {
    let mut screen = sample();
    let painted = screen.frames(1);
    assert_shows(
        &painted,
        &[
            // Bar, refs, list, details, status bar.
            "Fetch",
            "Local branches (2)",
            "Remotes (1)",
            "Tags (1)",
            "Stashes (0)",
            "Uncommitted changes",
            "Merge branch 'feature/graph'",
            "origin/feature/graph",
            "3 files changed",
            "src/main.rs -> src/bin/app.rs",
        ],
    );
    // Narrow and short, then tiny: the panels shrink, the list still shows.
    for size in [egui::vec2(520.0, 300.0), egui::vec2(160.0, 90.0)] {
        screen.size = size;
        screen.frames(3);
    }
    screen.size = egui::vec2(520.0, 300.0);
    assert_shows(&screen.frames(2), &["Commit panel", "Ref parser"]);
}

#[test]
fn the_window_frame_drawn_by_the_app() {
    let mut screen = sample();
    screen.app.frame = view::WindowFrame::Custom;
    assert_shows(&screen.frames(2), &["repo — BinariGit", "File", "Help"]);
    // A merge commit in the details: both parents, compared with the first.
    screen.shows(
        A::Select(2),
        &[
            "Parents",
            "(first parent)",
            "Merge commit",
            "tests/graph.rs",
        ],
    );
    let mut screen = Screen::with(MemoryOpener::sample(), None);
    screen.app.frame = view::WindowFrame::Custom;
    assert_shows(&screen.frames(2), &["BinariGit", "No repository open"]);
}

#[test]
fn the_stage_area_and_its_confirmations() {
    use crate::{clean, commit, discard};
    let mut screen = sample();
    screen.shows(
        A::SelectWorktree,
        &[
            "Staged (2)",
            "Unstaged (3)",
            "docs/guide.md",
            "Commit message",
            "Stage all",
        ],
    );
    screen.shows(
        A::Discard(discard::Action::Discard(vec!["README.md".into()])),
        &["Discard changes?", "Don't discard"],
    );
    screen.act(A::Discard(discard::Action::Cancel));
    screen.shows(
        A::Clean(clean::Action::Delete(vec!["note.txt".into()])),
        &[
            "Delete the untracked file?",
            "1 file, 19 bytes:",
            "Delete 1 file",
        ],
    );
    screen.act(A::Clean(clean::Action::Cancel));
    screen.shows(
        A::Commit(commit::Action::SetAmend(true)),
        &[
            "Commit (amend)",
            "will change: a new commit will replace it",
        ],
    );
}

#[test]
fn a_published_amend_asks_first() {
    use crate::commit::Action as C;
    let mut repo = MemoryBackend::sample();
    repo.set_remote_branch("origin/main", oid(12));
    let mut screen = Screen::open(repo);
    screen.act(A::SelectWorktree);
    screen.act(A::Commit(C::SetAmend(true)));
    screen.shows(
        A::Commit(C::Commit),
        &["Rewrite a commit that is already published?"],
    );
}

#[test]
fn diffs_unified_and_side_by_side() {
    use crate::{diff, lines};
    let mut screen = sample();
    let worktree = DiffTarget {
        source: DiffSource::Worktree,
        path: "README.md".into(),
    };
    screen.act(A::SelectWorktree);
    screen.shows(
        A::Diff(diff::Action::Open(worktree)),
        &[
            "< Graph",
            "unstaged",
            "@@ -1,8 +1,9 @@",
            "Stage hunk",
            "last change: unstaged change",
            "Click lines to select them",
        ],
    );
    screen.shows(
        A::Lines(lines::Action::Select {
            row: 3,
            shift: false,
        }),
        &["Stage selected lines (1)", "Discard lines… (1)"],
    );
    screen.shows(
        A::Lines(lines::Action::DiscardSelected),
        &["Discard 1 changed line of README.md?", "Discard 1 line"],
    );
    screen.act(A::Discard(crate::discard::Action::Cancel));
    screen.app.settings.diff_split = true;
    assert_shows(
        &screen.frames(2),
        &[
            "To stage single lines, switch to the unified view",
            " 2 -",
            " 9 +",
        ],
    );
    screen.size = egui::vec2(480.0, 320.0);
    screen.frames(3);
    screen.size = egui::vec2(1280.0, 800.0);
    // A renamed file of a commit: its title says from where.
    screen.act(A::Select(0));
    let commit = DiffTarget::commit(oid(12), "src/bin/app.rs".into());
    screen.shows(
        A::Diff(diff::Action::Open(commit)),
        &[
            "in commit 943ff9f",
            "renamed  92%  +2 -1",
            "// last change: commit 12",
        ],
    );
    screen.shows(A::Diff(diff::Action::Close), &["Uncommitted changes"]);
}

#[test]
fn branch_and_tag_dialogs() {
    use crate::branch::{Action as B, Start};
    let mut repo = MemoryBackend::sample();
    // Not merged anywhere: deleting it asks first.
    repo.insert_commit(Commit {
        id: oid(30),
        parents: vec![oid(12)],
        author: sig(30),
        committer: sig(30),
        summary: "On its own".into(),
        body: String::new(),
    });
    repo.set_branch("topic", oid(30));
    let mut screen = Screen::open(repo);
    let start = || Start::from_commit(oid(9));
    let steps: Vec<(B, &[&str])> = vec![
        (
            B::NewBranch(start(), String::new()),
            &["New branch", "Switch to the new branch", "Create branch"],
        ),
        (B::NewTag(start()), &["New tag", "Create tag"]),
        (
            B::RenameBranch("main".to_owned()),
            &["Rename branch", "Rename"],
        ),
        (
            B::DeleteBranch("topic".to_owned(), oid(30)),
            &[
                "Delete “topic” (a8ee577)?",
                "The branch is not merged",
                "Delete branch",
            ],
        ),
        (
            B::DeleteTag("v0.1.0".to_owned()),
            &["Delete tag “v0.1.0”?", "Don't delete"],
        ),
        (
            B::SetUpstream("main".to_owned()),
            &[
                "Upstream of “main”",
                "origin/main (current)",
                "Set upstream",
            ],
        ),
        (
            B::CheckoutCommit {
                id: oid(9),
                ask: true,
            },
            &["Check out commit aeaf52f?", "Don't ask again"],
        ),
    ];
    for (action, wanted) in steps {
        screen.shows(A::Branch(action), wanted);
        screen.act(A::Branch(B::CloseDialog));
    }
}

#[test]
fn remote_dialogs() {
    use crate::remotes::{Action as R, EditKind};
    let mut screen = sample();
    let origin = || "origin".to_owned();
    let steps: Vec<(R, &[&str])> = vec![
        (R::Add, &["Add remote", "Fetch right away"]),
        (
            R::Remove(origin()),
            &["Remove remote “origin”?", "What happens:"],
        ),
        (
            R::DeleteBranch(origin(), "main".to_owned()),
            &["Delete “origin/main” from the remote?"],
        ),
        (
            R::Edit(origin(), EditKind::Url),
            &["Edit URL of “origin”", "Separate push URL", "Save URL"],
        ),
        (
            R::Edit(origin(), EditKind::Rename),
            &["Rename remote “origin”"],
        ),
    ];
    for (action, wanted) in steps {
        screen.shows(A::Remotes(action), wanted);
        screen.act(A::Remotes(R::CloseDialog));
    }
}

#[test]
fn stash_dialog_panel_and_drop() {
    use crate::stash::Action as S;
    let mut screen = sample();
    screen.shows(
        A::Stash(S::Open),
        &[
            "Stash changes",
            "Include untracked files (1)",
            "Keep the index",
        ],
    );
    screen.shows(A::Stash(S::Submit), &["Stashes (1)", "on main · 943ff9f"]);
    let id = screen.repo().stash.list().unwrap_or_default()[0].id;
    screen.shows(
        A::Stash(S::Select(id)),
        &[
            "WIP on main: 943ff9f Commit panel",
            "Pop",
            "Restore the index too",
            "Drop…",
        ],
    );
    screen.shows(A::Stash(S::Drop(id)), &["Drop stash@{0}?", "What happens:"]);
}

#[test]
fn conflicts_left_by_a_stash_are_a_notice() {
    use crate::conflict::Action as C;
    use crate::stash::Action as S;
    let mut screen = Screen::open(crate::test_util::stash_in_conflict());
    let id = screen.repo().stash.list().unwrap_or_default()[0].id;
    // The pop stops on a conflict: the notice names the stash and the file, and the stage
    // area opens. No dialog, nothing floating over the bar.
    let painted = screen.shows(
        A::Stash(S::Apply(id, true)),
        &[
            "Conflicts from stash@{0}, which stays in the list",
            "1 conflicted file:",
            "README.md",
            "click one to open the editor and resolve it",
            "Uncommitted changes",
        ],
    );
    assert!(screen.repo().worktree_selected);
    assert!(!screen.repo().modal_open(), "the notice blocks nothing");
    assert!(!screen.repo().auto_fetch_blocked());
    assert!(
        !painted.iter().any(|p| p.text == "Go to changes"),
        "the floating window is gone"
    );
    let top = |text: &str| {
        let found = painted.iter().find(|p| p.text.contains(text));
        found
            .unwrap_or_else(|| panic!("{text:?} is not on screen"))
            .pos
            .y
    };
    assert!(top("Fetch") < top("Conflicts from stash@{0}"));
    assert!(top("Conflicts from stash@{0}") < top("Uncommitted changes"));

    // The file in the notice opens the conflict editor, with the notice still above it.
    let painted = screen.click_on(&painted, "README.md", Primary);
    assert_shows(
        &painted,
        &[
            "Mark as resolved",
            "Conflicts from stash@{0}",
            "Base (common ancestor)",
        ],
    );
    // Resolved: the last conflict is gone and the notice with it.
    screen.act(A::Conflict(C::ChooseAll(project_domain::Choice::Ours)));
    let painted = screen.act(A::Conflict(C::Resolve));
    assert_eq!(
        screen.repo().stash.conflicts(),
        None,
        "{}",
        screen.tab().error
    );
    assert!(!painted.iter().any(|p| p.text.contains("Conflicts from")));

    // After a restart nobody remembers the pop: the status alone brings the notice back.
    let mut repo = crate::test_util::stash_in_conflict();
    {
        use project_engine::{GitStash, StashApply};
        let stash = repo.stashes().unwrap().remove(0);
        let applied = repo
            .stash_apply(&StashApply {
                command: "stash.apply",
                id: stash.id,
                restore_index: false,
            })
            .unwrap();
        assert!(!applied.conflicts.is_empty());
    }
    let mut screen = Screen::open(repo);
    let painted =
        screen.shows_frames(&["Conflicts from a stash", "1 conflicted file:", "README.md"]);
    assert!(!screen.repo().modal_open());
    assert!(!painted.iter().any(|p| p.text.contains("stash@{0}, which")));
}

#[test]
fn merge_pick_and_reset_dialogs() {
    use crate::merge::{Action as M, Pick};
    use crate::reset;
    let mut screen = sample();
    screen.shows(
        A::Merge(M::OpenMerge {
            rev: "feature/graph".to_owned(),
            what: "feature/graph".to_owned(),
        }),
        &[
            "Merge “feature/graph” into “main”",
            "Squash into one commit",
            "Fast-forward only",
        ],
    );
    screen.act(A::Merge(M::CloseDialog));
    // A merge commit has two parents: which one the cherry-pick compares with is asked.
    screen.shows(
        A::Merge(M::Pick {
            pick: Pick::CherryPick,
            rev: oid(10),
            what: "Merge".to_owned(),
            parents: vec![oid(8), oid(9)],
        }),
        &[
            "Cherry-pick of a merge commit",
            "1. 9e5651b — Status parser",
        ],
    );
    screen.act(A::Merge(M::CloseDialog));
    screen.shows(
        A::Reset(reset::Action::Open {
            target: oid(8),
            what: "Status parser".to_owned(),
        }),
        &[
            "Reset “main” to Status parser",
            "“main” moves from 943ff9f to 9e5651b.",
            "Reset (mixed)",
        ],
    );
    screen.shows(
        A::Reset(reset::Action::Mode(reset::Mode::Hard)),
        &["Reset (hard)"],
    );
}

#[test]
fn the_interactive_rebase_editor() {
    use crate::rebase::Action as R;
    use project_domain::RebaseAction;
    let mut repo = MemoryBackend::new();
    repo.set_identity("Ada", "ada@example.com");
    commit(&mut repo, 1, None, "one\n");
    commit(&mut repo, 2, Some(1), "two\n");
    commit(&mut repo, 4, Some(2), "four\n");
    repo.set_branch("main", oid(4));
    repo.set_head_branch("main");
    let mut screen = Screen::open(repo);
    screen.shows(
        A::Rebase(R::Open {
            base: oid(1),
            what: "c1".to_owned(),
        }),
        &[
            "Interactive rebase of “main”",
            "Plan",
            "pick",
            "Result",
            "2.",
            "The plan changes nothing: change an action or the order first",
        ],
    );
    screen.shows(A::Rebase(R::Choose(0, RebaseAction::Reword)), &["reword"]);
    screen.act(A::Rebase(R::Select(0)));
    screen.act(A::Rebase(R::Message(0, "c2, reworded".to_owned())));
    screen.act(A::Rebase(R::Choose(0, RebaseAction::Pick)));
    screen.shows(A::Rebase(R::Choose(1, RebaseAction::Fixup)), &["fixup"]);
    screen.shows(
        A::Rebase(R::Run),
        &["Rebase of “main” done: 2 commits became 1"],
    );
}

#[test]
fn a_merge_in_conflict() {
    use crate::conflict::Action as C;
    use crate::merge::Action as M;
    use crate::stage::{Action as S, StageOp};
    let mut screen = Screen::open(diverging());
    screen.shows(
        A::Merge(M::Merge {
            rev: "other".to_owned(),
            what: "other".to_owned(),
        }),
        &[
            "Merge in progress: merging “other” (c3)",
            "Abort merge…",
            "1 conflicted file:",
        ],
    );
    screen.shows(
        A::SelectWorktree,
        &["There are conflicted files: resolve them and stage them before committing"],
    );
    screen.shows(
        A::Stage(S::Write(StageOp::Stage, vec!["a.txt".into()])),
        &["Mark the conflict as resolved?", "Conflicted file:"],
    );
    screen.act(A::Stage(S::Cancel));
    screen.shows(
        A::Conflict(C::Open("a.txt".into())),
        &[
            "Mark as resolved",
            ">>>>>>> other",
            "Base (common ancestor)",
            "Theirs (other)",
        ],
    );
    screen.shows(
        A::Conflict(C::ChooseBlock(0, project_domain::Choice::Ours)),
        &["no conflict blocks", "Start over"],
    );
    screen.act(A::Conflict(C::Close));
    screen.shows(A::Merge(M::Abort), &["Abort the merge?"]);
    screen.act(A::Merge(M::CloseDialog));
    screen.shows(A::Merge(M::Continue), &["Finish the merge"]);
}

#[test]
fn a_rejected_push_and_failed_network_commands() {
    use crate::fetch::Action as F;
    use crate::sync::Action as S;
    use project_engine::FetchFailure;
    let mut screen = Screen::open(diverged());
    // A rejected push is a notice in the tab: no dialog, git's text only in the details.
    // (The status bar says "rejected" too: this is the text of the notice alone.)
    const NOTICE: &str = "Pull to integrate them, then push again";
    let painted = screen.shows(
        A::Sync(S::Push(None)),
        &[
            "Push of “main” rejected",
            "The remote has commits you don't have: Pull to integrate them",
            "Details…",
            "×",
        ],
    );
    assert!(
        painted.iter().any(|p| p.text == "Pull"),
        "“Pull” is offered"
    );
    assert!(!screen.repo().modal_open(), "the notice blocks nothing");
    assert!(!screen.repo().auto_fetch_blocked());
    assert!(
        !painted.iter().any(|p| p.text.contains("[rejected]")),
        "git's text is in the details"
    );
    // The notice is under the bar, above the graph: it covers neither.
    let top = |text: &str| {
        let found = painted.iter().find(|p| p.text.contains(text));
        found
            .unwrap_or_else(|| panic!("{text:?} is not on screen"))
            .pos
            .y
    };
    assert!(top("Fetch") < top(NOTICE));
    assert!(top(NOTICE) < top("Commit panel"));
    // "Details…": a modal dialog with the whole explanation, git's text and the force.
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    screen.shows(
        A::Sync(S::RejectedDetails),
        &[
            "Nothing changed, neither on the remote nor locally",
            "Message from git:",
            "[rejected] (fetch first)",
            "Copy message",
            "Force (with lease)…",
            "Close",
        ],
    );
    assert!(screen.repo().modal_open());
    screen.frame(vec![key(egui::Key::Escape)]);
    let painted = screen.shows_frames(&[NOTICE]);
    assert!(!screen.repo().modal_open(), "Esc closes the details");
    assert!(!painted.iter().any(|p| p.text.contains("[rejected]")));
    // "×" hides the notice.
    let painted = screen.act(A::Sync(S::DismissRejected));
    assert!(!painted.iter().any(|p| p.text.contains(NOTICE)));

    // After the fetch the force would remove a commit from the remote: it asks. The branch is
    // behind, so the notice of the push rejected again stays.
    screen.shows(A::Fetch(F::Fetch(None)), &["From the remote", "↑1 ↓1"]);
    screen.shows(A::Sync(S::Push(None)), &[NOTICE]);
    screen.act(A::Sync(S::RejectedDetails));
    screen.shows(
        A::Sync(S::ForcePush),
        &["Force push of “main”: overwrite the branch on the remote?"],
    );

    // No local changes here: a commit can be checked out, and the bar says HEAD is detached.
    // The current branch changed: the notice has nothing more to say.
    screen.shows(A::Sync(S::Dismiss), &[NOTICE]);
    let painted = screen.shows(
        A::Branch(crate::branch::Action::CheckoutCommit {
            id: oid(9),
            ask: false,
        }),
        &["Detached at aeaf52f", "Create branch here…"],
    );
    assert!(!painted.iter().any(|p| p.text.contains(NOTICE)));

    // "Pull" from the notice integrates the remote's commits: the notice goes away.
    let mut screen = Screen::open(diverged());
    let painted = screen.shows(A::Sync(S::Push(None)), &[NOTICE]);
    let row = painted.iter().find(|p| p.text == "Details…").unwrap().pos.y;
    let pull = painted.iter().find(|p| p.text == "Pull" && p.pos.y == row);
    let pull = pull.expect("“Pull” in the notice").pos;
    let painted = screen.click(pull + egui::vec2(4.0, 6.0), Primary);
    assert!(!painted.iter().any(|p| p.text.contains(NOTICE)));
    assert_eq!(screen.repo().sync.rejected(), None);

    let mut repo = MemoryBackend::sample();
    let git = "fatal: could not read Username for 'https://example.com'";
    repo.fail_fetch("origin", Some(FetchFailure::Auth(git.into())));
    let mut screen = Screen::open(repo);
    screen.shows(
        A::Fetch(F::Fetch(None)),
        &[
            "Authentication failed",
            "Message from git:",
            git,
            "Close",
            "Copy message",
        ],
    );
    // The failure is a modal dialog: the arrows do not move the selection under it, and Esc
    // closes it.
    let selected = screen.repo().list.selected();
    screen.frame(vec![key(egui::Key::ArrowDown)]);
    screen.frames(2);
    assert_eq!(screen.repo().list.selected(), selected);
    assert!(screen.repo().failure().is_some());
    screen.frame(vec![key(egui::Key::Escape)]);
    screen.frames(2);
    assert!(screen.repo().failure().is_none(), "Esc closes it");
    screen.frame(vec![key(egui::Key::ArrowDown)]);
    screen.frames(2);
    assert_ne!(screen.repo().list.selected(), selected, "the keys are back");

    // A failed pull is the same dialog; Enter presses "Close", which has the keyboard.
    screen.shows(
        A::Sync(S::Pull(None, None)),
        &["Pull of “main” failed", "Authentication failed", "Close"],
    );
    screen.frame(vec![key(egui::Key::Enter)]);
    screen.frames(2);
    assert_eq!(screen.repo().sync.window(), None, "Enter closes it");
    assert!(!screen.repo().modal_open());

    // A checkout refused for the local changes: the short text of merge and pull, and
    // "Stash…" with the keyboard, so Enter opens the stash dialog.
    let painted = screen.shows(
        A::Branch(crate::branch::Action::Checkout("feature/graph".into())),
        &[
            "Cannot check out “feature/graph”",
            "You have uncommitted changes in README.md",
            "Close",
        ],
    );
    let dialog = painted
        .iter()
        .position(|p| p.text == "Cannot check out “feature/graph”")
        .expect("the title");
    assert!(
        painted[dialog..].iter().any(|p| p.text == "Stash…"),
        "“Stash…” is in the dialog"
    );
    assert!(
        !painted.iter().any(|p| p.text.contains("would overwrite")),
        "the long text of the engine is gone"
    );
    screen.frame(vec![key(egui::Key::Enter)]);
    let painted = screen.shows_frames(&["Stash changes", "Include untracked files"]);
    assert!(screen.repo().failure().is_none());
    assert!(
        !painted.iter().any(|p| p.text.contains("Cannot check out")),
        "one dialog at a time"
    );
}

#[test]
fn blame_history_search_and_compare() {
    use crate::file::{Action as F, Mode};
    use crate::{compare, search};
    let mut screen = sample();
    screen.shows(
        A::File(F::Open {
            path: "README.md".into(),
            rev: None,
            mode: Mode::Blame,
        }),
        &[
            "Blame",
            "working tree",
            "Not committed yet",
            "Project documentation.",
        ],
    );
    screen.shows(
        A::File(F::Mode(Mode::History)),
        &[
            "Follow renames",
            "Select a commit to see how it changed this file.",
        ],
    );
    screen.shows(
        A::File(F::Select(0)),
        &["in commit bd64a5d", "@@ -1,6 +1,7 @@"],
    );
    screen.act(A::Diff(crate::diff::Action::Close));
    screen.shows(A::File(F::Close), &["Uncommitted changes"]);

    screen.act(A::Search(search::Action::Open));
    screen.shows(
        A::Search(search::Action::Text("parser".to_owned(), 0.0)),
        &["1 of 3", "Regex"],
    );
    screen.act(A::Search(search::Action::Close));
    screen.act(A::Select(0));
    screen.shows(
        A::Compare(compare::Action::With(3)),
        &["Compare aeaf52f with 943ff9f", "Swap", "7 files changed"],
    );
    screen.shows(
        A::Compare(compare::Action::Swap),
        &["Compare 943ff9f with aeaf52f"],
    );
}

#[test]
fn clone_settings_and_worktree_dialogs() {
    use crate::clone::{Action as C, Kind};
    use crate::settings::{Action as S, Section};
    use crate::worktree::Action as W;
    let mut screen = sample();
    screen.shows(
        A::Clone(C::Open(Kind::Clone)),
        &["Clone a repository", "With submodules", "Browse…"],
    );
    screen.act(A::Clone(C::Close));
    screen.shows(A::Clone(C::Open(Kind::Init)), &["New repository", "Create"]);
    screen.act(A::Clone(C::Close));
    screen.shows(
        A::Settings(S::Open),
        &["Settings", "Theme", "Interface size", "External editor"],
    );
    screen.shows(
        A::Settings(S::Section(Section::Remotes)),
        &["Automatic fetch", "Every 15 minutes"],
    );
    screen.act(A::Settings(S::Close));
    screen.shows(
        A::Worktree(W::OpenAdd(None)),
        &["New worktree", "Existing branch", "Detached commit"],
    );
}

#[test]
fn two_tabs_and_the_light_theme() {
    let mut opener = MemoryOpener::new();
    opener.insert("one", MemoryBackend::sample());
    opener.insert("two", diverging());
    let mut screen = Screen::with(opener, Some("one"));
    screen.frames(2);
    // The second tab shows its own repo.
    screen.shows(
        A::Tabs(Action::Open("two".to_owned())),
        &["one", "two", "No remotes configured", "c3"],
    );
    screen.ctx.set_visuals(egui::Visuals::light());
    screen.shows(A::Tabs(Action::SwitchTab(0)), &["Commit panel"]);
    screen.act(A::SelectWorktree);
    screen.shows(
        A::Diff(crate::diff::Action::Open(DiffTarget {
            source: DiffSource::Index,
            path: "README.md".into(),
        })),
        &["staged", "Unstage hunk"],
    );
}

#[test]
fn menus_open_under_the_pointer() {
    let mut screen = sample();
    let painted = screen.frames(1);
    let menu = screen.click_on(&painted, "Ref parser", Secondary);
    assert_shows(
        &menu,
        &[
            "Copy commit id",
            "New branch from here…",
            "Check out this commit…",
            "Interactive rebase from here…",
            "Reset “main” to here…",
        ],
    );
    let painted = screen.click(ELSEWHERE, Primary);
    let menu = screen.click_on(&painted, "graph", Secondary);
    assert_shows(
        &menu,
        &[
            "Checkout",
            "Copy name",
            "Merge into “main”",
            "Set upstream…",
            "Delete",
        ],
    );
    let painted = screen.click(ELSEWHERE, Primary);
    let menu = screen.click_on(&painted, "origin", Secondary);
    assert_shows(&menu, &["Edit URL…", "Remove…"]);
    screen.click(ELSEWHERE, Primary);
    let painted = screen.act(A::SelectWorktree);
    let menu = screen.click_on(&painted, "note.txt", Secondary);
    assert_shows(
        &menu,
        &[
            "Show diff",
            "File history",
            "Open in editor",
            "Ignore",
            "Delete…",
        ],
    );
    let painted = screen.click(ELSEWHERE, Primary);
    let menu = screen.click_on(&painted, "File", Primary);
    assert_shows(&menu, &["Open recent", "Open terminal here", "Settings…"]);
    let painted = screen.click(ELSEWHERE, Primary);
    let menu = screen.click_on(&painted, "Help", Primary);
    assert_shows(&menu, &["Keyboard shortcuts", "About BinariGit"]);
    let painted = screen.click_on(&menu, "Keyboard shortcuts", Primary);
    assert_shows(&painted, &["Ctrl+Shift+P", "Push the current branch"]);
    assert_eq!(screen.app.help, Some(view::Help::Shortcuts));
    let painted = screen.click_on(&painted, "Close", Primary);
    assert_eq!(screen.app.help, None);
    let menu = screen.click_on(&painted, "Help", Primary);
    let painted = screen.click_on(&menu, "About BinariGit", Primary);
    assert_shows(
        &painted,
        &["A graphical git client", "License: MIT OR Apache-2.0"],
    );

    // The window is modal: the shortcuts of the tab under it do nothing, Esc closes it.
    let key = |key, modifiers| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    };
    screen.frame(vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
    screen.frames(2);
    assert!(!screen.repo().search.is_open(), "Ctrl+F reached the tab");
    assert_eq!(screen.app.help, Some(view::Help::About));
    screen.frame(vec![key(egui::Key::Escape, egui::Modifiers::NONE)]);
    screen.frames(2);
    assert_eq!(screen.app.help, None);
    // Without the window the same keys are the tab's again.
    screen.frame(vec![key(egui::Key::F, egui::Modifiers::COMMAND)]);
    screen.frames(2);
    assert!(screen.repo().search.is_open());
}

#[test]
fn submodules_worktrees_and_lfs() {
    use project_engine::{
        MemoryWorktree, SubmoduleInfo, SubmoduleState, WorktreeContents, WorktreeInfo,
    };
    let mut repo = MemoryBackend::sample();
    // Recorded on commit 1, checked out on `head`.
    let submodule = |head: u64, state| SubmoduleInfo {
        path: "vendor/lib".into(),
        name: Some("lib".to_owned()),
        url: Some("../lib".to_owned()),
        recorded: Some(oid(1)),
        head: Some(oid(head)),
        detached: true,
        state,
        changes: false,
        untracked: false,
        operation: None,
        unreferenced: false,
        ignored_overwritten: false,
    };
    repo.set_submodules(vec![submodule(2, SubmoduleState::DifferentCommit)]);
    repo.add_linked_worktree(MemoryWorktree {
        info: WorktreeInfo {
            path: "/wt/graph".to_owned(),
            main: false,
            current: false,
            bare: false,
            head: Some(oid(9)),
            branch: Some("refs/heads/feature/graph".to_owned()),
            detached: false,
            locked: false,
            lock_reason: None,
            prunable: false,
        },
        contents: Some(WorktreeContents::default()),
        unreferenced: false,
    });
    repo.set_lfs([RepoPath::from("README.md")], false);
    let with_lfs = {
        let mut repo = remote_ahead();
        repo.set_lfs([RepoPath::from("README.md")], true);
        repo
    };
    let mut screen = Screen::open(repo);
    let painted = screen.frames(1);
    assert_shows(
        &painted,
        &[
            "This repository uses Git LFS, which is not installed",
            "Submodules (1)",
            "vendor/lib",
            "other commit",
            "Worktrees (2)",
        ],
    );
    let menu = screen.click_on(&painted, "vendor/lib", Secondary);
    assert_shows(&menu, &["Update", "Open in a new tab"]);
    screen.click(ELSEWHERE, Primary);
    screen.shows(
        A::Worktree(crate::worktree::Action::Remove("/wt/graph".to_owned())),
        &["Remove this worktree?", "Remove worktree"],
    );

    // A checkout that leaves the submodule behind (here: the list from before the command had
    // it on its recorded commit): the notice under the bar.
    screen.act(A::Worktree(crate::worktree::Action::CancelRemove));
    let tab = &mut screen.app.tabs[0];
    tab.executor
        .submit("branch.create", serde_json::json!({ "name": "side" }));
    screen.frames(2);
    let before = serde_json::json!({ "submodules": [submodule(1, SubmoduleState::InSync)] });
    let repo = screen.app.tabs[0].repo.as_mut().unwrap();
    repo.submodules.receive_list(Ok(before), false);
    screen.shows(
        A::Branch(crate::branch::Action::Checkout("side".to_owned())),
        &[
            "1 submodule is not at the recorded commit",
            "Not now",
            "Update",
        ],
    );

    // git-lfs installed: the files stored in it carry a badge. With local changes, a pull that
    // has something to bring is refused in its window.
    let mut screen = Screen::open(with_lfs);
    screen.shows(A::SelectWorktree, &["LFS", "Staged (2)"]);
    screen.shows(
        A::Sync(crate::sync::Action::Pull(None, None)),
        &[
            "Pull of “main” not possible",
            "You have uncommitted changes in README.md",
        ],
    );
}

/// The status bar at the bottom: one line cut before the repo path, the backup ref and the rest
/// of the text only on hover.
#[test]
fn a_long_status_message_is_one_cut_line() {
    use std::time::{Duration, Instant};
    let long = "Undone: Amend 83c3266 “Make the greeting configurable and document the new option \
                in the README, with an example for every platform and a note about the default \
                value” (was 77ef74e); a backup was saved";
    let backup = "refs/project/backup/undo-1795097584-316326506";
    let mut screen = sample();
    for size in [egui::vec2(1200.0, 760.0), egui::vec2(760.0, 520.0)] {
        screen.size = size;
        let text = crate::status::with_backup(long.to_owned(), Some(backup));
        screen.app.tabs[0].notice.set(text);
        let painted = screen.frames(2);
        let message = painted
            .iter()
            .find(|p| p.text.starts_with("Undone: Amend"))
            .expect("the message is drawn");
        assert!(message.cut && message.text == long, "{message:?}");
        assert!(!painted.iter().any(|p| p.text.contains("refs/project")));
        // The repo path is the last text equal to it, at the bottom right: the message ends
        // before it and keeps most of the bar.
        let path = painted.iter().rfind(|p| p.text == PATH).expect("the path");
        assert!((path.pos.y - message.pos.y).abs() < 8.0, "{path:?}");
        let end = message.pos.x + message.width;
        assert!(end <= path.pos.x, "{end} over {path:?}");
        assert!(message.width >= size.x * 0.5, "{message:?}");
        // Hover: the whole message, then the backup ref on its own line.
        let over = message.pos + egui::vec2(40.0, 6.0);
        screen.frame(vec![egui::Event::PointerMoved(over)]);
        let hover = screen.frames(12);
        let whole = format!("{long}\nBackup: {backup}");
        assert!(
            hover.iter().any(|p| p.text == whole),
            "{:#?}",
            hover.iter().map(|p| &p.text).collect::<Vec<_>>()
        );
        screen.frame(vec![egui::Event::PointerMoved(ELSEWHERE)]);
    }
    // An old notice stays (drawn weak), and a short one is whole.
    let tab = &mut screen.app.tabs[0];
    let old = Instant::now() - Duration::from_secs(60);
    tab.notice
        .set_at("Fetch from “origin” done".to_owned(), old);
    assert_eq!(tab.notice.fresh_for(Instant::now()), None);
    let painted = screen.shows_frames(&["Fetch from “origin” done"]);
    let notice = painted.iter().find(|p| p.text.starts_with("Fetch from"));
    assert!(notice.is_some_and(|p| !p.cut), "{notice:?}");
    // An error in several lines is one line too, and takes the place of the notice.
    let rejected = "rejected ".repeat(40);
    screen.app.tabs[0].error = format!("Push failed: {rejected}\nhint: fetch first");
    let painted = screen.frames(2);
    let error = painted
        .iter()
        .find(|p| p.text.starts_with("Push failed"))
        .expect("the error is drawn");
    assert!(error.cut && !error.text.contains('\n'), "{error:?}");
    assert!(!painted.iter().any(|p| p.text.contains("Fetch from")));
    let path = painted.iter().rfind(|p| p.text == PATH).expect("the path");
    assert!(error.pos.x + error.width <= path.pos.x);
}

#[test]
fn notices_under_the_bar() {
    use project_engine::FetchFailure;
    use std::time::{Duration, Instant};
    let mut repo = MemoryBackend::sample();
    let failure = FetchFailure::Auth("fatal: Authentication failed".into());
    repo.fail_fetch("origin", Some(failure));
    let mut screen = Screen::open(repo);
    // The first page of the log was slow and the repo has no commit-graph.
    let ticket = crate::Ticket::for_test(9_999);
    let start = Instant::now();
    let mut slow = crate::optimize::OptimizeBox::new(false);
    slow.log_submitted(ticket, start);
    slow.log_received(ticket, start + Duration::from_secs(30), true);
    let tab = &mut screen.app.tabs[0];
    tab.repo.as_mut().unwrap().optimize = slow;
    // The automatic fetch gets an authentication error: suspended, with a notice.
    let every = Some(Duration::from_secs(60));
    let later = start + Duration::from_secs(600);
    tab.tick_fetch(start, true, every, false);
    tab.tick_fetch(later, true, every, false);
    tab.receive();
    tab.tick_fetch(later, true, every, false);
    assert_shows(
        &screen.frames(2),
        &[
            "The history took 30.0 s to load",
            "Optimize",
            "Automatic fetch paused",
            "Retry",
        ],
    );
}

#[test]
fn a_question_from_git() {
    use project_engine::{CredentialPrompter, CredentialRequest};
    use std::time::{Duration, Instant};
    let mut screen = sample();
    let dialog = Arc::new(crate::CredentialDialog::new());
    screen.app.credentials = dialog.connect(&screen.ctx).unwrap();
    let asker = std::thread::spawn(move || {
        let prompt = "Password for 'https://ada@host': ";
        dialog.ask(CredentialRequest::new(prompt, Some("origin".to_owned())))
    });
    let start = Instant::now();
    while screen.app.credentials.open().is_none() {
        assert!(start.elapsed() < Duration::from_secs(5), "no question");
        screen.frames(1);
    }
    assert_shows(
        &screen.frames(2),
        &[
            "Password for https://ada@host",
            "Remote: origin",
            "Show",
            "OK",
        ],
    );
    screen.act(A::Credentials(crate::credentials::Action::Cancel));
    assert_eq!(asker.join().unwrap(), None);
}
