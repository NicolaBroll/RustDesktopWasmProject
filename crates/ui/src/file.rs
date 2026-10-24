//! Blame and history of one file (S34): a view in place of the graph, opened from the context
//! menu of a file (commit details or stage area). Data only, no egui: tested with unit tests.
//! Contract in `impl-docs/git/reading.md`, "Blame and file history (S34)".
//!
//! - **Requests**: `file.blame` and `file.log` are slow reads (§6): they run in the background
//!   and only the tab in view is asked for, once. Every request has a number; an answer whose
//!   number is not the one in flight (the view was closed, another file opened, "Follow
//!   renames" toggled) is discarded, and the replaced request is cancelled
//!   ([`FileBox::take_cancel`] gives the tickets to pass to `Executor::cancel`).
//! - **Blame**: the lines are grouped in runs of consecutive lines from the same commit
//!   ([`runs`]); the view shows the commit only on the first line of each run.
//! - **History**: pages of [`PAGE`] entries, "Load more" asks for the next one with `skip`;
//!   selecting an entry opens `diff.commit` of that commit for the entry's path.

use crate::diff::{DiffTarget, MAX_LINE_COLS, number_width};
use crate::{Action, Done, Request, Tab, Ticket};
use project_domain::{Blame, BlameLine, Change, FileLogEntry, Oid, RepoPath};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// The id of the command that reads the blame.
pub const BLAME: &str = "file.blame";
/// The id of the command that reads the history.
pub const LOG: &str = "file.log";
/// Entries asked for per page of the history.
pub const PAGE: usize = 200;

/// The tab in view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Blame,
    History,
}

/// Consecutive lines last changed by the same commit (or all not committed yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// Index of the first line.
    pub start: usize,
    pub len: usize,
    /// Index in [`Blame::commits`]; `None` for lines not committed yet.
    pub commit: Option<usize>,
}

/// Groups the lines in runs of the same commit, in order.
pub fn runs(lines: &[BlameLine]) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        match out.last_mut() {
            Some(run) if run.commit == line.commit => run.len += 1,
            _ => out.push(Run {
                start: index,
                len: 1,
                commit: line.commit,
            }),
        }
    }
    out
}

/// A blame ready to draw: the runs and, per line, its run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameDoc {
    pub blame: Blame,
    pub runs: Vec<Run>,
    run_of_line: Vec<usize>,
    /// Digits of the last line number.
    pub number_width: usize,
    /// Columns of the widest line (at most [`MAX_LINE_COLS`]).
    pub max_cols: usize,
}

impl BlameDoc {
    pub fn new(blame: Blame) -> Self {
        let runs = runs(&blame.lines);
        let mut run_of_line = Vec::with_capacity(blame.lines.len());
        for (index, run) in runs.iter().enumerate() {
            run_of_line.extend(std::iter::repeat_n(index, run.len));
        }
        let max_cols = blame
            .lines
            .iter()
            .map(|l| {
                // Upper bound in characters, tabs expanded: cheap and enough for the scroll width.
                let text = l.text.to_string_lossy();
                text.chars().count() + text.matches('\t').count() * 7
            })
            .max()
            .unwrap_or(0)
            .min(MAX_LINE_COLS);
        let number_width = number_width(u32::try_from(blame.lines.len()).unwrap_or(u32::MAX));
        Self {
            blame,
            runs,
            run_of_line,
            number_width,
            max_cols,
        }
    }

    /// The index of the run of line `line` (index in `lines`).
    pub fn run_at(&self, line: usize) -> usize {
        self.run_of_line.get(line).copied().unwrap_or(0)
    }
}

/// The history read so far.
#[derive(Debug, Default)]
pub struct History {
    pub entries: Vec<FileLogEntry>,
    pub has_more: bool,
    /// The error of the last page asked for.
    pub error: Option<String>,
    /// True once the first page arrived.
    loaded: bool,
    /// "Load more" was asked: the next page is sent at the next [`FileBox::want`].
    more: bool,
}

/// What the view knows about something being read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loaded<'a, T> {
    Loading,
    Ready(&'a T),
    Failed(&'a str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Job {
    seq: u64,
    ticket: Option<Ticket>,
}

/// The open view.
#[derive(Debug)]
pub struct FileView {
    pub path: RepoPath,
    /// The commit (blame at that commit, history from it); `None`: the working tree for the blame,
    /// HEAD for the history.
    pub rev: Option<Oid>,
    pub mode: Mode,
    /// `follow` of `file.log` ("Follow renames", on by default).
    pub follow: bool,
    blame: Option<Result<BlameDoc, String>>,
    blame_job: Option<Job>,
    pub history: History,
    /// The page in flight and its `skip`.
    log_job: Option<(Job, usize)>,
}

impl FileView {
    pub fn blame(&self) -> Loaded<'_, BlameDoc> {
        match &self.blame {
            None => Loaded::Loading,
            Some(Ok(doc)) => Loaded::Ready(doc),
            Some(Err(e)) => Loaded::Failed(e),
        }
    }

    /// True while a page of the history is on its way.
    pub fn log_loading(&self) -> bool {
        self.log_job.is_some()
    }

    /// True before the first page of the history arrived (without errors).
    pub fn log_first_loading(&self) -> bool {
        !self.history.loaded && self.history.error.is_none()
    }

    /// The diff to open for entry `index` of the history: that commit, at the entry's path.
    pub fn entry_target(&self, index: usize) -> Option<DiffTarget> {
        let entry = self.history.entries.get(index)?;
        Some(DiffTarget::commit(entry.commit.id, entry.path.clone()))
    }
}

/// A request to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub command: &'static str,
    pub params: Value,
    pub seq: u64,
}

#[derive(Debug, Default)]
pub struct FileBox {
    view: Option<FileView>,
    next: u64,
    /// Tickets of replaced requests, to cancel.
    cancel: Vec<Ticket>,
}

impl FileBox {
    pub fn view(&self) -> Option<&FileView> {
        self.view.as_ref()
    }

    /// Opens the view of `path` at `rev` on tab `mode`. The same file at the same commit keeps
    /// what was already read and only changes tab.
    pub fn open(&mut self, path: RepoPath, rev: Option<Oid>, mode: Mode) {
        if let Some(view) = &mut self.view
            && view.path == path
            && view.rev == rev
        {
            view.mode = mode;
            return;
        }
        self.close();
        self.view = Some(FileView {
            path,
            rev,
            mode,
            follow: true,
            blame: None,
            blame_job: None,
            history: History::default(),
            log_job: None,
        });
    }

    /// Closes the view: the requests in flight are cancelled and their answers discarded.
    pub fn close(&mut self) {
        if let Some(view) = self.view.take() {
            self.forget(view.blame_job);
            self.forget(view.log_job.map(|(job, _)| job));
        }
    }

    fn forget(&mut self, job: Option<Job>) {
        if let Some(ticket) = job.and_then(|j| j.ticket) {
            self.cancel.push(ticket);
        }
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if let Some(view) = &mut self.view {
            view.mode = mode;
        }
    }

    /// Turns "Follow renames" on or off: the history is read again from the first page, the
    /// page in flight is cancelled.
    pub fn set_follow(&mut self, follow: bool) {
        let Some(view) = &mut self.view else {
            return;
        };
        if view.follow == follow {
            return;
        }
        view.follow = follow;
        view.history = History::default();
        let job = view.log_job.take().map(|(job, _)| job);
        self.forget(job);
    }

    /// "Load more": asks for the next page of the history.
    pub fn more(&mut self) {
        if let Some(view) = &mut self.view
            && view.history.has_more
            && view.log_job.is_none()
        {
            view.history.more = true;
            view.history.error = None;
        }
    }

    /// After an error, the tab in view is asked for again.
    pub fn retry(&mut self) {
        let Some(view) = &mut self.view else {
            return;
        };
        match view.mode {
            Mode::Blame => {
                if matches!(view.blame, Some(Err(_))) {
                    view.blame = None;
                }
            }
            Mode::History => {
                if view.history.error.take().is_some() && view.history.loaded {
                    view.history.more = true;
                }
            }
        }
    }

    /// The request to send for the tab in view, if it is not read yet and not in flight.
    /// Called on every frame.
    pub fn want(&mut self) -> Option<Want> {
        let view = self.view.as_mut()?;
        let path = serde_json::to_value(&view.path).unwrap_or_default();
        let rev = view.rev.map(|id| id.to_string());
        let seq = self.next;
        let want = match view.mode {
            Mode::Blame => {
                if view.blame.is_some() || view.blame_job.is_some() {
                    return None;
                }
                view.blame_job = Some(Job { seq, ticket: None });
                let mut params = json!({ "path": path });
                if let Some(rev) = rev {
                    params["rev"] = json!(rev);
                }
                Want {
                    command: BLAME,
                    params,
                    seq,
                }
            }
            Mode::History => {
                let history = &mut view.history;
                let first = !history.loaded && history.error.is_none();
                if view.log_job.is_some() || !(first || history.more) {
                    return None;
                }
                history.more = false;
                let skip = history.entries.len();
                view.log_job = Some((Job { seq, ticket: None }, skip));
                let mut params = json!({
                    "path": path,
                    "skip": skip,
                    "limit": PAGE,
                    "follow": view.follow,
                });
                if let Some(rev) = rev {
                    params["rev"] = json!(rev);
                }
                Want {
                    command: LOG,
                    params,
                    seq,
                }
            }
        };
        self.next += 1;
        Some(want)
    }

    /// Notes the ticket of request `seq`, to cancel it if it is replaced.
    pub fn sent(&mut self, seq: u64, ticket: Ticket) {
        let Some(view) = &mut self.view else {
            return;
        };
        for job in [
            view.blame_job.as_mut(),
            view.log_job.as_mut().map(|(j, _)| j),
        ]
        .into_iter()
        .flatten()
        {
            if job.seq == seq {
                job.ticket = Some(ticket);
            }
        }
    }

    /// The tickets of the requests replaced or closed since the last call.
    pub fn take_cancel(&mut self) -> Vec<Ticket> {
        std::mem::take(&mut self.cancel)
    }

    /// The answer to request `seq`: kept only if it is the one in flight; returns `false` if it
    /// was discarded.
    pub fn receive(&mut self, seq: u64, result: Result<Value, String>) -> bool {
        let Some(view) = &mut self.view else {
            return false;
        };
        if view.blame_job.is_some_and(|j| j.seq == seq) {
            view.blame_job = None;
            let doc = result.and_then(|value| {
                serde_json::from_value::<Blame>(value)
                    .map(BlameDoc::new)
                    .map_err(|e| unexpected(BLAME, &e))
            });
            view.blame = Some(doc);
            return true;
        }
        if let Some((job, skip)) = view.log_job
            && job.seq == seq
        {
            view.log_job = None;
            let history = &mut view.history;
            match result.and_then(|value| {
                serde_json::from_value::<LogPage>(value).map_err(|e| unexpected(LOG, &e))
            }) {
                Ok(page) => {
                    history.entries.truncate(skip);
                    history.entries.extend(page.commits);
                    history.has_more = page.has_more;
                    history.loaded = true;
                    history.error = None;
                }
                Err(e) => history.error = Some(e),
            }
            return true;
        }
        false
    }
}

#[derive(Deserialize)]
struct LogPage {
    commits: Vec<FileLogEntry>,
    has_more: bool,
}

fn unexpected(command: &str, e: &serde_json::Error) -> String {
    tr!(
        "common-unexpected-result",
        command = command,
        error = e.to_string()
    )
}

/// The name of the file in `entry` when it is not `current` (the file shown).
pub fn entry_path(entry: &FileLogEntry, current: &RepoPath) -> Option<String> {
    (entry.path != *current).then(|| entry.path.to_string_lossy().into_owned())
}

/// Where the file came from in `entry`: "renamed from X" or "copied from X".
pub fn entry_origin(entry: &FileLogEntry) -> Option<String> {
    let old = entry.old_path.as_ref()?.to_string_lossy();
    match entry.status {
        Change::Renamed => Some(tr!("file-renamed-from", path = old.as_ref())),
        Change::Copied => Some(tr!("file-copied-from", path = old.as_ref())),
        _ => None,
    }
}

impl Tab {
    pub(crate) fn apply_file(&mut self, action: Action) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        match action {
            Action::OpenFile { path, rev, mode } => {
                repo.conflict.close();
                repo.diff.close();
                repo.file.open(path, rev, mode);
            }
            Action::CloseFile => {
                repo.file.close();
                repo.diff.close();
            }
            Action::FileMode(mode) => repo.file.set_mode(mode),
            Action::FileFollow(follow) => {
                repo.file.set_follow(follow);
                repo.diff.close();
            }
            Action::FileMore => repo.file.more(),
            Action::FileRetry => repo.file.retry(),
            Action::FileSelect(index) => {
                let target = repo.file.view().and_then(|v| v.entry_target(index));
                if let Some(target) = target
                    && let Some(seq) = repo.diff.open(target.clone())
                {
                    self.request_diff(&target, seq);
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Cancels the replaced requests and sends the one of the tab in view, if needed.
    pub(crate) fn request_file(&mut self) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        let cancel = repo.file.take_cancel();
        let want = repo.file.want();
        for ticket in cancel {
            self.executor.cancel(ticket);
        }
        if let Some(want) = want {
            let ticket = self.executor.submit(want.command, want.params);
            self.pending
                .insert(ticket, Request::File(generation, want.seq));
            if let Some(repo) = &mut self.repo {
                repo.file.sent(want.seq, ticket);
            }
        }
    }

    pub(crate) fn receive_file(&mut self, generation: u64, seq: u64, done: Done) {
        if let Err(e) = &done.result {
            log::warn!("{}: {e}", done.command);
        }
        if let Some(repo) = self.current(generation) {
            repo.file.receive(seq, done.result);
        }
    }
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
