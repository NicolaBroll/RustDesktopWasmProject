//! L6 — egui interface of the desktop app.
//!
//! Rule: graphical callbacks do not modify the repository directly, they produce
//! an [`Action`]; commands go through the [`Executor`], which runs them with
//! [`Registry::run`](project_engine::Registry::run) off the UI thread.
//!
//! Layout (`impl-docs/ARCHITECTURE-git.md` §8): one tab per open repo (`tabs`), top bar, refs on the left, commits in the
//! center, details of the selected commit on the right. At the top of the list the "Uncommitted
//! changes" row opens the stage area on the right. A file's diff opens in place of the graph,
//! with the side panel next to it to switch between files. Under the stage area lists there is
//! the commit message box.
//! With an operation in progress (merge, rebase, ...) its panel sits under the bar
//! (`merge_view`), with the conflicted files and the commands to finish or abort it.

mod branch;
mod clone;
mod commit;
mod conflict;
mod details;
mod diff;
mod diff_view;
mod discard;
mod engine_text;
mod executor;
mod fetch;
mod file;
mod format;
mod graph;
mod icons;
mod lines;
mod list;
mod merge;
mod merge_view;
mod optimize;
mod rebase;
mod ref_tree;
mod refresh;
mod remotes;
mod reset;
mod stage;
mod stash;
mod sync;
mod tabs;
mod theme;
mod view;
mod word_diff;

pub use executor::{Done, ErrorKind, Executor, Job, Ticket, Wake};

use branch::{BranchBox, Start};
use commit::{CommitBox, Gate};
use conflict::ConflictBox;
use details::DetailsState;
use diff::DiffDoc;
use diff::{DiffSource, DiffState, DiffTarget, Pane};
use discard::{DiscardBox, History, UndoOp};
use fetch::FetchBox;
use graph::RefLabel;
use list::{CommitList, PageRequest, Seek};
use merge::MergeBox;
use optimize::OptimizeBox;
use project_domain::RebaseAction;
use project_domain::{Commit, CommitDetails, Diff, Head, Oid, RefInfo, RepoPath, Status};

use project_i18n::tr;
use rebase::RebaseBox;
use remotes::{AddFields, RemotesBox};
use serde::Deserialize;
use serde_json::{Value, json};
use stage::{Follow, PendingWrite, Side, StageLists, StageOp};
use stash::{PushFields, StashBox};
use std::collections::HashMap;

use sync::SyncBox;
pub use tabs::App;
pub use view::WindowFrame;

/// What the user asked for during the frame.
#[derive(Debug, Clone)]
enum Action {
    /// Dialog to choose the repo folder.
    PickFolder,
    /// Opens the repo in a tab: a new one, the empty one shown, or the one where it is already open.
    Open(String),
    /// Shows the tab at that index.
    SwitchTab(usize),
    /// Closes the tab at that index (not while it is writing).
    CloseTab(usize),
    /// Drops from the recent list the repos whose folder no longer exists.
    ForgetMissing,
    /// Opens the "Clone…" or "New repository…" dialog.
    OpenClone(clone::Kind),
    /// The fields of that dialog changed.
    CloneFields(clone::Fields),
    /// "Browse…" in that dialog: the folder dialog, the choice goes into the destination.
    BrowseCloneFolder,
    /// "Clone"/"Create" in that dialog.
    SubmitClone,
    /// "Cancel" the clone in progress.
    CancelClone,
    /// Closes that dialog (not while its command runs).
    CloseClone,
    /// Re-reads refs, status and log of the open repo.
    Refresh,
    /// Selects the commit at the given row of the list.
    Select(usize),
    /// Selects the "Uncommitted changes" row: the stage area on the right.
    SelectWorktree,
    /// Selects the commit a ref points to, if it is in the list.
    SelectCommit(Oid),
    /// Moves the selection by that many rows (keyboard).
    Move(isize),
    /// Selects the first commit (`true`) or the last loaded one (`false`).
    Edge(bool),
    /// The list shows rows up to this one (excluded): if needed it asks for the next page.
    Visible(usize),
    /// Asks again for the selected commit's details after an error.
    RetryDetails,
    /// Opens a file's diff of the selected commit in place of the graph.
    OpenDiff(DiffTarget),
    /// Closes the diff and returns to the graph.
    CloseDiff,
    /// Asks again for the open diff after an error.
    RetryDiff,
    /// Stage or unstage the given paths (never empty). Staging conflicted files asks
    /// for confirmation first.
    Write(StageOp, Vec<RepoPath>),
    /// Confirms the pending write.
    ConfirmWrite,
    /// Cancels the write awaiting confirmation.
    CancelWrite,
    /// Hides the error of the last write.
    DismissWriteError,
    /// Discards the unstaged changes of the given files (never empty): first the engine checks
    /// and asks for confirmation, which opens in a dialog.
    Discard(Vec<RepoPath>),
    /// Confirms the pending discard: resends with `"confirm": true`.
    ConfirmDiscard,
    /// Closes the discard dialog without discarding anything.
    CancelDiscard,
    /// Undoes the last journal operation, or redoes the last undone one.
    Undo(UndoOp),
    /// Click on a diff row (index in `DiffDoc::rows`): selects or deselects it;
    /// with Shift selects the range from the last clicked one.
    SelectLine { row: usize, shift: bool },
    /// Dragging over the diff rows.
    DragLines(Drag),
    /// Deselects all the diff rows.
    ClearLines,
    /// Stage (worktree diff) or unstage (index diff) the hunk with this index.
    WriteHunk(usize),
    /// Stage or unstage the selected lines.
    WriteSelectedLines,
    /// Toggles "Amend" in the message box.
    SetAmend(bool),
    /// Commit with the written message (button or Ctrl+Enter).
    Commit,
    /// Confirms the amend of an already published commit: resends with `"confirm": true`.
    ConfirmCommit,
    /// Cancels the amend awaiting confirmation.
    CancelCommit,
    /// Hides the error of the last commit.
    DismissCommitError,
    /// Opens the dialog for a new branch from `start`, with the proposed name.
    NewBranch(Start, String),
    /// Opens the dialog for a new tag on the commit of `start`.
    NewTag(Start),
    /// The text of the tag message changed.
    TagMessage(String),
    /// Opens the dialog to rename the local branch (short name).
    RenameBranch(String),
    /// The text of the branch name dialog changed.
    BranchName(String),
    /// Submits the name dialog: creates or renames.
    SubmitBranchName,
    /// Switches to the local branch (short name).
    Checkout(String),
    /// Deletes the local branch (short name) seen on the given commit: first without confirmation;
    /// if the engine asks for it the dialog opens. The commit goes to the engine as `expected`.
    DeleteBranch(String, Oid),
    /// Confirms the pending deletion: resends with `"confirm": true`.
    ConfirmDeleteBranch,
    /// Closes the open branch dialog without sending anything.
    CloseBranchDialog,
    /// Fetch from the given remote (`None`: the one the engine chooses: upstream, `origin`,
    /// the only one).
    Fetch(Option<String>),
    /// "Stop" the fetch in progress.
    CancelFetch,
    /// Closes the fetch error window.
    DismissFetchError,
    /// "Optimize" in the commit-graph notice: `repo.optimize` in the background.
    Optimize,
    /// "Stop" the `repo.optimize` in progress.
    CancelOptimize,
    /// "Not now" in the commit-graph notice: hidden for this repo view.
    DismissOptimize,
    /// Pull the given branch (`None`: HEAD's), with the given strategy (`None`:
    /// the configured one).
    Pull(Option<String>, Option<sync::Strategy>),
    /// Push the given branch (`None`: HEAD's), never forced.
    Push(Option<String>),
    /// "Force (with lease)…" in the rejected push window: `"force": true` without
    /// `confirm`.
    ForcePush,
    /// "Force the push" in the confirmation: `"force": true, "confirm": true`.
    ConfirmForcePush,
    /// "Stop" the pull or push in progress.
    CancelSync,
    /// Closes the pull and push window (also the force confirmation, without forcing).
    DismissSync,
    /// Opens the "Add remote" dialog.
    AddRemote,
    /// The fields of the "Add remote" dialog changed.
    AddRemoteFields(AddFields),
    /// "Hide" (`true`) or "Show" the URL with credentials in the "Add remote" dialog.
    HideRemoteUrl(bool),
    /// Submits the "Add remote" dialog.
    SubmitAddRemote,
    /// "Stop" the fetch of the add in progress (the remote stays added).
    CancelAddRemote,
    /// Removes the given remote: first without confirmation, the engine always asks and the
    /// dialog opens.
    RemoveRemote(String),
    /// Confirms the pending removal: resends with `"confirm": true`.
    ConfirmRemoveRemote,
    /// Deletes the branch (second value) from the remote (first value): first without
    /// confirmation, the engine always asks and the dialog opens.
    DeleteRemoteBranch(String, String),
    /// Confirms the pending deletion of the remote branch: resends with `"confirm": true` and the
    /// commit seen.
    ConfirmDeleteRemoteBranch,
    /// Closes the open remotes dialog without sending anything.
    CloseRemoteDialog,
    /// Closes the fetch error window after the add.
    DismissRemoteError,
    /// Opens the "Stash" dialog to set changes aside.
    OpenStash,
    /// The fields of the "Stash" dialog changed.
    StashFields(PushFields),
    /// Submits the "Stash" dialog (`stash.push`).
    SubmitStash,
    /// Selects the stash with this id: its files on the right, its base in the graph.
    SelectStash(Oid),
    /// Applies the stash with this id (`true`: pop, i.e. apply and, if all goes well, delete
    /// it), with `index` and `expected` from the list seen.
    ApplyStash(Oid, bool),
    /// "Also restore the stage" for apply and pop.
    StashRestoreIndex(bool),
    /// Deletes the stash with this id: first without confirmation, the engine always asks and
    /// the dialog opens.
    DropStash(Oid),
    /// Confirms the pending deletion: resends with `"confirm": true`.
    ConfirmDropStash,
    /// Closes the open stash dialog without sending anything.
    CloseStashDialog,
    /// Closes the apply or pop conflicts window.
    DismissStashConflicts,
    /// Merges `rev` (shown as `what`) into the active branch; `no_ff`: always a merge
    /// commit.
    Merge {
        rev: String,
        what: String,
        no_ff: bool,
    },
    /// Opens the "Continue merge" dialog.
    ContinueMerge,
    /// The message of the "Continue merge" dialog changed.
    MergeMessage(String),
    /// Finishes the merge with the dialog's message (`merge.continue`).
    SubmitContinueMerge,
    /// Starts aborting the merge: the engine answers with the confirmation.
    AbortMerge,
    /// The user confirmed the abort.
    ConfirmAbortMerge,
    /// Cherry-pick or revert the commit `rev` (shown as `what`) on the active branch; with several
    /// `parents` (merge commit) the parent dialog opens first.
    Pick {
        pick: merge::Pick,
        rev: Oid,
        what: String,
        parents: Vec<Oid>,
    },
    /// The parent (from 1) chosen in the `mainline` dialog.
    Mainline(usize),
    /// Submits the cherry-pick or revert with the chosen parent.
    SubmitMainline,
    /// Closes the open merge, cherry-pick or revert dialog.
    CloseMergeDialog,
    /// Closes the window of a rejected merge.
    DismissMergeError,
    /// Opens the interactive rebase editor for the commits after `base` (the parent of the chosen
    /// commit, shown as `what`).
    OpenRebase { base: Oid, what: String },
    /// Moves step `from` into the gap above row `to` of the rebase plan (`len` = at the bottom).
    RebaseMove { from: usize, to: usize },
    /// The action of a step of the rebase plan.
    RebaseChoose(usize, RebaseAction),
    /// The message of a step of the rebase plan.
    RebaseMessage(usize, String),
    /// Selects the step whose message is edited.
    RebaseSelect(usize),
    /// "Run rebase" (`rebase.start`).
    RunRebase,
    /// Confirms rewriting published commits (`"confirm": true`).
    ConfirmRebase,
    /// "Back to the plan" from the confirmation.
    CancelRebaseConfirm,
    /// Closes the rebase editor.
    CloseRebase,
    /// Opens the stage area and, in the center, the file's conflict editor.
    OpenConflict(RepoPath),
    /// Closes the conflict editor (unmarked choices are lost).
    CloseConflict,
    /// "Start over": re-reads the conflicted file.
    RestartConflict,
    /// The result of the conflict editor, edited by hand.
    ConflictText(String),
    /// Replaces the conflict block with this index with the choice.
    ChooseBlock(usize, project_domain::Choice),
    /// Replaces all conflict blocks with the same choice.
    ChooseAll(project_domain::Choice),
    /// "Mark as resolved" with the result (asks for confirmation if markers remain).
    ResolveConflict,
    /// Resolves with the whole version of one side.
    UseSide(conflict::Pick),
    /// "Delete the file": opens the confirmation.
    DeleteConflicted,
    /// Confirms the pending resolution (remaining markers or deletion).
    ConfirmConflict,
    /// Gives up the resolution awaiting confirmation.
    CancelConflict,
    /// Opens the dialog to reset the current branch (or a detached HEAD) to `target` (shown as
    /// `what`): the plan of the chosen mode is read first.
    OpenReset { target: Oid, what: String },
    /// The mode chosen in the reset dialog: its plan is read again.
    ResetMode(reset::Mode),
    /// "Reset" in the dialog (`branch.reset` with the plan's `expected`).
    SubmitReset,
    /// Closes the reset dialog without resetting.
    CloseReset,
    /// Opens blame or history of `path` at commit `rev` (`None`: working tree / HEAD).
    OpenFile {
        path: RepoPath,
        rev: Option<Oid>,
        mode: file::Mode,
    },
    /// Closes the blame and history view.
    CloseFile,
    /// Switches between blame and history.
    FileMode(file::Mode),
    /// "Follow renames" of the history.
    FileFollow(bool),
    /// "Load more" of the history.
    FileMore,
    /// Asks again for the tab that failed.
    FileRetry,
    /// Shows the diff of entry `n` of the history.
    FileSelect(usize),
}

/// A step of dragging over the diff rows.
#[derive(Debug, Clone, Copy)]
enum Drag {
    Start(usize),
    Over(usize),
    End,
}

/// Result of `index.stage`/`index.unstage` (and the per-line versions): the new status and the index backup ref,
/// if one was saved before writing.
#[derive(serde::Deserialize)]
struct WriteResult {
    status: Status,
    backup: Option<String>,
}

/// What a command sent to the executor is for. Reads carry the generation of the open repo:
/// if another one was opened in the meantime the result is discarded.
#[derive(Debug, Clone, Copy)]
enum Request {
    /// `index.stage`/`index.unstage`/`index.stage_lines`/`index.unstage_lines`: the result
    /// is the new status.
    Write(u64),
    Open,
    /// `repo.clone`/`repo.init` (the dialog state is in [`clone::CloneBox`], held by [`App`]).
    Clone,
    Refs(u64),
    Status(u64),
    /// `repo.status` of the automatic refresh: afterwards the refs ([`Request::AutoRefs`]).
    AutoStatus(u64),
    /// `repo.refs` of the automatic refresh: the log is read again only if they changed.
    AutoRefs(u64),
    Log(u64, PageRequest),
    /// `commit.show` of the given commit.
    Show(u64, Oid),
    /// `diff.commit` with the request number of [`DiffState`].
    Diff(u64, u64),
    /// `commit.create`: the result is the new commit and the status.
    Commit(u64),
    /// `commit.show` of HEAD to propose its message with amend, when HEAD is not among
    /// the loaded commits.
    HeadMessage(u64),
    /// `worktree.discard`, without or with `confirm` (the phase is in [`DiscardBox`]).
    Discard(u64),
    /// `edit.undo`/`edit.redo`.
    Undo(u64),
    /// `edit.history`: the state of the Undo and Redo buttons.
    History(u64),
    /// `branch.*` (the dialog state is in [`BranchBox`]).
    Branch(u64, branch::Op),
    /// `remote.fetch` (the state is in [`FetchBox`]).
    Fetch(u64),
    /// `repo.optimize` (the state is in [`OptimizeBox`]).
    Optimize(u64),
    /// `remote.pull`/`remote.push` (the state is in [`SyncBox`]).
    Sync(u64),
    /// `remote.list`.
    Remotes(u64),
    /// `remote.add`/`remote.remove` (the dialog state is in [`RemotesBox`]).
    Remote(u64, remotes::Op),
    /// `stash.list`.
    Stashes(u64),
    /// `stash.push`/`stash.apply`/`stash.pop`/`stash.drop` (the state is in [`StashBox`]).
    Stash(u64, stash::Op),
    /// `repo.operation`: the operation in progress for the panel.
    Operation(u64),
    /// `merge.*`, `commit.cherry_pick`, `commit.revert`, `cherry_pick.*`, `revert.*` (the state
    /// is in [`MergeBox`]).
    Merge(u64, merge::Op),
    /// `rebase.plan` for the editor (the state is in [`RebaseBox`]).
    RebasePlan(u64),
    /// `rebase.start` from the editor.
    Rebase(u64),
    /// `conflict.show` with the read number of [`ConflictBox`].
    ConflictShow(u64, u64),
    /// `conflict.resolve` (the state is in [`ConflictBox`]).
    ConflictResolve(u64),
    /// `branch.reset_preview` with the request number of [`reset::ResetBox`].
    ResetPreview(u64, u64),
    /// `branch.reset` (the dialog state is in [`reset::ResetBox`]).
    Reset(u64),
    /// `file.blame`/`file.log` with the request number of [`file::FileBox`].
    File(u64, u64),
}

/// The repo shown and the data read so far.
struct RepoView {
    path: String,
    head: Head,
    refs: Vec<RefInfo>,
    status: Option<Status>,
    /// The status entries split for the stage area.
    stage: StageLists,
    /// True if the "Uncommitted changes" row is selected instead of a commit.
    worktree_selected: bool,
    /// Brings the changes row into view on the next frame.
    scroll_header: bool,
    /// Write awaiting confirmation.
    confirm: Option<PendingWrite>,
    /// The write in progress, described (e.g. "Stage of note.txt").
    write_label: Option<String>,
    /// The error of the last write.
    write_error: Option<String>,
    list: CommitList,
    /// Ref labels per commit, rebuilt when refs or HEAD change.
    labels: HashMap<Oid, Vec<RefLabel>>,
    /// Grows every time `labels` is rebuilt: the left column rebuilds its tree only then.
    refs_version: u64,
    /// Details of the selected commit (`commit.show`).
    details: DetailsState,
    /// Diff of the open file (`diff.commit`), shown in place of the graph.
    diff: DiffState,
    /// The commit message box: draft, commit in progress, confirmation, error.
    commit: CommitBox,
    /// Discarding changes: check, confirmation, submission.
    discard: DiscardBox,
    /// Branch commands: name dialogs, deletion confirmation, checkout.
    branch: BranchBox,
    /// The fetch in progress and the last error to explain.
    fetch: FetchBox,
    /// The commit-graph notice and `repo.optimize` in progress.
    optimize: OptimizeBox,
    /// The pull or push in progress and the open window (conflict, rejection, force confirmation).
    sync: SyncBox,
    /// The configured remotes (`remote.list`) and the dialogs to add and remove them.
    remotes: RemotesBox,
    /// The stashes (`stash.list`), the selected stash, the push and drop dialogs.
    stash: StashBox,
    /// The merge started, the operation in progress (panel) and the Continue and Abort dialogs.
    merge: MergeBox,
    /// The interactive rebase editor.
    rebase: RebaseBox,
    /// The reset dialog (mode, plan, submission).
    reset: reset::ResetBox,
    /// The conflict editor, in place of the graph when it is open.
    conflict: ConflictBox,
    /// Blame and history of a file, in place of the graph when open.
    file: file::FileBox,
    /// Grows with every new status: the operation in progress is re-read once per status.
    status_seq: u64,
    /// What Undo and Redo would do.
    history: History,
    /// The commit to select when the first page of the log arrives (after a commit, if the
    /// changes row has disappeared).
    select_after_load: Option<Oid>,
    /// HEAD when the automatic refresh in progress started (see `refresh::log_stale`).
    refresh_head: Option<Option<Oid>>,
}

impl RepoView {
    fn new(path: String, head: Head) -> Self {
        // On opening HEAD is selected: with all refs in the graph it may not be the first
        // commit (a just-fetched remote branch is more recent).
        let select_after_load = head.target();
        let mut view = Self {
            path,
            head,
            refs: Vec::new(),
            status: None,
            stage: StageLists::default(),
            worktree_selected: false,
            scroll_header: false,
            confirm: None,
            write_label: None,
            write_error: None,
            list: CommitList::default(),
            labels: HashMap::new(),
            refs_version: 0,
            details: DetailsState::default(),
            diff: DiffState::default(),
            commit: CommitBox::default(),
            discard: DiscardBox::default(),
            branch: BranchBox::default(),
            fetch: FetchBox::default(),
            optimize: OptimizeBox::default(),
            sync: SyncBox::default(),
            remotes: RemotesBox::default(),
            stash: StashBox::default(),
            merge: MergeBox::default(),
            rebase: RebaseBox::default(),
            reset: reset::ResetBox::default(),
            conflict: ConflictBox::default(),
            file: file::FileBox::default(),
            status_seq: 0,
            history: History::default(),
            select_after_load,
            refresh_head: None,
        };
        view.update_labels();
        view
    }

    fn update_labels(&mut self) {
        self.refs_version += 1;
        self.labels = graph::ref_labels(&self.refs, &self.head);
        let stashes = self.stash.list().unwrap_or_default();
        graph::add_stash_labels(
            &mut self.labels,
            stashes.iter().map(|s| (s.base, s.name.as_str())),
        );
    }

    /// New repo status, from `repo.status` or a write. Without changes the changes
    /// row disappears: if it was selected we go back to the commit.
    fn set_status(&mut self, status: Status) {
        self.head = status.head.clone();
        self.stage = StageLists::new(&status);
        self.conflict.follow_status(&status);
        self.status_seq += 1;
        self.status = Some(status);
        if self.stage.is_empty() {
            self.worktree_selected = false;
            self.confirm = None;
            self.discard.cancel();
        }
        self.update_labels();
    }

    /// True if a confirmation is open (stage of conflicts, published amend, discard) or a
    /// branch dialog: while it is, no writes and the keys go only to it.
    fn modal_open(&self) -> bool {
        self.confirm.is_some()
            || self.commit.confirm().is_some()
            || self.discard.confirm().is_some()
            || self.branch.modal_open()
            || self.sync.modal_open()
            || self.remotes.modal_open()
            || self.stash.modal_open()
            || self.merge.modal_open()
            || self.rebase.modal_open()
            || self.reset.modal_open()
    }

    /// The last commit (HEAD), if it is among the loaded ones.
    fn head_commit(&self) -> Option<&Commit> {
        let head = self.head.target()?;
        self.list.commits().iter().find(|c| c.id == head)
    }

    /// The state of the "Commit" button.
    fn commit_gate(&self, writing: bool) -> Gate<'_> {
        Gate {
            draft: &self.commit.draft,
            status: self.status.as_ref(),
            staged: self.stage.staged.len(),
            has_head: self.head.target().is_some(),
            busy: writing || self.modal_open(),
        }
    }

    /// True if the "Uncommitted changes" row is at the top of the list.
    fn has_changes(&self) -> bool {
        !self.stage.is_empty()
    }

    /// Selects the changes row (`true`) or goes back to the commits.
    fn select_worktree(&mut self, on: bool) {
        self.worktree_selected = on && self.has_changes();
        if self.worktree_selected {
            self.stash.select(None);
        }
        if self.worktree_selected {
            self.scroll_header = true;
        }
    }

    /// Arrows and Page up/down: the changes row sits above the first commit.
    fn move_selection(&mut self, delta: isize) {
        if self.worktree_selected {
            if delta > 0 && !self.list.commits().is_empty() {
                self.worktree_selected = false;
                self.list.select(0);
                self.list.move_selection(delta - 1);
            }
            return;
        }
        let above_top = self
            .list
            .selected()
            .is_none_or(|i| i.checked_add_signed(delta).is_none());
        if delta < 0 && above_top && self.has_changes() {
            self.select_worktree(true);
        } else {
            self.list.move_selection(delta);
        }
    }
}

/// Where the commit list was in the last frame: needed to bring the selection into view and
/// to know how many rows Page down skips.
#[derive(Debug, Default)]
struct ListViewport {
    /// Rows drawn, even only partly.
    rows: std::ops::Range<usize>,
    height: f32,
}

#[derive(Deserialize)]
struct Opened {
    path: String,
    head: Head,
    /// False: git reads no commit-graph, the log of a big repo is slow (S36).
    commit_graph: bool,
}

#[derive(Deserialize)]
struct LogPage {
    commits: Vec<Commit>,
    has_more: bool,
}

/// One open repository: its own `Session` (journal for undo/redo included) in the executor, the
/// requests it is waiting for and its view. Results always land in the tab that sent the request,
/// shown or not.
struct Tab {
    /// Stable for the life of the tab (not its index): salts the ids of its panels and scroll
    /// areas, so widths and scroll positions stay per tab.
    id: u64,
    executor: Executor,
    pending: HashMap<Ticket, Request>,
    /// Grows with every repo opened.
    generation: u64,
    repo: Option<RepoView>,
    /// The path asked to `repo.open`, until its answer arrives.
    requested: Option<String>,
    /// The path `repo.open` just opened, collected by [`App`] for the tabs and the recent list.
    opened: Option<String>,
    /// The path `repo.open` refused because there is no repository there: [`App`] drops it from
    /// the recent list.
    not_found: Option<String>,
    /// The answer of `repo.clone`/`repo.init`, collected by [`App`] for its dialog.
    clone_done: Option<Done>,
    viewport: ListViewport,
    error: String,
    /// Informational message in the status bar (e.g. the saved index backup).
    notice: String,
    /// The watch of the repo and the refresh it asks for (S40).
    auto: refresh::AutoRefresh,
}

impl Tab {
    fn new(id: u64, executor: Executor) -> Self {
        Self {
            id,
            executor,
            pending: HashMap::new(),
            generation: 0,
            repo: None,
            requested: None,
            opened: None,
            not_found: None,
            clone_done: None,
            viewport: ListViewport::default(),
            error: String::new(),
            notice: String::new(),
            auto: refresh::AutoRefresh::default(),
        }
    }

    /// Opens `path` in this tab, in the background.
    fn open(&mut self, path: String) {
        self.submit("repo.open", json!({ "path": &path }), Request::Open);
        self.requested = Some(path);
    }

    /// Sends `repo.clone`/`repo.init` (the session opens the new repo at the end): the answer
    /// waits in `clone_done`.
    fn create_repo(&mut self, command: &str, params: Value) -> Ticket {
        let ticket = self.executor.submit(command, params);
        self.pending.insert(ticket, Request::Clone);
        ticket
    }

    /// The repo created by [`Tab::create_repo`] is open in the session: shown as after
    /// `repo.open`, with git's messages (`notice`) in the status bar.
    fn created(&mut self, value: Value, notice: String) {
        match self.store(Request::Open, value) {
            Ok(()) => self.notice = notice,
            Err(e) => {
                self.error = tr!(
                    "common-unexpected-result",
                    command = "repo.clone",
                    error = e.to_string()
                );
            }
        }
    }

    /// The repo of the tab, or the one being opened.
    fn path(&self) -> Option<&str> {
        self.repo_path().or(self.requested.as_deref())
    }

    /// The repo open in the tab.
    fn repo_path(&self) -> Option<&str> {
        self.repo.as_ref().map(|r| r.path.as_str())
    }

    /// Nothing open and nothing in progress: a new repo can open here.
    fn is_empty(&self) -> bool {
        self.repo.is_none() && !self.executor.is_busy()
    }

    fn submit(&mut self, command: &str, params: Value, request: Request) {
        let ticket = self.executor.submit(command, params);
        self.pending.insert(ticket, request);
    }

    fn apply(&mut self, action: Action) {
        match action {
            // The tabs and the folder dialog belong to `App`.
            Action::PickFolder
            | Action::Open(_)
            | Action::SwitchTab(_)
            | Action::CloseTab(_)
            | Action::ForgetMissing
            | Action::OpenClone(_)
            | Action::CloneFields(_)
            | Action::BrowseCloneFolder
            | Action::SubmitClone
            | Action::CancelClone
            | Action::CloseClone => {}
            // During a write (a fetch can last) reads would wait in the queue.
            Action::Refresh => {
                if self.executor.writing().is_none() {
                    self.auto.clear();
                    self.load();
                }
            }
            Action::Select(index) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                    repo.select_worktree(false);
                    repo.list.select(index);
                }
            }
            Action::SelectWorktree => {
                if let Some(repo) = &mut self.repo {
                    repo.select_worktree(true);
                }
            }
            Action::SelectCommit(id) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                }
                self.seek(id);
            }
            Action::Move(delta) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                    repo.move_selection(delta);
                }
            }
            Action::Edge(first) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.select(None);
                    if first && repo.has_changes() {
                        repo.select_worktree(true);
                    } else {
                        repo.select_worktree(false);
                        repo.list.select_edge(first);
                    }
                }
            }
            Action::RetryDetails => {
                if let Some(repo) = &mut self.repo {
                    repo.details.retry();
                }
            }
            Action::OpenDiff(target) => {
                if let Some(repo) = &mut self.repo {
                    repo.conflict.close();
                    repo.file.close();
                }
                if let Some(seq) = self.repo.as_mut().and_then(|r| r.diff.open(target.clone())) {
                    self.request_diff(&target, seq);
                }
            }
            Action::CloseDiff => {
                if let Some(repo) = &mut self.repo {
                    repo.diff.close();
                }
            }
            Action::RetryDiff => {
                let repo = self.repo.as_mut();
                if let Some((seq, target)) = repo.and_then(|r| {
                    let seq = r.diff.retry()?;
                    Some((seq, r.diff.target()?.clone()))
                }) {
                    self.request_diff(&target, seq);
                }
            }
            Action::Visible(end) => {
                if let Some(request) = self.repo.as_mut().and_then(|r| r.list.next_page(end)) {
                    self.request_page(request);
                }
            }
            Action::Write(..)
            | Action::ConfirmWrite
            | Action::CancelWrite
            | Action::DismissWriteError => self.apply_stage(action),
            Action::Discard(..)
            | Action::ConfirmDiscard
            | Action::CancelDiscard
            | Action::Undo(..) => self.apply_discard(action),
            Action::SelectLine { .. }
            | Action::DragLines(..)
            | Action::ClearLines
            | Action::WriteHunk(..)
            | Action::WriteSelectedLines => self.apply_lines(action),
            Action::SetAmend(..)
            | Action::Commit
            | Action::ConfirmCommit
            | Action::CancelCommit
            | Action::DismissCommitError => self.apply_commit(action),
            Action::NewBranch(..)
            | Action::NewTag(..)
            | Action::TagMessage(..)
            | Action::RenameBranch(..)
            | Action::BranchName(..)
            | Action::SubmitBranchName
            | Action::Checkout(..)
            | Action::DeleteBranch(..)
            | Action::ConfirmDeleteBranch
            | Action::CloseBranchDialog => self.apply_branch(action),
            Action::Fetch(..) | Action::CancelFetch | Action::DismissFetchError => {
                self.apply_fetch(action)
            }
            Action::Optimize | Action::CancelOptimize | Action::DismissOptimize => {
                self.apply_optimize(action)
            }
            Action::Pull(..)
            | Action::Push(..)
            | Action::ForcePush
            | Action::ConfirmForcePush
            | Action::CancelSync
            | Action::DismissSync => self.apply_sync(action),
            Action::AddRemote
            | Action::AddRemoteFields(..)
            | Action::HideRemoteUrl(..)
            | Action::SubmitAddRemote
            | Action::CancelAddRemote
            | Action::RemoveRemote(..)
            | Action::ConfirmRemoveRemote
            | Action::DeleteRemoteBranch(..)
            | Action::ConfirmDeleteRemoteBranch
            | Action::CloseRemoteDialog
            | Action::DismissRemoteError => self.apply_remotes(action),
            Action::OpenStash
            | Action::StashFields(..)
            | Action::SubmitStash
            | Action::SelectStash(..)
            | Action::ApplyStash(..)
            | Action::StashRestoreIndex(..)
            | Action::DropStash(..)
            | Action::ConfirmDropStash
            | Action::CloseStashDialog
            | Action::DismissStashConflicts => self.apply_stash(action),
            Action::Merge { .. }
            | Action::ContinueMerge
            | Action::MergeMessage(..)
            | Action::SubmitContinueMerge
            | Action::AbortMerge
            | Action::ConfirmAbortMerge
            | Action::Pick { .. }
            | Action::Mainline(..)
            | Action::SubmitMainline
            | Action::CloseMergeDialog
            | Action::DismissMergeError => self.apply_merge(action),
            Action::OpenRebase { .. }
            | Action::RebaseMove { .. }
            | Action::RebaseChoose(..)
            | Action::RebaseMessage(..)
            | Action::RebaseSelect(..)
            | Action::RunRebase
            | Action::ConfirmRebase
            | Action::CancelRebaseConfirm
            | Action::CloseRebase => self.apply_rebase(action),
            Action::OpenReset { .. }
            | Action::ResetMode(..)
            | Action::SubmitReset
            | Action::CloseReset => self.apply_reset(action),
            Action::OpenConflict(..)
            | Action::CloseConflict
            | Action::RestartConflict
            | Action::ConflictText(..)
            | Action::ChooseBlock(..)
            | Action::ChooseAll(..)
            | Action::ResolveConflict
            | Action::UseSide(..)
            | Action::DeleteConflicted
            | Action::ConfirmConflict
            | Action::CancelConflict => self.apply_conflict(action),
            Action::OpenFile { .. }
            | Action::CloseFile
            | Action::FileMode(..)
            | Action::FileFollow(..)
            | Action::FileMore
            | Action::FileRetry
            | Action::FileSelect(..) => self.apply_file(action),
        }
    }

    fn request_page(&mut self, page: PageRequest) {
        let generation = self.generation;
        let ticket = self.executor.submit("repo.log", page.params());
        self.pending.insert(ticket, Request::Log(generation, page));
        // The first page from the start is timed for the commit-graph notice.
        if page.skip == 0
            && let Some(repo) = self.current(generation)
        {
            repo.optimize
                .log_submitted(ticket, std::time::Instant::now());
        }
    }

    /// Selects a ref's commit; if it is not loaded yet asks for the next pages
    /// until it arrives (up to [`list::SEEK_LIMIT`] commits).
    fn seek(&mut self, id: Oid) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        match repo.list.seek(id) {
            Seek::Selected => repo.select_worktree(false),
            Seek::Loading(Some(page)) => self.request_page(page),
            Seek::Loading(None) => {}
            Seek::Missing => {
                self.error = tr!("app-commit-not-loaded", id = id.short());
            }
        }
    }

    fn request_diff(&mut self, target: &DiffTarget, seq: u64) {
        let (command, params) = target.request();
        self.submit(command, params, Request::Diff(self.generation, seq));
    }

    /// The loaded diff with its selection, if it allows partial selection.
    fn selectable_diff(&mut self) -> Option<(&DiffTarget, &DiffDoc, &mut lines::LineSelection)> {
        let (target, doc, selection) = self.repo.as_mut()?.diff.loaded_mut()?;
        matches!(
            lines::partial(doc, target.source),
            lines::Partial::Available(_)
        )
        .then_some((target, doc, selection))
    }

    /// After a new status, the open worktree or index diff is requested again; if the file
    /// moved entirely to the other list that diff opens, if it has no more changes it closes.
    fn follow_diff(&mut self) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        let Some(target) = repo.diff.target().cloned() else {
            return;
        };
        let side = match target.source {
            DiffSource::Worktree => Side::Unstaged,
            DiffSource::Index => Side::Staged,
            DiffSource::Commit(_) => return,
        };
        let request = match stage::follow(&repo.stage, side, &target.path) {
            Follow::Reload => repo.diff.reload().map(|seq| (seq, target)),
            Follow::Switch(other) => {
                let target = DiffTarget {
                    source: side_source(other),
                    path: target.path,
                };
                repo.diff.open(target.clone()).map(|seq| (seq, target))
            }
            Follow::Close => {
                repo.diff.close();
                None
            }
        };
        if let Some((seq, target)) = request {
            self.request_diff(&target, seq);
        }
    }

    /// Closes the diff if it is no longer of what is selected: a commit's diff holds while
    /// that commit is selected, worktree and index ones while the changes row
    /// is selected.
    fn close_stale_diff(&mut self) {
        if let Some(repo) = &mut self.repo {
            // The history's diff belongs to the file view, not to the selection.
            if repo.file.view().is_some() {
                return;
            }
            let selected = match repo.stash.selected() {
                Some(stash) => Some(stash.id),
                None => repo.list.selected_commit().map(|c| c.id),
            };
            let stale = repo.diff.target().is_some_and(|t| match t.source {
                DiffSource::Commit(id) => repo.worktree_selected || Some(id) != selected,
                DiffSource::Worktree | DiffSource::Index => !repo.worktree_selected,
            });
            if stale {
                repo.diff.close();
            }
            // The conflict editor holds while the changes row is selected.
            if !repo.worktree_selected {
                repo.conflict.close();
            }
        }
    }

    /// Asks for the selected commit's details, if it changed and they are not there already.
    fn request_details(&mut self) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        // A selected stash is shown as its commit: the files relative to the base.
        let selected = match repo.stash.selected() {
            Some(stash) => Some(stash.id),
            None => repo.list.selected_commit().map(|c| c.id),
        }
        .filter(|_| !repo.worktree_selected);
        if let Some(id) = repo.details.want(selected) {
            let request = Request::Show(self.generation, id);
            self.submit("commit.show", json!({ "id": id.to_string() }), request);
        }
    }

    /// List key actions: arrows, Page up/down, Home/End. With the diff open,
    /// Esc closes it and the arrows stay with the diff view. Only if no text field is
    /// receiving the keyboard.
    fn keyboard(&self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(repo) = &self.repo else {
            return;
        };
        // With the confirmation open the keys go only to it.
        if ctx.egui_wants_keyboard_input() || repo.modal_open() {
            return;
        }
        // With the conflict editor open the list is not in view.
        if repo.conflict.editor().is_some() {
            return;
        }
        // Blame and history: Esc closes the history's diff, then the view.
        if repo.file.view().is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                actions.push(if repo.diff.target().is_some() {
                    Action::CloseDiff
                } else {
                    Action::CloseFile
                });
            }
            return;
        }
        if repo.diff.target().is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                // First deselect, then close.
                if repo.diff.selection().is_some_and(|s| !s.is_empty()) {
                    actions.push(Action::ClearLines);
                } else {
                    actions.push(Action::CloseDiff);
                }
            }
            return;
        }
        let page = isize::try_from(self.viewport.rows.len().saturating_sub(2).max(1))
            .unwrap_or(isize::MAX);
        ctx.input(|i| {
            let keys = [
                (egui::Key::ArrowDown, Action::Move(1)),
                (egui::Key::ArrowUp, Action::Move(-1)),
                (egui::Key::PageDown, Action::Move(page)),
                (egui::Key::PageUp, Action::Move(-page)),
                (egui::Key::Home, Action::Edge(true)),
                (egui::Key::End, Action::Edge(false)),
            ];
            for (key, action) in keys {
                for _ in 0..i.num_presses(key) {
                    actions.push(action.clone());
                }
            }
        });
    }

    /// Asks for refs, status and log of the open repo.
    fn load(&mut self) {
        if self.repo.is_none() {
            return;
        }
        let generation = self.generation;
        self.submit("repo.refs", json!({}), Request::Refs(generation));
        self.submit("repo.status", json!({}), Request::Status(generation));
        self.submit("edit.history", json!({}), Request::History(generation));
        self.request_remotes(generation);
        self.submit(stash::LIST, json!({}), Request::Stashes(generation));
        if let Some(repo) = &mut self.repo {
            let first = repo.list.reset();
            self.request_page(first);
        }
    }

    /// Asks for `repo.operation` if the status that arrived says there is an operation in progress and for
    /// that status it has not been asked yet (one at a time).
    fn request_operation(&mut self) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        let Some(operation) = repo.status.as_ref().map(|s| s.operation) else {
            return;
        };
        if repo.merge.want_info(operation, repo.status_seq) {
            self.submit(merge::OPERATION, json!({}), Request::Operation(generation));
        }
    }

    /// Re-reads the remotes list (how many remote branches, which is the default).
    fn request_remotes(&mut self, generation: u64) {
        self.submit(remotes::LIST, json!({}), Request::Remotes(generation));
    }

    fn receive(&mut self) {
        for done in self.executor.poll() {
            let Some(request) = self.pending.remove(&done.ticket) else {
                continue;
            };
            if matches!(request, Request::Clone) {
                self.clone_done = Some(done);
                continue;
            }
            if matches!(request, Request::Open) {
                let path = self.requested.take();
                if done.error_kind == Some(ErrorKind::RepoNotFound) {
                    self.not_found = path;
                }
            }
            // Details have their own state, errors included: the panel shows them.
            if let Request::Show(generation, id) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<CommitDetails>(value).map_err(|e| {
                        tr!(
                            "common-unexpected-result",
                            command = &done.command,
                            error = e.to_string()
                        )
                    })
                });
                if let Some(repo) = self.current(generation) {
                    repo.details.receive(id, result);
                }
                continue;
            }
            // Writes: the new status, or the error in the stage area.
            if let Request::Write(generation) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<WriteResult>(value).map_err(|e| {
                        tr!(
                            "common-unexpected-result",
                            command = &done.command,
                            error = e.to_string()
                        )
                    })
                });
                let Some(repo) = self.current(generation) else {
                    continue;
                };
                let label = repo
                    .write_label
                    .take()
                    .unwrap_or_else(|| done.command.clone());
                match result {
                    Ok(WriteResult { status, backup }) => {
                        repo.set_status(status);
                        self.error.clear();
                        // Staged versions that existed only in the index: saved before
                        // writing, this is said in the status bar.
                        self.notice = backup
                            .map(|r| tr!("app-index-backup", backup = r))
                            .unwrap_or_default();
                        self.follow_diff();
                    }
                    Err(e) => {
                        log::warn!("{}: {e}", done.command);
                        let stale = done.error_kind == Some(ErrorKind::StaleDiff);
                        let message = if stale {
                            tr!("app-write-stale", label = &label)
                        } else {
                            tr!("common-failed", label = &label, error = &e)
                        };
                        repo.write_error = Some(message.clone());
                        // The diff seen is no longer the current one: status and diff are re-read
                        // (the diff follows the status) and the selection is cleared.
                        if stale && let Some((_, _, selection)) = repo.diff.loaded_mut() {
                            selection.clear();
                        }
                        self.error = message;
                        self.notice.clear();
                        if stale {
                            self.submit("repo.status", json!({}), Request::Status(generation));
                        }
                    }
                }
                continue;
            }
            if let Request::Commit(generation) = request {
                self.receive_commit(generation, done);
                continue;
            }
            if let Request::Discard(generation) = request {
                self.receive_discard(generation, done);
                continue;
            }
            if let Request::Undo(generation) = request {
                self.receive_undo(generation, done);
                continue;
            }
            if let Request::ResetPreview(generation, seq) = request {
                self.receive_reset_preview(generation, seq, done);
                continue;
            }
            if let Request::Reset(generation) = request {
                self.receive_reset(generation, done);
                continue;
            }
            if let Request::Branch(generation, op) = request {
                self.receive_branch(generation, op, done);
                continue;
            }
            if let Request::Fetch(generation) = request {
                self.receive_fetch(generation, done);
                continue;
            }
            if let Request::Sync(generation) = request {
                self.receive_sync(generation, done);
                continue;
            }
            if let Request::Optimize(generation) = request {
                self.receive_optimize(generation, done);
                continue;
            }
            if let Request::Log(generation, _) = request
                && let Some(repo) = self.current(generation)
            {
                let now = std::time::Instant::now();
                repo.optimize
                    .log_received(done.ticket, now, done.result.is_ok());
            }
            // The remotes list has its own error, shown in its section.
            if let Request::Remotes(generation) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.remotes.receive_list(done.result);
                }
                continue;
            }
            if let Request::Remote(generation, op) = request {
                self.receive_remote(generation, op, done);
                continue;
            }
            // The stash list also has its own error, in its section.
            if let Request::Stashes(generation) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.stash.receive_list(done.result);
                    repo.update_labels();
                }
                continue;
            }
            if let Request::Stash(generation, op) = request {
                self.receive_stash(generation, op, done);
                continue;
            }
            // The operation in progress also has its own error, in the panel.
            if let Request::Operation(generation) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.merge.receive_info(done.result);
                }
                continue;
            }
            if let Request::Merge(generation, op) = request {
                self.receive_merge(generation, op, done);
                continue;
            }
            // The plan's errors are shown in the editor.
            if let Request::RebasePlan(generation) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.rebase.receive_plan(done.result);
                }
                continue;
            }
            if let Request::Rebase(generation) = request {
                self.receive_rebase(generation, done);
                continue;
            }
            // The conflict editor shows read errors by itself.
            if let Request::ConflictShow(generation, seq) = request {
                if let Some(repo) = self.current(generation) {
                    if let Err(e) = &done.result {
                        log::warn!("{}: {e}", done.command);
                    }
                    repo.conflict.receive_show(seq, done.result);
                }
                continue;
            }
            if let Request::ConflictResolve(generation) = request {
                self.receive_conflict(generation, done);
                continue;
            }
            // Blame and history show their errors in their view.
            if let Request::File(generation, seq) = request {
                self.receive_file(generation, seq, done);
                continue;
            }
            // The diff too: errors in its view, old answers discarded.
            if let Request::Diff(generation, seq) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<Diff>(value).map_err(|e| {
                        tr!(
                            "common-unexpected-result",
                            command = &done.command,
                            error = e.to_string()
                        )
                    })
                });
                if let Some(repo) = self.current(generation) {
                    repo.diff.receive(seq, result);
                }
                continue;
            }
            // A page that did not arrive can be asked for again.
            if done.result.is_err()
                && let Request::Log(generation, page) = request
                && let Some(repo) = self.current(generation)
            {
                repo.list.failed(page);
            }
            match done.result {
                Ok(value) => {
                    if let Err(e) = self.store(request, value) {
                        log::error!("{}: unexpected result: {e}", done.command);
                        self.error = tr!(
                            "common-unexpected-result",
                            command = &done.command,
                            error = e.to_string()
                        );
                    } else {
                        self.error.clear();
                    }
                }
                Err(e) => {
                    log::warn!("{}: {e}", done.command);
                    self.error = e;
                }
            }
        }
    }

    /// The repo shown, if it is still the one of generation `generation`.
    fn current(&mut self, generation: u64) -> Option<&mut RepoView> {
        self.repo.as_mut().filter(|_| generation == self.generation)
    }

    /// Puts the result of `request` into the view.
    fn store(&mut self, request: Request, value: Value) -> Result<(), serde_json::Error> {
        match request {
            Request::Open => {
                let Opened {
                    path,
                    head,
                    commit_graph,
                } = serde_json::from_value(value)?;
                self.generation += 1;
                self.notice.clear();
                self.opened = Some(path.clone());
                let mut repo = RepoView::new(path, head);
                repo.optimize = OptimizeBox::new(commit_graph);
                self.repo = Some(repo);
                self.load();
            }
            Request::Refs(generation) => {
                if let Some(repo) = self.current(generation) {
                    repo.refs = serde_json::from_value(value)?;
                    repo.update_labels();
                }
            }
            Request::Status(generation) | Request::AutoStatus(generation) => {
                if let Some(repo) = self.current(generation) {
                    let status: Status = serde_json::from_value(value)?;
                    repo.set_status(status);
                    self.follow_diff();
                    if matches!(request, Request::AutoStatus(_)) {
                        self.after_auto_status(generation);
                    }
                }
            }
            Request::AutoRefs(generation) => {
                self.receive_auto_refs(generation, serde_json::from_value(value)?);
            }
            Request::Log(generation, page) => {
                if let Some(repo) = self.current(generation) {
                    let LogPage { commits, has_more } = serde_json::from_value(value)?;
                    if !repo.list.receive(page, commits, has_more) {
                        return Ok(());
                    }
                    if page.skip == 0
                        && let Some(id) = repo.select_after_load.take()
                        && !repo.worktree_selected
                    {
                        repo.list.select_id(id);
                    }
                    // A clicked ref whose commit was not loaded yet: look in the
                    // page that arrived and, if it is not there, ask for the next one.
                    if let Some(id) = repo.list.seeking() {
                        self.seek(id);
                    }
                }
            }
            Request::HeadMessage(generation) => {
                if let Some(repo) = self.current(generation) {
                    let details: CommitDetails = serde_json::from_value(value)?;
                    repo.commit.draft.propose(&details.commit);
                }
            }
            Request::History(generation) => {
                if let Some(repo) = self.current(generation) {
                    repo.history = serde_json::from_value(value)?;
                }
            }
            // Handled separately in `receive`.
            Request::Clone
            | Request::Show(..)
            | Request::Diff(..)
            | Request::Write(..)
            | Request::Commit(..)
            | Request::Discard(..)
            | Request::Undo(..)
            | Request::Branch(..)
            | Request::Fetch(..)
            | Request::Optimize(..)
            | Request::Sync(..)
            | Request::Remotes(..)
            | Request::Remote(..)
            | Request::Stashes(..)
            | Request::Stash(..)
            | Request::Operation(..)
            | Request::Merge(..)
            | Request::RebasePlan(..)
            | Request::Rebase(..)
            | Request::ResetPreview(..)
            | Request::Reset(..)
            | Request::ConflictShow(..)
            | Request::ConflictResolve(..)
            | Request::File(..) => {}
        }
        Ok(())
    }
}

impl Tab {
    /// The fetch, pull or push in progress with its latest progress, for the bar.
    fn network(&self) -> Option<view::Network> {
        let r = self.repo.as_ref()?;
        if let Some(running) = r.optimize.running() {
            return Some(view::Network {
                what: tr!("optimize-label"),
                cancelling: running.cancelling,
                progress: self.executor.progress(running.ticket),
                stop_hint: tr!("optimize-stop-hint"),
                cancel: Action::CancelOptimize,
            });
        }
        if let Some(running) = r.fetch.running() {
            return Some(view::Network {
                what: fetch::label(running.remote.as_deref()),
                cancelling: running.cancelling,
                progress: self.executor.progress(running.ticket),
                stop_hint: tr!("app-stop-fetch-hint"),
                cancel: Action::CancelFetch,
            });
        }
        if let Some(adding) = r.remotes.running_fetch() {
            return Some(view::Network {
                what: adding.label(),
                cancelling: adding.cancelling,
                progress: adding.ticket.and_then(|t| self.executor.progress(t)),
                stop_hint: tr!("app-stop-add-remote-hint"),
                cancel: Action::CancelAddRemote,
            });
        }
        let running = r.sync.running()?;
        Some(view::Network {
            what: running.request.label(),
            cancelling: running.cancelling,
            progress: self.executor.progress(running.ticket),
            stop_hint: match running.request.op {
                sync::Op::Pull => tr!("app-stop-pull-hint"),
                sync::Op::Push => tr!("app-stop-push-hint"),
            },
            cancel: Action::CancelSync,
        })
    }

    /// Draws the tab shown; what the user asks for ends up in `actions`, applied afterwards by
    /// [`App`]. `recent`: the recent repos, for the menus.
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        can_pick: bool,
        recent: &[view::RecentRepo],
    ) -> Vec<Action> {
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
        let writing = self.executor.writing().is_some();
        // The message draft is written directly (UI state, not repo state): it is
        // lent to the panel for this frame and then put back.
        let mut draft = self
            .repo
            .as_mut()
            .map(|r| std::mem::take(&mut r.commit.draft))
            .unwrap_or_default();

        let network = self.network();
        if let Some(repo) = &self.repo {
            egui::Panel::top("bar").show(ui, |ui| {
                view::top_bar(ui, repo, writing, network, &mut actions);
            });
        }
        egui::Panel::bottom("status").show(ui, |ui| {
            let write = self.repo.as_ref().and_then(|r| r.write_label.as_deref());
            view::status_bar(
                ui,
                self.repo.as_ref(),
                self.executor.running(),
                write,
                &self.error,
                &self.notice,
            );
        });
        // The operation in progress (merge, rebase, ...) under the bar, across the full width.
        if let Some(repo) = &self.repo
            && let Some(operation) = repo.status.as_ref().and_then(|s| s.operation)
        {
            let can_write = !writing && !repo.modal_open() && !repo.merge.busy();
            egui::Panel::top("operation").show(ui, |ui| {
                merge_view::banner(ui, repo, operation, can_write, &mut actions);
            });
        }
        // The commit-graph notice (S36), under the bar too.
        if let Some(repo) = &self.repo
            && repo.optimize.shown()
        {
            let can_write = !writing && !repo.modal_open();
            egui::Panel::top("optimize").show(ui, |ui| {
                view::optimize_notice(ui, &repo.optimize, can_write, &mut actions);
            });
        }
        // Side panels do not exceed a third of the window: the content wraps.
        let side_max = (ui.available_width() / 3.0).max(200.0);
        match &self.repo {
            Some(repo) => {
                egui::Panel::left("ref")
                    .resizable(true)
                    .default_size(220.0)
                    .min_size(140.0)
                    .max_size(side_max)
                    .show(ui, |ui| view::refs_panel(ui, repo, writing, &mut actions));
                egui::Panel::right("details")
                    .resizable(true)
                    .default_size(320.0)
                    .min_size(200.0)
                    .max_size(side_max)
                    .show(ui, |ui| {
                        if repo.worktree_selected {
                            view::stage_panel(ui, repo, &mut draft, writing, &mut actions);
                        } else if let Some(stash) = repo.stash.selected() {
                            view::stash_panel(ui, repo, stash, writing, &mut actions);
                        } else {
                            view::details_panel(
                                ui,
                                repo.list.selected_commit(),
                                repo.details.shown(),
                                repo.diff.target(),
                                &mut actions,
                            );
                        }
                    });
                if let Some(write) = &repo.confirm {
                    view::confirm_write(&ctx, write, &mut actions);
                }
                if let Some(pending) = repo.discard.confirm() {
                    view::confirm_discard(&ctx, pending, &mut actions);
                }
                if let Some(failure) = repo.fetch.failure() {
                    view::failure_window(
                        &ctx,
                        "fetch-error",
                        failure,
                        Action::DismissFetchError,
                        &mut actions,
                    );
                }
                if let Some(failure) = repo.remotes.failure() {
                    view::failure_window(
                        &ctx,
                        "remote-error",
                        failure,
                        Action::DismissRemoteError,
                        &mut actions,
                    );
                }
                if let Some(dialog) = repo.remotes.dialog() {
                    let progress = repo
                        .remotes
                        .running_fetch()
                        .and_then(|d| d.ticket)
                        .and_then(|t| self.executor.progress(t));
                    view::remote_dialog(&ctx, dialog, progress.as_ref(), &mut actions);
                }
                if let Some(dialog) = repo.stash.dialog() {
                    view::stash_dialog(&ctx, repo, dialog, &mut actions);
                }
                if let Some(conflicts) = repo.stash.conflicts() {
                    view::stash_conflicts_window(&ctx, conflicts, &mut actions);
                }
                if let Some(window) = repo.sync.window() {
                    let can_write = !writing && !repo.modal_open();
                    view::sync_window(&ctx, window, writing, can_write, &mut actions);
                }
                if let Some(dialog) = repo.branch.dialog() {
                    view::branch_dialog(&ctx, dialog, &mut actions);
                }
                if let Some(dialog) = repo.merge.dialog() {
                    merge_view::merge_dialog(&ctx, repo, dialog, &mut actions);
                }
                if let Some(editor) = repo.rebase.editor() {
                    view::rebase_editor(&ctx, editor, &mut actions);
                }
                if let Some(dialog) = repo.reset.dialog() {
                    view::reset_dialog(&ctx, dialog, &mut actions);
                }
                if let Some(failure) = repo.merge.failure() {
                    view::failure_window(
                        &ctx,
                        "merge-error",
                        failure,
                        Action::DismissMergeError,
                        &mut actions,
                    );
                }
                if let Some(confirm) = repo.commit.confirm() {
                    let branch = match &repo.head {
                        Head::Branch { name, .. } => Some(format!("refs/heads/{name}")),
                        Head::Detached(_) => None,
                    };
                    let left_behind =
                        commit::refs_left_behind(&repo.refs, repo.head.target(), branch.as_deref());
                    view::confirm_amend(&ctx, confirm, &left_behind, &mut actions);
                }
                let viewport = &mut self.viewport;
                let incoming = repo
                    .merge
                    .info()
                    .and_then(|i| i.incoming.first())
                    .map(|i| i.label.as_str());
                let conflict_write = !writing && !repo.modal_open();
                egui::CentralPanel::default().show(ui, |ui| match repo.diff.pane() {
                    _ if repo.conflict.editor().is_some() => {
                        if let Some(editor) = repo.conflict.editor() {
                            view::conflict_editor(
                                ui,
                                editor,
                                incoming,
                                conflict_write,
                                &mut actions,
                            );
                        }
                    }
                    _ if repo.file.view().is_some() => {
                        if let Some(file) = repo.file.view() {
                            let can_write = !writing && !repo.modal_open();
                            view::file_view(ui, file, repo.diff.pane(), can_write, &mut actions);
                        }
                    }
                    Pane::Closed => view::commit_list(ui, repo, viewport, writing, &mut actions),
                    pane => {
                        let lines = diff_view::LinesUi {
                            selection: repo.diff.selection(),
                            can_write: !writing && !repo.modal_open(),
                            back: true,
                        };
                        diff_view::diff_panel(ui, pane, lines, &mut actions);
                    }
                });
            }
            None => {
                egui::CentralPanel::default().show(ui, |ui| {
                    let busy = self.executor.is_busy();
                    view::welcome(ui, can_pick, busy, recent, &mut actions);
                });
            }
        }

        // The list brought the requested row into view: no longer needed.
        if let Some(repo) = &mut self.repo {
            repo.commit.draft = draft;
            repo.list.take_scroll_to();
            repo.scroll_header = false;
        }
        actions
    }

    /// After the actions of the frame: the selection just changed is requested in this same
    /// frame.
    fn after_actions(&mut self) {
        self.close_stale_diff();
        self.request_details();
        self.request_file();
    }
}

/// The diff of a file of the `side` list.
fn side_source(side: Side) -> DiffSource {
    match side {
        Side::Staged => DiffSource::Index,
        Side::Unstaged => DiffSource::Worktree,
    }
}

/// `text` with the first letter lowercased, to put it inside a sentence.
fn lowercase_first(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}
