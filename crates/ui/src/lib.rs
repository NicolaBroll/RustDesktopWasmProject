//! L6 — egui interface of the desktop app.
//!
//! Rule: graphical callbacks do not modify the repository directly, they produce
//! an `Action`; commands go through the [`Executor`], which runs them with
//! [`Registry::run`](project_engine::Registry::run) off the UI thread.
//!
//! Layout (`impl-docs/ARCHITECTURE-git.md` §8): one tab per open repo (`tabs`), top bar, refs on the left, commits in the
//! center, details of the selected commit on the right. At the top of the list the "Uncommitted
//! changes" row opens the stage area on the right. A file's diff opens in place of the graph,
//! with the side panel next to it to switch between files. Under the stage area lists there is
//! the commit message box.
//! With an operation in progress (merge, rebase, ...) its panel sits under the bar
//! (`merge_view`), with the conflicted files and the commands to finish or abort it.

mod auto_fetch;
mod branch;
mod clean;
mod clone;
mod commit;
mod compare;
mod conflict;
mod credentials;
mod detach;
mod details;
mod diff;
mod diff_view;
mod discard;
mod executor;
mod fetch;
mod file;
mod format;
mod graph;
mod icons;
mod ignore;
mod keys;
mod lfs;
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
mod running;
mod search;
mod settings;
mod side_panel;
mod stage;
mod stash;
mod submodule;
mod sync;
mod tabs;
mod theme;
mod tracking;
mod view;
mod word_diff;
mod worktree;

pub use credentials::{CredentialBox, CredentialDialog};
pub use executor::{Done, ErrorKind, Executor, Job, Ticket, Wake};

use branch::BranchBox;
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
use project_domain::HEADS_PREFIX;
use project_domain::{Commit, CommitDetails, Diff, Head, Oid, RefInfo, Status};
use project_engine::{
    CommitShowParams, IndexWriteResult, RepoLogResult, RepoOpenParams, RepoOpenResult,
};

use project_i18n::tr;
use rebase::RebaseBox;
use remotes::RemotesBox;
use serde_json::{Value, json};
use stage::{Follow, PendingWrite, Side, StageLists, StageOp};
use stash::StashBox;
use std::collections::HashMap;

use sync::SyncBox;
pub use tabs::App;
pub use view::WindowFrame;

/// What the user asked for during the frame: the list selection here, the rest grouped by
/// area (`branch::Action`, `stash::Action`, ...), each applied by its `apply_*` function.
#[derive(Debug, Clone)]
enum Action {
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
    /// "Retry" in the notice of the automatic fetch suspended by an authentication error.
    RetryAutoFetch,
    /// Tabs, folder dialog and recent list: applied by [`App`].
    Tabs(tabs::Action),
    /// The "Clone…" and "New repository…" dialog: applied by [`App`].
    Clone(clone::Action),
    /// The "Settings…" window and the settings: applied by [`App`].
    Settings(settings::Action),
    /// The credential modal (S52): applied by [`App`].
    Credentials(credentials::Action),
    /// Opening and closing a file's diff.
    Diff(diff::Action),
    /// Stage and unstage of whole files.
    Stage(stage::Action),
    /// Discarding changes, undo and redo.
    Discard(discard::Action),
    /// Deleting untracked files.
    Clean(clean::Action),
    /// Ignoring untracked files (`.gitignore`, `.git/info/exclude`).
    Ignore(ignore::Action),
    /// Selecting diff rows and staging hunks or lines.
    Lines(lines::Action),
    /// The commit message box.
    Commit(commit::Action),
    /// Branches and tags: dialogs, checkout, deletion.
    Branch(branch::Action),
    /// Fetch.
    Fetch(fetch::Action),
    /// The commit-graph notice and `repo.optimize`.
    Optimize(optimize::Action),
    /// Pull and push.
    Sync(sync::Action),
    /// Adding and removing remotes, deleting remote branches and tags.
    Remotes(remotes::Action),
    /// Stashes.
    Stash(stash::Action),
    /// Updating submodules.
    Submodule(submodule::Action),
    /// Linked worktrees: the "New worktree" dialog and the removal.
    Worktree(worktree::Action),
    /// The notice "git-lfs is not installed".
    Lfs(lfs::Action),
    /// Merge, cherry-pick and revert.
    Merge(merge::Action),
    /// The interactive rebase editor.
    Rebase(rebase::Action),
    /// The conflict editor.
    Conflict(conflict::Action),
    /// The reset dialog.
    Reset(reset::Action),
    /// Blame and history of a file.
    File(file::Action),
    /// The search in the history.
    Search(search::Action),
    /// Comparing two revisions in the details panel.
    Compare(compare::Action),
}

/// A step of dragging over the diff rows.
#[derive(Debug, Clone, Copy)]
enum Drag {
    Start(usize),
    Over(usize),
    End,
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
    /// `worktree.clean`, without or with `confirm` (the phase is in [`clean::CleanBox`]).
    Clean(u64),
    /// `worktree.ignore` (the pattern sent is in [`ignore::IgnoreBox`]).
    Ignore(u64),
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
    /// `submodule.list`.
    Submodules(u64, bool),
    /// `submodule.update` (the state is in [`submodule::SubmoduleBox`]).
    Submodule(u64),
    /// `worktree.list`.
    Worktrees(u64),
    /// `worktree.add`/`worktree.remove` (the state is in [`worktree::WorktreeBox`]).
    Worktree(u64),
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
    /// `repo.search` with the request number of [`search::SearchBox`].
    Search(u64, u64),
    /// `diff.range` of the comparison with the request number of [`compare::CompareBox`].
    Compare(u64, u64),
    /// `repo.lfs_status` (the state is in [`lfs::LfsBox`]).
    LfsStatus(u64),
    /// `repo.lfs_paths` (the paths sent are in [`lfs::LfsBox`]).
    LfsPaths(u64),
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
    /// Two revisions compared in the details panel (`diff.range`).
    compare: compare::CompareBox,
    /// The commit message box: draft, commit in progress, confirmation, error.
    commit: CommitBox,
    /// Discarding changes: check, confirmation, submission.
    discard: DiscardBox,
    /// Deleting untracked files: check, confirmation, submission.
    clean: clean::CleanBox,
    /// Ignoring an untracked file: the pattern sent.
    ignore: ignore::IgnoreBox,
    /// Branch commands: name dialogs, deletion confirmation, checkout.
    branch: BranchBox,
    /// The fetch in progress and the last error to explain.
    fetch: FetchBox,
    /// The timer of the automatic fetch (S50).
    auto_fetch: auto_fetch::AutoFetch,
    /// The commit-graph notice and `repo.optimize` in progress.
    optimize: OptimizeBox,
    /// The pull or push in progress and the open window (conflict, rejection, force confirmation).
    sync: SyncBox,
    /// The configured remotes (`remote.list`) and the dialogs to add and remove them.
    remotes: RemotesBox,
    /// The stashes (`stash.list`), the selected stash, the push and drop dialogs.
    stash: StashBox,
    /// The submodules (`submodule.list`) and the update in progress.
    submodules: submodule::SubmoduleBox,
    /// The linked worktrees (`worktree.list`), the "New worktree" dialog and the removal.
    worktrees: worktree::WorktreeBox,
    /// Git LFS: the files stored in it and the notice "git-lfs is not installed" (S63).
    lfs: lfs::LfsBox,
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
    /// The search in the history: field, matches, next/previous.
    search: search::SearchBox,
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
            compare: compare::CompareBox::default(),
            commit: CommitBox::default(),
            discard: DiscardBox::default(),
            clean: clean::CleanBox::default(),
            ignore: ignore::IgnoreBox::default(),
            branch: BranchBox::default(),
            fetch: FetchBox::default(),
            auto_fetch: auto_fetch::AutoFetch::default(),
            optimize: OptimizeBox::default(),
            sync: SyncBox::default(),
            remotes: RemotesBox::default(),
            stash: StashBox::default(),
            submodules: submodule::SubmoduleBox::default(),
            worktrees: worktree::WorktreeBox::default(),
            lfs: lfs::LfsBox::default(),
            merge: MergeBox::default(),
            rebase: RebaseBox::default(),
            reset: reset::ResetBox::default(),
            conflict: ConflictBox::default(),
            file: file::FileBox::default(),
            search: search::SearchBox::default(),
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
            self.clean.cancel();
        }
        self.update_labels();
    }

    /// True if a confirmation is open (stage of conflicts, published amend, discard) or a
    /// branch dialog: while it is, no writes and the keys go only to it.
    fn modal_open(&self) -> bool {
        self.confirm.is_some()
            || self.commit.confirm().is_some()
            || self.discard.confirm().is_some()
            || self.clean.confirm().is_some()
            || self.branch.modal_open()
            || self.sync.modal_open()
            || self.remotes.modal_open()
            || self.stash.modal_open()
            || self.merge.modal_open()
            || self.rebase.modal_open()
            || self.reset.modal_open()
            || self.worktrees.modal_open()
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
    /// The worktree `worktree.add` just created: [`App`] opens it in a new tab.
    open_worktree: Option<String>,
    viewport: ListViewport,
    error: String,
    /// Informational message in the status bar (e.g. the saved index backup).
    notice: String,
    /// The watch of the repo and the refresh it asks for (S40).
    auto: refresh::AutoRefresh,
    /// The repos (paths) that already showed the notice "git-lfs is not installed" in this tab:
    /// once per repo per session (S63).
    lfs_noticed: std::collections::HashSet<String>,
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
            open_worktree: None,
            viewport: ListViewport::default(),
            error: String::new(),
            notice: String::new(),
            auto: refresh::AutoRefresh::default(),
            lfs_noticed: std::collections::HashSet::new(),
        }
    }

    /// Opens `path` in this tab, in the background.
    fn open(&mut self, path: String) {
        // A content search running would hold the session until it ends.
        self.stop_search();
        let params = params(RepoOpenParams { path: path.clone() });
        self.submit("repo.open", params, Request::Open);
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
                self.error = unexpected_result("repo.clone", e);
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

    fn submit(&mut self, command: &str, params: Value, request: Request) -> Ticket {
        let ticket = self.executor.submit(command, params);
        self.pending.insert(ticket, request);
        ticket
    }

    /// Applies an action of the frame: the list selection here, the rest in its area's
    /// `apply_*` function.
    fn apply(&mut self, action: Action) {
        // A result chosen in the list: closing the search keeps it.
        if matches!(
            action,
            Action::Select(_)
                | Action::SelectWorktree
                | Action::SelectCommit(_)
                | Action::Move(_)
                | Action::Edge(_)
        ) && let Some(repo) = &mut self.repo
        {
            repo.search.keep();
            // A plain selection goes back from the comparison to one commit.
            repo.compare.close();
        }
        match action {
            // The tabs, the folder dialog, clone, settings and credentials belong to `App`.
            Action::Tabs(_) | Action::Clone(_) | Action::Settings(_) | Action::Credentials(_) => {}
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
            Action::Visible(end) => {
                if let Some(repo) = &mut self.repo {
                    repo.search_visible(end);
                }
                if let Some(request) = self.repo.as_mut().and_then(|r| r.list.next_page(end)) {
                    self.request_page(request);
                }
            }
            Action::RetryAutoFetch => self.retry_auto_fetch(),
            Action::Diff(action) => self.apply_diff(action),
            Action::Stage(action) => self.apply_stage(action),
            Action::Discard(action) => self.apply_discard(action),
            Action::Clean(action) => self.apply_clean(action),
            Action::Ignore(action) => self.apply_ignore(action),
            Action::Lines(action) => self.apply_lines(action),
            Action::Commit(action) => self.apply_commit(action),
            Action::Branch(action) => self.apply_branch(action),
            Action::Fetch(action) => self.apply_fetch(action),
            Action::Optimize(action) => self.apply_optimize(action),
            Action::Submodule(action) => self.apply_submodule(action),
            Action::Worktree(action) => self.apply_worktree(action),
            Action::Lfs(action) => self.apply_lfs(action),
            Action::Sync(action) => self.apply_sync(action),
            Action::Remotes(action) => self.apply_remotes(action),
            Action::Stash(action) => self.apply_stash(action),
            Action::Merge(action) => self.apply_merge(action),
            Action::Rebase(action) => self.apply_rebase(action),
            Action::Reset(action) => self.apply_reset(action),
            Action::Conflict(action) => self.apply_conflict(action),
            Action::File(action) => self.apply_file(action),
            Action::Search(action) => self.apply_search(action),
            Action::Compare(action) => self.apply_compare(action),
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
            DiffSource::Commit(_) | DiffSource::Range { .. } => return,
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
                DiffSource::Range { .. } => !repo.compare.view().is_some_and(|c| c.owns(t)),
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
            let params = params(CommitShowParams { id: id.to_string() });
            self.submit("commit.show", params, request);
        }
    }

    /// The tab's shortcuts ([`keys`]) and the list keys: arrows, Page up/down, Home/End. With
    /// the diff open, Esc closes it and the arrows stay with the diff view. Only if no text field
    /// is receiving the keyboard (Ctrl+F also from a field); nothing with a menu or a
    /// confirmation open: the keys, Esc too, are theirs.
    fn keyboard(&self, ctx: &egui::Context, pressed: &keys::Pressed, actions: &mut Vec<Action>) {
        let Some(repo) = &self.repo else {
            return;
        };
        if pressed.popup || repo.modal_open() {
            return;
        }
        let list_in_view = repo.conflict.editor().is_none()
            && repo.file.view().is_none()
            && repo.diff.target().is_none();
        for &command in &pressed.commands {
            // Ctrl+F: the search field takes the keyboard.
            if command == keys::Command::Find {
                if list_in_view {
                    actions.push(Action::Search(search::Action::Open));
                }
            } else if let Some(action) = self.shortcut(repo, command) {
                actions.push(action);
            }
        }
        if ctx.egui_wants_keyboard_input() {
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
                    Action::Diff(diff::Action::Close)
                } else {
                    Action::File(file::Action::Close)
                });
            }
            return;
        }
        if repo.diff.target().is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                // First deselect, then close.
                if repo.diff.selection().is_some_and(|s| !s.is_empty()) {
                    actions.push(Action::Lines(lines::Action::Clear));
                } else {
                    actions.push(Action::Diff(diff::Action::Close));
                }
            }
            return;
        }
        // Esc goes back from a comparison to the selected commit, then closes the search (the
        // field without the keyboard).
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if repo.compare.view().is_some() {
                actions.push(Action::Compare(compare::Action::Close));
            } else if repo.search.is_open() {
                actions.push(Action::Search(search::Action::Close));
            }
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

    /// The action of a toolbar or stage panel shortcut, with the same gates as its button; `None`
    /// when the button would be disabled.
    fn shortcut(&self, repo: &RepoView, command: keys::Command) -> Option<Action> {
        use keys::Command;
        let writing = self.executor.writing().is_some();
        let network = !writing && self.network().is_none();
        let branch = view::current_branch(&repo.head).is_some();
        match command {
            Command::Undo | Command::Redo => {
                let op = if command == Command::Undo {
                    UndoOp::Undo
                } else {
                    UndoOp::Redo
                };
                view::undo_enabled(repo, op, writing)
                    .then_some(Action::Discard(discard::Action::Undo(op)))
            }
            Command::Refresh => (!writing).then_some(Action::Refresh),
            Command::Fetch => network.then_some(Action::Fetch(fetch::Action::Fetch(None))),
            Command::Pull => {
                (network && branch).then_some(Action::Sync(sync::Action::Pull(None, None)))
            }
            Command::Push => (network && branch).then_some(Action::Sync(sync::Action::Push(None))),
            Command::ToggleStage => {
                let target = repo.diff.target().filter(|_| !writing)?;
                let (op, list) = match target.source {
                    DiffSource::Worktree => (StageOp::Stage, &repo.stage.unstaged),
                    DiffSource::Index => (StageOp::Unstage, &repo.stage.staged),
                    DiffSource::Commit(_) | DiffSource::Range { .. } => return None,
                };
                list.iter()
                    .any(|item| item.path == target.path)
                    .then(|| Action::Stage(stage::Action::Write(op, vec![target.path.clone()])))
            }
            // Find is the tab's own (above), the others belong to `App`.
            Command::Find
            | Command::Settings
            | Command::NextTab
            | Command::PreviousTab
            | Command::ZoomIn
            | Command::ZoomOut
            | Command::ZoomReset => None,
        }
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
        self.request_submodules(generation, false);
        self.request_worktrees(generation);
        self.request_lfs_status(generation);
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
            if let Some(request) = self.pending.remove(&done.ticket) {
                self.receive_one(request, done);
            }
        }
    }

    /// The answer `done` to `request`: each area takes its own, the reads of the view are
    /// stored by [`Tab::store`].
    fn receive_one(&mut self, request: Request, done: Done) {
        // Submodules left behind by a checkout, pull, merge...: the notice (S62b).
        let moved = match request {
            Request::Branch(g, _)
            | Request::Merge(g, _)
            | Request::Stash(g, _)
            | Request::Sync(g)
            | Request::Rebase(g)
            | Request::Reset(g)
            | Request::Undo(g)
                if submodule::moves_submodules(&done.command) && done.result.is_ok() =>
            {
                Some(g)
            }
            _ => None,
        };
        if let Some(generation) = moved {
            self.check_submodules(generation);
        }
        self.receive_request(request, done);
        // A new status or diff: which of its files are stored in Git LFS (S63).
        self.follow_lfs();
    }

    fn receive_request(&mut self, request: Request, done: Done) {
        match request {
            Request::Clone => self.clone_done = Some(done),
            // Details have their own state, errors included: the panel shows them.
            Request::Show(generation, id) => {
                let result = done
                    .result
                    .and_then(|value| decode::<CommitDetails>(&done.command, value));
                if let Some(repo) = self.current(generation) {
                    repo.details.receive(id, result);
                }
            }
            Request::Write(generation) => self.receive_write(generation, done),
            Request::Commit(generation) => self.receive_commit(generation, done),
            Request::Discard(generation) => self.receive_discard(generation, done),
            Request::Clean(generation) => self.receive_clean(generation, done),
            Request::Ignore(generation) => self.receive_ignore(generation, done),
            Request::Undo(generation) => self.receive_undo(generation, done),
            Request::ResetPreview(generation, seq) => {
                self.receive_reset_preview(generation, seq, done);
            }
            Request::Reset(generation) => self.receive_reset(generation, done),
            Request::Branch(generation, op) => self.receive_branch(generation, op, done),
            Request::Fetch(generation) => self.receive_fetch(generation, done),
            Request::Sync(generation) => self.receive_sync(generation, done),
            Request::Optimize(generation) => self.receive_optimize(generation, done),
            // The remotes list has its own error, shown in its section.
            Request::Remotes(generation) => {
                log_failure(&done);
                if let Some(repo) = self.current(generation) {
                    repo.remotes.receive_list(done.result);
                }
            }
            Request::Remote(generation, op) => self.receive_remote(generation, op, done),
            // The stash list also has its own error, in its section.
            Request::Stashes(generation) => {
                log_failure(&done);
                if let Some(repo) = self.current(generation) {
                    repo.stash.receive_list(done.result);
                    repo.update_labels();
                }
            }
            Request::Stash(generation, op) => self.receive_stash(generation, op, done),
            // The submodule list also has its own error, in its section.
            Request::Submodules(generation, check) => {
                log_failure(&done);
                if let Some(repo) = self.current(generation) {
                    repo.submodules.receive_list(done.result, check);
                }
            }
            Request::Submodule(generation) => self.receive_submodule(generation, done),
            // The worktree list also has its own error, in its section.
            Request::Worktrees(generation) => {
                log_failure(&done);
                if let Some(repo) = self.current(generation) {
                    repo.worktrees.receive_list(done.result);
                }
            }
            Request::Worktree(generation) => self.receive_worktree(generation, done),
            // The operation in progress also has its own error, in the panel.
            Request::Operation(generation) => {
                log_failure(&done);
                if let Some(repo) = self.current(generation) {
                    repo.merge.receive_info(done.result);
                }
            }
            Request::Merge(generation, op) => self.receive_merge(generation, op, done),
            // The plan's errors are shown in the editor.
            Request::RebasePlan(generation) => {
                log_failure(&done);
                if let Some(repo) = self.current(generation) {
                    repo.rebase.receive_plan(done.result);
                }
            }
            Request::Rebase(generation) => self.receive_rebase(generation, done),
            // The conflict editor shows read errors by itself.
            Request::ConflictShow(generation, seq) => {
                log_failure(&done);
                if let Some(repo) = self.current(generation) {
                    repo.conflict.receive_show(seq, done.result);
                }
            }
            Request::ConflictResolve(generation) => self.receive_conflict(generation, done),
            // Blame and history show their errors in their view.
            Request::File(generation, seq) => self.receive_file(generation, seq, done),
            // The search shows its errors in its bar.
            Request::Search(generation, seq) => self.receive_search(generation, seq, done),
            // The comparison shows its errors in the details panel.
            Request::Compare(generation, seq) => self.receive_compare(generation, seq, done),
            Request::LfsStatus(_) | Request::LfsPaths(_) => self.receive_lfs(request, done),
            // The diff too: errors in its view, old answers discarded.
            Request::Diff(generation, seq) => {
                let result = done
                    .result
                    .and_then(|value| decode::<Diff>(&done.command, value));
                if let Some(repo) = self.current(generation) {
                    repo.diff.receive(seq, result);
                }
            }
            Request::Open
            | Request::Refs(_)
            | Request::Status(_)
            | Request::AutoStatus(_)
            | Request::AutoRefs(_)
            | Request::Log(..)
            | Request::HeadMessage(_)
            | Request::History(_) => self.receive_read(request, done),
        }
    }

    /// The reads of the view (open, status, log, refs...): stored, errors in the status bar.
    fn receive_read(&mut self, request: Request, done: Done) {
        match request {
            Request::Open => {
                let path = self.requested.take();
                if done.error_kind == Some(ErrorKind::RepoNotFound) {
                    self.not_found = path;
                }
            }
            Request::Log(generation, page) => {
                if let Some(repo) = self.current(generation) {
                    let now = std::time::Instant::now();
                    repo.optimize
                        .log_received(done.ticket, now, done.result.is_ok());
                    // A page that did not arrive can be asked for again.
                    if done.result.is_err() {
                        repo.list.failed(page);
                    }
                }
            }
            _ => {}
        }
        match done.result {
            Ok(value) => {
                if let Err(e) = self.store(request, value) {
                    log::error!("{}: unexpected result: {e}", done.command);
                    self.error = unexpected_result(&done.command, e);
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

    /// Writes (stage, unstage, lines): the new status, or the error in the stage area.
    fn receive_write(&mut self, generation: u64, done: Done) {
        let result = done
            .result
            .and_then(|value| decode::<IndexWriteResult>(&done.command, value));
        let Some(repo) = self.current(generation) else {
            return;
        };
        let label = repo
            .write_label
            .take()
            .unwrap_or_else(|| done.command.clone());
        match result {
            Ok(IndexWriteResult { status, backup }) => {
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
    }

    /// The repo shown, if it is still the one of generation `generation`.
    fn current(&mut self, generation: u64) -> Option<&mut RepoView> {
        self.repo.as_mut().filter(|_| generation == self.generation)
    }

    /// Puts the result of `request` into the view.
    fn store(&mut self, request: Request, value: Value) -> Result<(), serde_json::Error> {
        match request {
            Request::Open => {
                let RepoOpenResult {
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
                    // A checkout or a renamed branch changes what the worktrees show.
                    if repo.worktrees.shown() {
                        self.request_worktrees(generation);
                    }
                }
            }
            Request::Status(generation) | Request::AutoStatus(generation) => {
                if let Some(repo) = self.current(generation) {
                    let status: Status = serde_json::from_value(value)?;
                    let submodules = repo.submodules.wants_list(&status);
                    repo.set_status(status);
                    self.follow_diff();
                    if submodules {
                        self.request_submodules(generation, false);
                    }
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
                    let RepoLogResult { commits, has_more } = serde_json::from_value(value)?;
                    if !repo.list.receive(page, commits, has_more) {
                        return Ok(());
                    }
                    // The order of the list may have changed: the ids of the search too.
                    if page.skip == 0 {
                        repo.search.refresh();
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
                    let result: project_engine::HistoryResult = serde_json::from_value(value)?;
                    repo.history = result.journal.into();
                }
            }
            // Handled separately in `receive`.
            Request::Clone
            | Request::Show(..)
            | Request::Diff(..)
            | Request::Write(..)
            | Request::Commit(..)
            | Request::Discard(..)
            | Request::Clean(..)
            | Request::Ignore(..)
            | Request::Undo(..)
            | Request::Branch(..)
            | Request::Fetch(..)
            | Request::Optimize(..)
            | Request::Sync(..)
            | Request::Remotes(..)
            | Request::Remote(..)
            | Request::Stashes(..)
            | Request::Stash(..)
            | Request::Submodules(..)
            | Request::Submodule(..)
            | Request::Worktrees(..)
            | Request::Worktree(..)
            | Request::Operation(..)
            | Request::Merge(..)
            | Request::RebasePlan(..)
            | Request::Rebase(..)
            | Request::ResetPreview(..)
            | Request::Reset(..)
            | Request::ConflictShow(..)
            | Request::ConflictResolve(..)
            | Request::File(..)
            | Request::Search(..)
            | Request::Compare(..)
            | Request::LfsStatus(..)
            | Request::LfsPaths(..) => {}
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
                cancel: Action::Optimize(optimize::Action::Cancel),
            });
        }
        if let Some(running) = r.submodules.running() {
            return Some(view::Network {
                what: running.extra.clone(),
                cancelling: running.cancelling,
                progress: self.executor.progress(running.ticket),
                stop_hint: tr!("submodule-stop-hint"),
                cancel: Action::Submodule(submodule::Action::Cancel),
            });
        }
        if let Some(running) = r.fetch.running() {
            return Some(view::Network {
                what: if running.extra.auto {
                    tr!("fetch-auto-label")
                } else {
                    fetch::label(running.extra.remote.as_deref())
                },
                cancelling: running.cancelling,
                progress: self.executor.progress(running.ticket),
                stop_hint: tr!("app-stop-fetch-hint"),
                cancel: Action::Fetch(fetch::Action::Cancel),
            });
        }
        if let Some(adding) = r.remotes.running_fetch() {
            return Some(view::Network {
                what: adding.label(),
                cancelling: adding.running.is_some_and(|r| r.cancelling),
                progress: adding
                    .running
                    .and_then(|r| self.executor.progress(r.ticket)),
                stop_hint: tr!("app-stop-add-remote-hint"),
                cancel: Action::Remotes(remotes::Action::CancelAdd),
            });
        }
        let running = r.sync.running()?;
        Some(view::Network {
            what: running.extra.label(),
            cancelling: running.cancelling,
            progress: self.executor.progress(running.ticket),
            stop_hint: match running.extra.op {
                sync::Op::Pull => tr!("app-stop-pull-hint"),
                sync::Op::Push | sync::Op::PushTag => tr!("app-stop-push-hint"),
            },
            cancel: Action::Sync(sync::Action::Cancel),
        })
    }

    /// Draws the tab shown; what the user asks for ends up in `actions`, applied afterwards by
    /// [`App`]. `recent`: the recent repos, for the menus.
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        can_pick: bool,
        recent: &[view::RecentRepo],
        settings: &settings::Settings,
        pressed: &keys::Pressed,
    ) -> Vec<Action> {
        let ctx = ui.ctx().clone();
        let mut actions = Vec::new();
        self.keyboard(&ctx, pressed, &mut actions);
        let focus_search = self.repo.as_mut().is_some_and(|r| r.search.take_focus());
        let writing = self.executor.writing().is_some();
        // The message draft is written directly (UI state, not repo state): it is
        // lent to the panel for this frame and then put back.
        let mut draft = self
            .repo
            .as_mut()
            .map(|r| std::mem::take(&mut r.commit.draft))
            .unwrap_or_default();

        self.top_panels(ui, writing, &mut actions);
        match &self.repo {
            Some(repo) => {
                side_panels(ui, repo, &mut draft, writing, &mut actions);
                self.dialogs(&ctx, repo, writing, can_pick, &mut actions);
                let center = Center {
                    writing,
                    focus_search,
                    split: settings.diff_split,
                    dates: settings.dates,
                    ask_detach: !settings.skip_detach_confirm,
                };
                central_panel(ui, repo, &mut self.viewport, center, &mut actions);
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

    /// The panels across the full width: the bar, the status bar and, under the bar, the notices.
    fn top_panels(&self, ui: &mut egui::Ui, writing: bool, actions: &mut Vec<Action>) {
        let network = self.network();
        if let Some(repo) = &self.repo {
            egui::Panel::top("bar").show(ui, |ui| {
                view::top_bar(ui, repo, writing, network, actions);
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
                merge_view::banner(ui, repo, operation, can_write, actions);
            });
        }
        // The commit-graph notice (S36), under the bar too.
        if let Some(repo) = &self.repo
            && repo.optimize.shown()
        {
            let can_write = !writing && !repo.modal_open();
            egui::Panel::top("optimize").show(ui, |ui| {
                view::optimize_notice(ui, &repo.optimize, can_write, actions);
            });
        }
        // Submodules left on another commit by the last checkout, pull, merge... (S62b).
        if let Some(repo) = &self.repo
            && !repo.submodules.outdated().is_empty()
        {
            let can_write = !writing && !repo.modal_open() && repo.submodules.running().is_none();
            egui::Panel::top("submodule-outdated").show(ui, |ui| {
                view::outdated_notice(ui, repo.submodules.outdated(), can_write, actions);
            });
        }
        // The repo uses Git LFS but git-lfs is missing (S63): once per repo in this tab.
        if let Some(repo) = &self.repo
            && repo.lfs.notice()
        {
            egui::Panel::top("lfs-missing").show(ui, |ui| {
                view::lfs_notice(ui, actions);
            });
        }
        // The automatic fetch suspended by an authentication error (S50), under the bar too.
        if let Some(repo) = &self.repo
            && let Some(suspended) = repo.auto_fetch.suspended()
        {
            egui::Panel::top("auto-fetch").show(ui, |ui| {
                view::auto_fetch_notice(ui, suspended, actions);
            });
        }
    }

    /// The dialogs and windows of the repo, above the panels.
    fn dialogs(
        &self,
        ctx: &egui::Context,
        repo: &RepoView,
        writing: bool,
        can_pick: bool,
        actions: &mut Vec<Action>,
    ) {
        if let Some(write) = &repo.confirm {
            view::confirm_write(ctx, write, actions);
        }
        if let Some(pending) = repo.discard.confirm() {
            view::confirm_discard(ctx, pending, actions);
        }
        if let Some(pending) = repo.clean.confirm() {
            view::confirm_clean(ctx, pending, actions);
        }
        if let Some(failure) = repo.fetch.failure() {
            view::failure_window(
                ctx,
                "fetch-error",
                failure,
                Action::Fetch(fetch::Action::DismissError),
                actions,
            );
        }
        if let Some(failure) = repo.remotes.failure() {
            view::failure_window(
                ctx,
                "remote-error",
                failure,
                Action::Remotes(remotes::Action::DismissError),
                actions,
            );
        }
        if let Some(dialog) = repo.remotes.dialog() {
            let progress = repo
                .remotes
                .running_fetch()
                .and_then(|d| d.running)
                .and_then(|r| self.executor.progress(r.ticket));
            view::remote_dialog(ctx, dialog, progress.as_ref(), actions);
        }
        if let Some(dialog) = repo.stash.dialog() {
            view::stash_dialog(ctx, repo, dialog, actions);
        }
        if let Some(conflicts) = repo.stash.conflicts() {
            view::stash_conflicts_window(ctx, conflicts, actions);
        }
        if let Some(window) = repo.sync.window() {
            let can_write = !writing && !repo.modal_open();
            view::sync_window(ctx, window, writing, can_write, actions);
        }
        if let Some(dialog) = repo.branch.dialog() {
            view::branch_dialog(ctx, dialog, actions);
        }
        if let Some(dialog) = repo.merge.dialog() {
            merge_view::merge_dialog(ctx, repo, dialog, actions);
        }
        if let Some(editor) = repo.rebase.editor() {
            view::rebase_editor(ctx, editor, actions);
        }
        if let Some(dialog) = repo.reset.dialog() {
            view::reset_dialog(ctx, dialog, actions);
        }
        if let Some(dialog) = repo.worktrees.dialog() {
            view::worktree_dialog(ctx, dialog, repo, can_pick, actions);
        }
        if let Some(remove) = repo.worktrees.remove_dialog() {
            view::confirm_worktree_remove(ctx, remove, actions);
        }
        if let Some(failure) = repo.merge.failure() {
            view::failure_window(
                ctx,
                "merge-error",
                failure,
                Action::Merge(merge::Action::DismissError),
                actions,
            );
        }
        if let Some(confirm) = repo.commit.confirm() {
            let branch = match &repo.head {
                Head::Branch { name, .. } => Some(format!("{HEADS_PREFIX}{name}")),
                Head::Detached(_) => None,
            };
            let left_behind =
                commit::refs_left_behind(&repo.refs, repo.head.target(), branch.as_deref());
            view::confirm_amend(ctx, confirm, &left_behind, actions);
        }
    }

    /// After the actions of the frame: the selection just changed is requested in this same
    /// frame.
    fn after_actions(&mut self, ctx: &egui::Context) {
        self.close_stale_diff();
        self.request_details();
        self.request_compare();
        self.request_file();
        self.request_search(ctx);
    }
}

/// The refs on the left and, on the right, the stage area, the selected stash or the commit
/// details.
fn side_panels(
    ui: &mut egui::Ui,
    repo: &RepoView,
    draft: &mut commit::CommitDraft,
    writing: bool,
    actions: &mut Vec<Action>,
) {
    // The center keeps a share of the window: the side panels shrink first, the content wraps;
    // when the window grows again they get back the width chosen by the user.
    let available = ui.available_width();
    let refs = egui::Rangef::new(140.0, side_panel::max_width(available, 140.0));
    let details = egui::Rangef::new(200.0, side_panel::max_width(available, 200.0));
    side_panel::prepare(ui, "ref", 220.0, refs);
    side_panel::prepare(ui, "details", 320.0, details);
    egui::Panel::left("ref")
        .resizable(true)
        .default_size(220.0)
        .size_range(refs)
        .show(ui, |ui| view::refs_panel(ui, repo, writing, actions));
    egui::Panel::right("details")
        .resizable(true)
        .default_size(320.0)
        .size_range(details)
        .show(ui, |ui| {
            if repo.worktree_selected {
                view::stage_panel(ui, repo, draft, writing, actions);
            } else if let Some(stash) = repo.stash.selected() {
                view::stash_panel(ui, repo, stash, writing, actions);
            } else if let Some(comparison) = repo.compare.view() {
                view::compare_panel(ui, repo, comparison, repo.diff.target(), actions);
            } else {
                view::details_panel(
                    ui,
                    repo.list.selected_commit(),
                    repo.details.shown(),
                    repo.diff.target(),
                    actions,
                );
            }
        });
}

/// What the central panel needs besides the repo.
#[derive(Clone, Copy)]
struct Center {
    writing: bool,
    focus_search: bool,
    /// Diffs side by side.
    split: bool,
    /// The dates of the commit list.
    dates: settings::DateFormat,
    /// "Check out this commit" asks first.
    ask_detach: bool,
}

/// The center: conflict editor, blame and history, the open diff or the commit list.
fn central_panel(
    ui: &mut egui::Ui,
    repo: &RepoView,
    viewport: &mut ListViewport,
    center: Center,
    actions: &mut Vec<Action>,
) {
    let Center {
        writing,
        focus_search,
        split,
        dates,
        ask_detach,
    } = center;
    let incoming = repo
        .merge
        .info()
        .and_then(|i| i.incoming.first())
        .map(|i| i.label.as_str());
    let conflict_write = !writing && !repo.modal_open();
    egui::CentralPanel::default().show(ui, |ui| match repo.diff.pane() {
        _ if repo.conflict.editor().is_some() => {
            if let Some(editor) = repo.conflict.editor() {
                view::conflict_editor(ui, editor, incoming, conflict_write, actions);
            }
        }
        _ if repo.file.view().is_some() => {
            if let Some(file) = repo.file.view() {
                let can_write = !writing && !repo.modal_open();
                view::file_view(ui, file, repo.diff.pane(), can_write, split, actions);
            }
        }
        Pane::Closed => {
            let list = view::ListOptions {
                writing,
                focus_search,
                dates,
                ask_detach,
            };
            view::commit_list(ui, repo, viewport, list, actions);
        }
        pane => {
            let lines = diff_view::LinesUi {
                selection: repo.diff.selection(),
                can_write: !writing && !repo.modal_open(),
                back: true,
                split,
            };
            diff_view::diff_panel(ui, pane, lines, actions);
        }
    });
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

/// The message for a result of `command` that does not have the expected shape.
fn unexpected_result(command: &str, error: impl std::fmt::Display) -> String {
    tr!(
        "common-unexpected-result",
        command = command,
        error = error.to_string()
    )
}

/// The JSON parameters of a command from its engine type (e.g.
/// [`BranchCreateParams`](project_engine::BranchCreateParams)).
fn params(params: impl serde::Serialize) -> Value {
    serde_json::to_value(params).expect("the parameter types are serializable")
}

/// The result of `command` read as `T`; if it does not match, the message for the status bar.
fn decode<T: serde::de::DeserializeOwned>(command: &str, value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| unexpected_result(command, e))
}

/// `; warning: ...` for each warning, to append to a message.
fn warnings_text(warnings: &[String]) -> String {
    warnings
        .iter()
        .map(|w| tr!("common-warning", warning = w))
        .collect()
}

/// The engine warnings as the text appended to a notice.
fn messages_text(warnings: &[project_engine::Message]) -> String {
    let texts: Vec<String> = warnings.iter().map(project_engine::Message::text).collect();
    warnings_text(&texts)
}

/// Logs the failure of a command whose area shows the error by itself.
fn log_failure(done: &Done) {
    if let Err(e) = &done.result {
        log::warn!("{}: {e}", done.command);
    }
}
