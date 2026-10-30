//! Discarding unstaged changes (`worktree.discard`) and the journal for Undo/Redo
//! (`edit.undo`/`edit.redo`). Data only, no egui: tested with unit tests.
//!
//! Discarding happens in two steps: first `worktree.discard` without `confirm`, which changes nothing and
//! answers with `ConfirmRequired` (or refuses at once with `NotAllowed`, e.g. a conflicted file in the
//! repo); then, only if the user confirms in the dialog, the same command with `"confirm": true`.

use crate::ErrorKind;
use crate::stage::StageItem;
use crate::{Action, Done, Request, Tab};
use project_domain::{Change, RepoPath, Status};
use project_engine::DiscardResult;
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

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
    let mut params = json!({ "paths": paths });
    if confirm {
        params["confirm"] = json!(true);
    }
    params
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
    pub phase: Phase,
}

/// Discarding the changes, from the check to the response. Only one at a time.
#[derive(Debug, Default)]
pub struct DiscardBox {
    current: Option<Discard>,
}

/// What came back from `worktree.discard`.
#[derive(Debug)]
pub enum Received {
    /// Confirmation needed: the dialog opens.
    Confirm,
    /// The changes are discarded.
    Done(Box<DiscardResult>),
    /// Refused or failed: the message to show. Nothing changed, except if the
    /// result cannot be read (then the discard may have happened: the status is reread).
    Failed { message: String, reload: bool },
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
        if self.current.is_some() || paths.is_empty() {
            return None;
        }
        let params = params(&paths, false);
        self.current = Some(Discard {
            paths,
            phase: Phase::Checking,
        });
        Some(params)
    }

    /// The user confirmed: the parameters with `"confirm": true`.
    pub fn confirmed(&mut self) -> Option<Value> {
        let discard = self.current.as_mut()?;
        if !matches!(discard.phase, Phase::Confirm { .. }) {
            return None;
        }
        discard.phase = Phase::Sending;
        Some(params(&discard.paths, true))
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
            };
        };
        let label = describe(&discard.paths);
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && discard.phase == Phase::Checking =>
            {
                self.current = Some(Discard {
                    paths: discard.paths,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                });
                Received::Confirm
            }
            Err(e) if kind == Some(ErrorKind::NotAllowed) => Received::Failed {
                message: tr!("discard-not-allowed", label = &label, error = e),
                reload: false,
            },
            Err(e) => Received::Failed {
                message: tr!("common-failed", label = &label, error = e),
                reload: false,
            },
            Ok(value) => match serde_json::from_value::<DiscardResult>(value) {
                Ok(result) => Received::Done(Box::new(result)),
                Err(e) => Received::Failed {
                    message: crate::unexpected_result(COMMAND, e),
                    reload: true,
                },
            },
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct History {
    pub can_undo: bool,
    pub can_redo: bool,
    #[serde(default, deserialize_with = "crate::engine_text::opt_text")]
    pub next_undo: Option<String>,
    #[serde(default, deserialize_with = "crate::engine_text::opt_text")]
    pub next_redo: Option<String>,
}

impl From<project_engine::JournalState> for History {
    fn from(j: project_engine::JournalState) -> Self {
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

/// The journal entry affected by undo or redo: the description is enough.
#[derive(Debug, Clone, Deserialize)]
pub struct EntryName {
    #[serde(deserialize_with = "crate::engine_text::text")]
    pub description: String,
}

/// Result of `edit.undo`/`edit.redo`.
#[derive(Debug, Clone, Deserialize)]
pub struct UndoResult {
    pub undone: Option<EntryName>,
    pub redone: Option<EntryName>,
    #[serde(default)]
    pub backups: Vec<String>,
    #[serde(default, deserialize_with = "crate::engine_text::texts")]
    pub warnings: Vec<String>,
    pub status: Option<Status>,
    pub status_error: Option<String>,
    #[serde(flatten)]
    pub history: History,
}

impl UndoResult {
    /// The message for the status bar.
    pub fn notice(&self) -> String {
        let mut text = match (&self.undone, &self.redone) {
            (Some(entry), _) => tr!("undo-undone", description = &entry.description),
            (None, Some(entry)) => tr!("undo-redone", description = &entry.description),
            (None, None) => tr!("undo-done"),
        };
        if !self.backups.is_empty() {
            text.push_str(&tr!("undo-backups", refs = self.backups.join(", ")));
        }
        text.push_str(&crate::warnings_text(&self.warnings));
        text
    }
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_discard(&mut self, action: Action) {
        match action {
            Action::Discard(paths) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = describe_check(&paths);
                if let Some(params) = repo.discard.start(paths) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(COMMAND, params, Request::Discard(generation));
                }
            }
            Action::ConfirmDiscard => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let label = repo.discard.confirm().map(|d| describe(&d.paths));
                if let Some(params) = repo.discard.confirmed() {
                    repo.write_label = label;
                    self.submit(COMMAND, params, Request::Discard(generation));
                }
            }
            Action::CancelDiscard => {
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
            _ => unreachable!("action dispatched to the wrong function"),
        }
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
                let notice = notice(&result);
                let DiscardResult {
                    status,
                    status_error,
                    journal,
                    ..
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
            Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                repo.write_error = Some(message.clone());
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
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
                repo.history = result.history.clone();
                if let Some(status) = result.status.clone() {
                    repo.set_status(status);
                }
                self.notice = result.notice();
                self.error = result
                    .status_error
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
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}");
        };
        assert_eq!(
            message,
            "Discarding changes to README.md not possible: operation refused: there are \
             conflicted files"
        );
        assert!(!reload);
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
        let history: History = serde_json::from_value(
            registry
                .run("edit.history", &mut session, json!({}))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(history, History::default());
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
        assert!(result.backup.starts_with("refs/project/backup/discard-"));
        let paths: Vec<&RepoPath> = result.files.iter().map(|f| &f.path).collect();
        assert_eq!(paths, [&RepoPath::from("src/lib.rs")]);
        let notice = notice(&result);
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
        assert!(
            undo.notice().starts_with("Undone: Discard"),
            "{}",
            undo.notice()
        );
        assert!(undo.history.can_redo && !undo.history.can_undo);
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
        assert!(redo.notice().starts_with("Redone: "), "{}", redo.notice());
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
}
