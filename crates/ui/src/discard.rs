//! Discarding unstaged changes (`worktree.discard`) and the journal for Undo/Redo
//! (`edit.undo`/`edit.redo`). Data only, no egui: tested with unit tests.
//!
//! Discarding happens in two steps: first `worktree.discard` without `confirm`, which changes nothing and
//! answers with `ConfirmRequired` (or refuses at once with `NotAllowed`, e.g. a conflicted file in the
//! repo); then, only if the user confirms in the dialog, the same command with `"confirm": true`.
//! Discarding a hunk or the selected lines of the unstaged diff (`worktree.discard_lines`) goes
//! through the same two steps, with the lines and the digest of the diff seen.

use crate::ErrorKind;
use crate::stage::StageItem;
use crate::{Done, Request, Tab};
use project_domain::{Change, LineRef, RepoPath, Status};
use project_engine::{
    DiscardLinesParams, DiscardLinesResult, DiscardParams, DiscardResult, JournalState, UndoResult,
};
use project_i18n::tr;
use serde_json::{Value, json};

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Discards the unstaged changes of the given files (never empty): first the engine checks
    /// and asks for confirmation, which opens in a dialog.
    Discard(Vec<RepoPath>),
    /// Confirms the pending discard: resends with `"confirm": true`.
    Confirm,
    /// Closes the discard dialog without discarding anything.
    Cancel,
    /// Undoes the last journal operation, or redoes the last undone one.
    Undo(UndoOp),
}

/// True if the changes of the file can be discarded: tracked, not conflicted. Untracked files
/// are never touched (discarding them would mean deleting them, with no backup).
pub fn discardable(item: &StageItem) -> bool {
    !item.is_conflict() && !matches!(item.change, Change::Untracked | Change::Unmerged)
}

/// Why the changes of `item` cannot be discarded, for the menu; `None` if they can.
pub fn not_discardable_reason(item: &StageItem) -> Option<String> {
    if item.is_conflict() || item.change == Change::Unmerged {
        Some(tr!("discard-reason-conflict"))
    } else if item.change == Change::Untracked {
        Some(tr!("discard-reason-untracked"))
    } else {
        None
    }
}

/// The paths to discard with "Discard all": only the tracked files without conflicts in the
/// list, always explicit; `None` if there are none.
pub fn discardable_paths(items: &[StageItem]) -> Option<Vec<RepoPath>> {
    let mut paths: Vec<RepoPath> = Vec::new();
    for item in items.iter().filter(|item| discardable(item)) {
        if !paths.contains(&item.path) {
            paths.push(item.path.clone());
        }
    }
    (!paths.is_empty()).then_some(paths)
}

/// The id of the Registry command.
pub const COMMAND: &str = "worktree.discard";

/// The id of the Registry command that discards some lines of one file.
pub const LINES_COMMAND: &str = "worktree.discard_lines";

/// Some lines of one file to discard (`worktree.discard_lines`), chosen in its unstaged diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinesPick {
    /// The changed lines, never empty.
    pub lines: Vec<LineRef>,
    /// The digest of the `diff.worktree` diff seen.
    pub digest: String,
    /// The hunk (index in the file) when it is a whole hunk: only for the texts.
    pub hunk: Option<usize>,
}

/// Name of the operation for messages, e.g. "Discarding changes to README.md".
pub fn describe(paths: &[RepoPath]) -> String {
    match paths {
        [one] => tr!(
            "discard-label-one",
            path = one.to_string_lossy().into_owned()
        ),
        many => tr!("discard-label-many", n = many.len()),
    }
}

/// The check before discarding, e.g. "Checking before discarding changes to README.md".
pub fn describe_check(paths: &[RepoPath]) -> String {
    match paths {
        [one] => tr!(
            "discard-check-label-one",
            path = one.to_string_lossy().into_owned()
        ),
        many => tr!("discard-check-label-many", n = many.len()),
    }
}

/// The parameters of `worktree.discard`; `confirm` only after the user's confirmation.
pub fn params(paths: &[RepoPath], confirm: bool) -> Value {
    crate::params(DiscardParams {
        paths: paths.to_vec(),
        confirm,
    })
}

/// Where the discard stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Sent without `confirm`: the engine checks and answers with the confirmation request.
    Checking,
    /// The confirmation dialog is open; `reason` is the engine's reason.
    Confirm { reason: String },
    /// Sent with `"confirm": true`.
    Sending,
}

/// A discard in progress or to confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discard {
    pub paths: Vec<RepoPath>,
    /// Only these lines of the one file in `paths` (`worktree.discard_lines`); `None`: the
    /// whole files (`worktree.discard`).
    pub lines: Option<LinesPick>,
    pub phase: Phase,
}

impl Discard {
    /// The id of the Registry command.
    pub fn command(&self) -> &'static str {
        if self.lines.is_some() {
            LINES_COMMAND
        } else {
            COMMAND
        }
    }

    /// The parameters of the command; `confirm` only after the user's confirmation.
    pub fn params(&self, confirm: bool) -> Value {
        match (&self.lines, self.paths.as_slice()) {
            (Some(pick), [path]) => crate::params(DiscardLinesParams {
                path: path.clone(),
                lines: pick.lines.clone(),
                digest: pick.digest.clone(),
                confirm,
            }),
            _ => params(&self.paths, confirm),
        }
    }

    /// Name of the operation for messages, e.g. "Discarding hunk 1 of README.md".
    pub fn label(&self) -> String {
        match (&self.lines, self.paths.as_slice()) {
            (Some(pick), [path]) => {
                let path = path.to_string_lossy().into_owned();
                match pick.hunk {
                    Some(h) => tr!("discard-lines-label-hunk", hunk = h + 1, path = path),
                    None => tr!("discard-lines-label", n = pick.lines.len(), path = path),
                }
            }
            _ => describe(&self.paths),
        }
    }

    /// The check before discarding, e.g. "Checking before discarding lines of README.md".
    pub fn check_label(&self) -> String {
        match (&self.lines, self.paths.as_slice()) {
            (Some(_), [path]) => tr!(
                "discard-lines-check-label",
                path = path.to_string_lossy().into_owned()
            ),
            _ => describe_check(&self.paths),
        }
    }
}

/// Discarding the changes, from the check to the response. Only one at a time.
#[derive(Debug, Default)]
pub struct DiscardBox {
    current: Option<Discard>,
}

/// A completed discard, of files or of lines.
#[derive(Debug)]
pub struct Finished {
    /// The message for the status bar, with the backup ref.
    pub notice: String,
    pub status: Option<Status>,
    pub status_error: Option<String>,
    pub journal: JournalState,
}

impl From<DiscardResult> for Finished {
    fn from(result: DiscardResult) -> Self {
        Finished {
            notice: notice(&result),
            status: result.status,
            status_error: result.status_error,
            journal: result.journal,
        }
    }
}

impl Finished {
    fn lines(result: DiscardLinesResult, pick: &LinesPick) -> Self {
        let path = result.file.path.to_string_lossy().into_owned();
        let mut notice = match pick.hunk {
            Some(h) => tr!(
                "discard-lines-done-hunk",
                hunk = h + 1,
                path = path,
                backup = &result.backup
            ),
            None => tr!(
                "discard-lines-done",
                n = pick.lines.len(),
                path = path,
                backup = &result.backup
            ),
        };
        notice.push_str(&crate::messages_text(&result.warnings));
        Finished {
            notice,
            status: result.status,
            status_error: result.status_error,
            journal: result.journal,
        }
    }
}

/// What came back from `worktree.discard` or `worktree.discard_lines`.
#[derive(Debug)]
pub enum Received {
    /// Confirmation needed: the dialog opens.
    Confirm,
    /// The changes are discarded.
    Done(Box<Finished>),
    /// Refused or failed: the message to show. Nothing changed, except if the
    /// result cannot be read (then the discard may have happened: the status is reread).
    /// `stale`: the diff seen is no longer the current one (lines only): it is reread.
    Failed {
        message: String,
        reload: bool,
        stale: bool,
    },
}

impl DiscardBox {
    #[cfg(test)]
    pub fn current(&self) -> Option<&Discard> {
        self.current.as_ref()
    }

    /// The discard awaiting confirmation, if the dialog is open.
    pub fn confirm(&self) -> Option<&Discard> {
        self.current
            .as_ref()
            .filter(|d| matches!(d.phase, Phase::Confirm { .. }))
    }

    /// Starts a discard: the parameters of the check (without `confirm`). `None` if there is already
    /// one or if there is nothing to discard.
    pub fn start(&mut self, paths: Vec<RepoPath>) -> Option<Value> {
        self.begin(Discard {
            paths,
            lines: None,
            phase: Phase::Checking,
        })
    }

    /// Starts discarding some lines of `path`: the parameters of the check (without
    /// `confirm`). `None` if there is already a discard or no line.
    pub fn start_lines(&mut self, path: RepoPath, pick: LinesPick) -> Option<Value> {
        if pick.lines.is_empty() {
            return None;
        }
        self.begin(Discard {
            paths: vec![path],
            lines: Some(pick),
            phase: Phase::Checking,
        })
    }

    fn begin(&mut self, discard: Discard) -> Option<Value> {
        if self.current.is_some() || discard.paths.is_empty() {
            return None;
        }
        let params = discard.params(false);
        self.current = Some(discard);
        Some(params)
    }

    /// The discard in progress (checking, to confirm or sending).
    pub fn pending(&self) -> Option<&Discard> {
        self.current.as_ref()
    }

    /// The user confirmed: the parameters with `"confirm": true`.
    pub fn confirmed(&mut self) -> Option<Value> {
        let discard = self.current.as_mut()?;
        if !matches!(discard.phase, Phase::Confirm { .. }) {
            return None;
        }
        discard.phase = Phase::Sending;
        Some(discard.params(true))
    }

    /// Closes the dialog without discarding anything.
    pub fn cancel(&mut self) {
        if self.confirm().is_some() {
            self.current = None;
        }
    }

    /// The response of `worktree.discard`, in the phase it was in.
    pub fn receive(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(discard) = self.current.take() else {
            return Received::Failed {
                message: tr!("common-unexpected-response", command = COMMAND),
                reload: true,
                stale: false,
            };
        };
        let label = discard.label();
        let failed = |message| Received::Failed {
            message,
            reload: false,
            stale: false,
        };
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && discard.phase == Phase::Checking =>
            {
                self.current = Some(Discard {
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                    ..discard
                });
                Received::Confirm
            }
            Err(_) if kind == Some(ErrorKind::StaleDiff) => Received::Failed {
                message: tr!("app-write-stale", label = &label),
                reload: false,
                stale: true,
            },
            Err(e) if kind == Some(ErrorKind::NotAllowed) => {
                failed(tr!("discard-not-allowed", label = &label, error = e))
            }
            Err(e) => failed(tr!("common-failed", label = &label, error = e)),
            Ok(value) => {
                let command = discard.command();
                let finished = match &discard.lines {
                    Some(pick) => serde_json::from_value::<DiscardLinesResult>(value)
                        .map(|r| Finished::lines(r, pick)),
                    None => serde_json::from_value::<DiscardResult>(value).map(Finished::from),
                };
                match finished {
                    Ok(finished) => Received::Done(Box::new(finished)),
                    Err(e) => Received::Failed {
                        message: crate::unexpected_result(command, e),
                        reload: true,
                        stale: false,
                    },
                }
            }
        }
    }
}

/// The message for the status bar after `worktree.discard`.
fn notice(result: &DiscardResult) -> String {
    match result.files.as_slice() {
        [one] => tr!(
            "discard-done-one",
            path = one.path.to_string_lossy().into_owned(),
            backup = &result.backup
        ),
        many => tr!("discard-done-many", n = many.len(), backup = &result.backup),
    }
}

/// The state of the operation journal: what "Undo" would undo and what "Redo" would redo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub can_undo: bool,
    pub can_redo: bool,
    pub next_undo: Option<String>,
    pub next_redo: Option<String>,
}

impl From<JournalState> for History {
    fn from(j: JournalState) -> Self {
        History {
            can_undo: j.can_undo,
            can_redo: j.can_redo,
            next_undo: j.next_undo.as_ref().map(project_engine::Message::text),
            next_redo: j.next_redo.as_ref().map(project_engine::Message::text),
        }
    }
}

/// Undo or Redo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UndoOp {
    Undo,
    Redo,
}

impl UndoOp {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            UndoOp::Undo => "edit.undo",
            UndoOp::Redo => "edit.redo",
        }
    }

    pub fn label(self) -> String {
        match self {
            UndoOp::Undo => tr!("undo-undo"),
            UndoOp::Redo => tr!("undo-redo"),
        }
    }

    /// True if there is something to do.
    pub fn available(self, history: &History) -> bool {
        match self {
            UndoOp::Undo => history.can_undo,
            UndoOp::Redo => history.can_redo,
        }
    }

    /// The description of the entry it would affect.
    pub fn next(self, history: &History) -> Option<&str> {
        match self {
            UndoOp::Undo => history.next_undo.as_deref(),
            UndoOp::Redo => history.next_redo.as_deref(),
        }
    }

    /// The button's hint.
    pub fn hint(self, history: &History) -> String {
        match (self, self.next(history)) {
            (UndoOp::Undo, Some(next)) => tr!("undo-undo-hint", next = next),
            (UndoOp::Redo, Some(next)) => tr!("undo-redo-hint", next = next),
            (UndoOp::Undo, None) => tr!("undo-nothing-to-undo"),
            (UndoOp::Redo, None) => tr!("undo-nothing-to-redo"),
        }
    }
}

/// The message for the status bar after `edit.undo`/`edit.redo`.
fn undo_notice(result: &UndoResult) -> String {
    let mut text = match (&result.undone, &result.redone) {
        (Some(entry), _) => tr!("undo-undone", description = entry.description.text()),
        (None, Some(entry)) => tr!("undo-redone", description = entry.description.text()),
        (None, None) => tr!("undo-done"),
    };
    if !result.backups.is_empty() {
        text.push_str(&tr!("undo-backups", refs = result.backups.join(", ")));
    }
    text.push_str(&crate::messages_text(&result.warnings));
    text
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_discard(&mut self, action: Action) {
        match action {
            Action::Discard(paths) => self.start_discard(paths, None),
            Action::Confirm => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let Some(pending) = repo.discard.confirm() else {
                    return;
                };
                let (label, command) = (pending.label(), pending.command());
                if let Some(params) = repo.discard.confirmed() {
                    repo.write_label = Some(label);
                    self.submit(command, params, Request::Discard(generation));
                }
            }
            Action::Cancel => {
                if let Some(repo) = &mut self.repo {
                    repo.discard.cancel();
                }
            }
            Action::Undo(op) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some()
                    || repo.modal_open()
                    || !op.available(&repo.history)
                {
                    return;
                }
                repo.write_error = None;
                repo.write_label = Some(op.hint(&repo.history));
                self.submit(op.command(), json!({}), Request::Undo(generation));
            }
        }
    }

    /// Starts discarding the changes of `paths`, or only the `lines` of the one file in
    /// `paths`: the check, then the confirmation dialog. Never two writes together, nor with a
    /// confirmation open.
    pub(crate) fn start_discard(&mut self, paths: Vec<RepoPath>, lines: Option<LinesPick>) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        if self.executor.writing().is_some() || repo.modal_open() {
            return;
        }
        let params = match (lines, paths.as_slice()) {
            (Some(pick), [path]) => repo.discard.start_lines(path.clone(), pick),
            (Some(_), _) => None,
            (None, _) => repo.discard.start(paths),
        };
        let Some(params) = params else {
            return;
        };
        let Some(pending) = repo.discard.pending() else {
            return;
        };
        let (label, command) = (pending.check_label(), pending.command());
        repo.write_error = None;
        repo.write_label = Some(label);
        self.submit(command, params, Request::Discard(generation));
    }

    /// Response of `worktree.discard`: the check opens the confirmation dialog, the completed
    /// discard brings the new status and the Undo/Redo buttons; a refusal goes in the stage area.
    pub(crate) fn receive_discard(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .discard
            .receive(done.result, done.error_kind, done.error_detail)
        {
            Received::Confirm => {}
            Received::Done(result) => {
                let Finished {
                    notice,
                    status,
                    status_error,
                    journal,
                } = *result;
                repo.history = journal.into();
                match status {
                    Some(status) => {
                        repo.set_status(status);
                        self.error.clear();
                        self.follow_diff();
                    }
                    None => {
                        let reason = status_error.unwrap_or_default();
                        self.error = tr!("discard-status-not-reread", reason = reason);
                        self.submit("repo.status", json!({}), Request::Status(generation));
                    }
                }
                self.notice = notice;
            }
            Received::Failed {
                message,
                reload,
                stale,
            } => {
                log::warn!("{}: {message}", done.command);
                repo.write_error = Some(message.clone());
                // The diff seen is no longer the current one: the selection is cleared, status
                // and diff are reread (the diff follows the status).
                if stale && let Some((_, _, selection)) = repo.diff.loaded_mut() {
                    selection.clear();
                }
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else if stale {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                }
            }
        }
    }

    /// Response of `edit.undo`/`edit.redo`: the updated journal, then everything is reread (an
    /// undo may move branches or put files back).
    pub(crate) fn receive_undo(&mut self, generation: u64, done: Done) {
        let result = done
            .result
            .and_then(|value| crate::decode::<UndoResult>(&done.command, value));
        let Some(repo) = self.current(generation) else {
            return;
        };
        let label = repo
            .write_label
            .take()
            .unwrap_or_else(|| done.command.clone());
        match result {
            Ok(result) => {
                let notice = undo_notice(&result);
                let UndoResult {
                    status,
                    status_error,
                    journal,
                    ..
                } = result;
                repo.history = journal.into();
                if let Some(status) = status {
                    repo.set_status(status);
                }
                self.notice = notice;
                self.error = status_error
                    .map(|e| tr!("app-done-status-unread", error = e))
                    .unwrap_or_default();
                self.follow_diff();
                self.load();
            }
            Err(e) => {
                log::warn!("{}: {e}", done.command);
                self.error = tr!("common-failed", label = &label, error = &e);
                self.notice.clear();
                // The journal is reread: an entry may no longer be undoable.
                self.submit("edit.history", json!({}), Request::History(generation));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::{Side, StageLists};
    use project_domain::Conflict;
    use project_engine::HistoryResult;

    fn item(path: &str, change: Change, conflict: Option<Conflict>) -> StageItem {
        StageItem {
            path: path.into(),
            change,
            original: None,
            conflict,
        }
    }

    #[test]
    fn only_tracked_files_without_conflicts_are_discardable() {
        let items = [
            item("zeta.rs", Change::Unmerged, Some(Conflict::BothModified)),
            item("README.md", Change::Modified, None),
            item("removed.txt", Change::Deleted, None),
            item("note.txt", Change::Untracked, None),
        ];
        let can: Vec<bool> = items.iter().map(discardable).collect();
        assert_eq!(can, [false, true, true, false]);
        assert!(
            not_discardable_reason(&items[0])
                .unwrap()
                .contains("Conflicted")
        );
        assert!(
            not_discardable_reason(&items[3])
                .unwrap()
                .contains("Untracked")
        );
        assert_eq!(not_discardable_reason(&items[1]), None);
        assert_eq!(
            discardable_paths(&items).unwrap(),
            ["README.md", "removed.txt"].map(RepoPath::from)
        );
        assert_eq!(discardable_paths(&items[3..]), None);
        assert_eq!(discardable_paths(&[]), None);
    }

    #[test]
    fn confirm_is_sent_only_after_the_dialog() {
        let mut b = DiscardBox::default();
        assert_eq!(b.start(Vec::new()), None);
        let check = b.start(vec!["README.md".into()]).unwrap();
        assert_eq!(check, json!({ "paths": ["README.md"] }));
        // One at a time, and no confirmation before the engine's response.
        assert_eq!(b.start(vec!["other".into()]), None);
        assert_eq!(b.confirmed(), None);
        assert!(b.confirm().is_none());

        let r = b.receive(
            Err("asks for confirmation".to_owned()),
            Some(ErrorKind::ConfirmRequired),
            Some("reason".to_owned()),
        );
        assert!(matches!(r, Received::Confirm));
        let confirm = b.confirm().unwrap();
        assert_eq!(
            confirm.phase,
            Phase::Confirm {
                reason: "reason".to_owned()
            }
        );
        assert_eq!(confirm.paths, [RepoPath::from("README.md")]);

        let send = b.confirmed().unwrap();
        assert_eq!(send, json!({ "paths": ["README.md"], "confirm": true }));
        assert!(b.confirm().is_none());
        assert_eq!(b.current().unwrap().phase, Phase::Sending);
        assert_eq!(b.confirmed(), None);
    }

    #[test]
    fn cancelling_the_dialog_sends_nothing() {
        let mut b = DiscardBox::default();
        b.start(vec!["a".into(), "b".into()]).unwrap();
        b.receive(Err(String::new()), Some(ErrorKind::ConfirmRequired), None);
        b.cancel();
        assert!(b.current().is_none());
        assert_eq!(b.confirmed(), None);
    }

    #[test]
    fn refusals_are_shown_and_close_the_discard() {
        let mut b = DiscardBox::default();
        b.start(vec!["README.md".into()]).unwrap();
        let r = b.receive(
            Err("operation refused: there are conflicted files".to_owned()),
            Some(ErrorKind::NotAllowed),
            None,
        );
        let Received::Failed {
            message,
            reload,
            stale,
        } = r
        else {
            panic!("{r:?}");
        };
        assert_eq!(
            message,
            "Discarding changes to README.md not possible: operation refused: there are \
             conflicted files"
        );
        assert!(!reload && !stale);
        assert!(b.current().is_none());

        // A confirmation requested again after `confirm: true` does not reopen the dialog forever.
        b.start(vec!["a".into(), "b".into()]).unwrap();
        b.receive(Err(String::new()), Some(ErrorKind::ConfirmRequired), None);
        b.confirmed().unwrap();
        let r = b.receive(Err("x".to_owned()), Some(ErrorKind::ConfirmRequired), None);
        assert!(matches!(r, Received::Failed { .. }), "{r:?}");
        assert!(b.current().is_none());
    }

    #[test]
    fn undo_buttons_name_the_next_entry() {
        let history = History {
            can_undo: true,
            can_redo: false,
            next_undo: Some("Discard changes to “README.md”".to_owned()),
            next_redo: None,
        };
        assert!(UndoOp::Undo.available(&history));
        assert!(!UndoOp::Redo.available(&history));
        assert_eq!(
            UndoOp::Undo.hint(&history),
            "Undo: Discard changes to “README.md”"
        );
        assert_eq!(UndoOp::Redo.hint(&history), "Nothing to redo");
        assert_eq!(UndoOp::Undo.command(), "edit.undo");
        assert_eq!(UndoOp::Redo.command(), "edit.redo");
    }

    fn statuses(out: &Value) -> StageLists {
        StageLists::new(&serde_json::from_value(out["status"].clone()).unwrap())
    }

    /// The full path with the sample repo of the `MemoryBackend`, as the UI does it:
    /// check, confirmation, discard, then undo and redo from the button.
    #[test]
    fn discarding_with_the_sample_repo_then_undo() {
        use project_engine::{EngineError, MemoryOpener, Registry, Session};
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        let path = MemoryOpener::SAMPLE_PATH;
        registry
            .run("repo.open", &mut session, json!({ "path": path }))
            .unwrap();
        let history: HistoryResult = serde_json::from_value(
            registry
                .run("edit.history", &mut session, json!({}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(History::from(history.journal), History::default());
        let status = registry
            .run("repo.status", &mut session, json!({}))
            .unwrap();
        let lists = StageLists::new(&serde_json::from_value(status).unwrap());
        let paths = discardable_paths(&lists.unstaged).unwrap();
        assert!(paths.contains(&"src/lib.rs".into()), "{paths:?}");
        let diff_before = registry
            .run(
                "diff.worktree",
                &mut session,
                json!({ "path": "src/lib.rs" }),
            )
            .unwrap();

        // The check: nothing changes, confirmation is needed.
        let mut b = DiscardBox::default();
        let check = b.start(vec!["src/lib.rs".into()]).unwrap();
        let err = registry.run(COMMAND, &mut session, check).unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
        let r = b.receive(
            Err(err.text()),
            Some(ErrorKind::of(&err)),
            ErrorKind::detail(&err),
        );
        assert!(matches!(r, Received::Confirm));
        let Phase::Confirm { reason } = &b.confirm().unwrap().phase else {
            unreachable!()
        };
        assert!(reason.contains("src/lib.rs"), "{reason}");

        // The confirmation.
        let send = b.confirmed().unwrap();
        let out = registry.run(COMMAND, &mut session, send).unwrap();
        assert!(!statuses(&out).contains(Side::Unstaged, &"src/lib.rs".into()));
        let Received::Done(result) = b.receive(Ok(out), None, None) else {
            panic!()
        };
        let notice = result.notice;
        assert!(notice.contains("refs/project/backup/discard-"), "{notice}");
        assert!(notice.contains("src/lib.rs"), "{notice}");
        let history = History::from(result.journal);
        assert!(history.can_undo);
        assert_eq!(
            UndoOp::Undo.next(&history),
            Some("Discard changes to “src/lib.rs”")
        );
        assert!(notice.contains("Undo"), "{notice}");

        // Undo: the file comes back with its changes, identical.
        let out = registry
            .run(UndoOp::Undo.command(), &mut session, json!({}))
            .unwrap();
        assert!(statuses(&out).contains(Side::Unstaged, &"src/lib.rs".into()));
        let undo: UndoResult = serde_json::from_value(out).unwrap();
        let notice = undo_notice(&undo);
        assert!(notice.starts_with("Undone: Discard"), "{notice}");
        assert!(undo.journal.can_redo && !undo.journal.can_undo);
        let diff_after = registry
            .run(
                "diff.worktree",
                &mut session,
                json!({ "path": "src/lib.rs" }),
            )
            .unwrap();
        assert_eq!(diff_before, diff_after);

        // Redo: discarded again.
        let out = registry
            .run(UndoOp::Redo.command(), &mut session, json!({}))
            .unwrap();
        assert!(!statuses(&out).contains(Side::Unstaged, &"src/lib.rs".into()));
        let redo: UndoResult = serde_json::from_value(out).unwrap();
        let notice = undo_notice(&redo);
        assert!(notice.starts_with("Redone: "), "{notice}");
    }

    #[test]
    fn untracked_files_are_refused_before_the_dialog() {
        use project_engine::{MemoryOpener, Registry, Session};
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        registry
            .run(
                "repo.open",
                &mut session,
                json!({ "path": MemoryOpener::SAMPLE_PATH }),
            )
            .unwrap();
        let mut b = DiscardBox::default();
        let check = b.start(vec!["note.txt".into()]).unwrap();
        let err = registry.run(COMMAND, &mut session, check).unwrap_err();
        let r = b.receive(Err(err.text()), Some(ErrorKind::of(&err)), None);
        let Received::Failed { message, .. } = r else {
            panic!("{r:?}")
        };
        assert!(message.contains("not possible"), "{message}");
        assert!(b.confirm().is_none());
    }

    #[test]
    fn discarding_lines_asks_with_lines_and_digest() {
        let mut b = DiscardBox::default();
        let pick = LinesPick {
            lines: vec![LineRef::Old(2), LineRef::New(2)],
            digest: "d".to_owned(),
            hunk: None,
        };
        let empty = LinesPick {
            lines: Vec::new(),
            ..pick.clone()
        };
        assert_eq!(b.start_lines("a.rs".into(), empty), None);
        let check = b.start_lines("a.rs".into(), pick.clone()).unwrap();
        assert_eq!(
            check,
            json!({ "path": "a.rs", "lines": [{ "old": 2 }, { "new": 2 }], "digest": "d" })
        );
        let pending = b.pending().unwrap();
        assert_eq!(pending.command(), LINES_COMMAND);
        assert_eq!(pending.label(), "Discarding 2 lines of a.rs");
        assert_eq!(
            pending.check_label(),
            "Checking before discarding lines of a.rs"
        );
        // One at a time.
        assert_eq!(b.start(vec!["b".into()]), None);
        b.receive(Err(String::new()), Some(ErrorKind::ConfirmRequired), None);
        assert_eq!(b.confirm().unwrap().lines.as_ref(), Some(&pick));
        let send = b.confirmed().unwrap();
        assert_eq!(send["confirm"], json!(true));
        assert_eq!(send["digest"], json!("d"));

        // The diff changed meanwhile: the selection is redone.
        let r = b.receive(Err("stale".to_owned()), Some(ErrorKind::StaleDiff), None);
        let Received::Failed { stale, reload, .. } = r else {
            panic!("{r:?}")
        };
        assert!(stale && !reload);
        assert!(b.current().is_none());

        let hunk = LinesPick {
            hunk: Some(0),
            ..pick
        };
        b.start_lines("a.rs".into(), hunk).unwrap();
        assert_eq!(b.pending().unwrap().label(), "Discarding hunk 1 of a.rs");
    }

    /// The full path with the sample repo: the first hunk of the unstaged diff of `src/lib.rs`
    /// chosen as the diff view does, check, confirmation, discard, then undo.
    #[test]
    fn discarding_a_hunk_with_the_sample_repo_then_undo() {
        use crate::diff::{DiffDoc, DiffSource, DiffTarget};
        use crate::lines::{Pick, discard_plan};
        use project_domain::Diff;
        use project_engine::{DiffWithDigests, EngineError, MemoryOpener, Registry, Session};
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        registry
            .run(
                "repo.open",
                &mut session,
                json!({ "path": MemoryOpener::SAMPLE_PATH }),
            )
            .unwrap();
        let path = RepoPath::from("src/lib.rs");
        let target = DiffTarget {
            source: DiffSource::Worktree,
            path: path.clone(),
        };
        let read = |session: &mut Session| {
            let (command, params) = target.request();
            let value = registry.run(command, session, params).unwrap();
            let read: DiffWithDigests = serde_json::from_value(value.clone()).unwrap();
            let diff = Diff {
                files: read.files.into_iter().map(|f| f.file).collect(),
            };
            (DiffDoc::new(diff, &path), value)
        };
        let (doc, before) = read(&mut session);
        assert_eq!(doc.diff.files[0].hunks.len(), 2);
        let (file, pick) = discard_plan(&doc, &target, Pick::Hunk(0)).unwrap();

        let mut b = DiscardBox::default();
        let check = b.start_lines(file, pick).unwrap();
        let err = registry
            .run(LINES_COMMAND, &mut session, check)
            .unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
        let r = b.receive(
            Err(err.text()),
            Some(ErrorKind::of(&err)),
            ErrorKind::detail(&err),
        );
        assert!(matches!(r, Received::Confirm));
        let Phase::Confirm { reason } = &b.confirm().unwrap().phase else {
            unreachable!()
        };
        assert!(reason.contains("src/lib.rs"), "{reason}");

        let send = b.confirmed().unwrap();
        let out = registry.run(LINES_COMMAND, &mut session, send).unwrap();
        assert!(
            statuses(&out).contains(Side::Unstaged, &path),
            "one hunk left"
        );
        let Received::Done(result) = b.receive(Ok(out), None, None) else {
            panic!()
        };
        assert!(
            result.notice.starts_with("Hunk 1 of src/lib.rs discarded")
                && result.notice.contains("refs/project/backup/discard-"),
            "{}",
            result.notice
        );
        assert!(History::from(result.journal).can_undo);
        let (doc, _) = read(&mut session);
        assert_eq!(doc.diff.files[0].hunks.len(), 1, "the second hunk remains");

        registry
            .run(UndoOp::Undo.command(), &mut session, json!({}))
            .unwrap();
        assert_eq!(read(&mut session).1, before);
    }
}
