//! Deleting untracked files (`worktree.clean`). Data only, no egui: tested with unit tests.
//!
//! As for discarding, two steps: first `worktree.clean` without `confirm`, which changes nothing and
//! answers with `ConfirmRequired` (with the total size) or refuses at once with `NotAllowed`;
//! then, only if the user confirms in the dialog, the same command with `"confirm": true`. The UI
//! always sends explicit files (the status lists every untracked file), so the dialog lists
//! exactly what is deleted.

use crate::ErrorKind;
use crate::stage::StageItem;
use crate::{Done, Request, Tab};
use project_domain::{Change, RepoPath};
use project_engine::{CleanParams, CleanResult};
use project_i18n::tr;
use serde_json::{Value, json};

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Deletes the given untracked files (never empty): first the engine checks and asks for
    /// confirmation, which opens in a dialog.
    Delete(Vec<RepoPath>),
    /// Confirms the pending deletion: resends with `"confirm": true`.
    Confirm,
    /// Closes the dialog without deleting anything.
    Cancel,
}

/// The id of the Registry command.
pub const COMMAND: &str = "worktree.clean";

/// True if the file can be deleted from the stage area: untracked (tracked files are
/// discarded, never deleted).
pub fn deletable(item: &StageItem) -> bool {
    item.change == Change::Untracked && !item.is_conflict()
}

/// The paths for "Delete all untracked files": every untracked file of the list, always
/// explicit; `None` if there are none.
pub fn untracked_paths(items: &[StageItem]) -> Option<Vec<RepoPath>> {
    let mut paths: Vec<RepoPath> = Vec::new();
    for item in items.iter().filter(|item| deletable(item)) {
        if !paths.contains(&item.path) {
            paths.push(item.path.clone());
        }
    }
    (!paths.is_empty()).then_some(paths)
}

/// Name of the operation for messages, e.g. "Deleting notes.txt".
pub fn describe(paths: &[RepoPath]) -> String {
    match paths {
        [one] => tr!("clean-label-one", path = one.to_string_lossy().into_owned()),
        many => tr!("clean-label-many", n = many.len()),
    }
}

/// The check before deleting.
pub fn describe_check(paths: &[RepoPath]) -> String {
    match paths {
        [one] => tr!(
            "clean-check-label-one",
            path = one.to_string_lossy().into_owned()
        ),
        many => tr!("clean-check-label-many", n = many.len()),
    }
}

/// A size for people: "512 bytes", "1.5 KB", "2.0 MB" (powers of 1024).
pub fn size_text(bytes: u64) -> String {
    const UNIT: f64 = 1024.0;
    let b = bytes as f64;
    if b < UNIT {
        tr!("clean-size-bytes", n = bytes)
    } else if b < UNIT * UNIT {
        tr!("clean-size-kb", n = format!("{:.1}", b / UNIT))
    } else if b < UNIT * UNIT * UNIT {
        tr!("clean-size-mb", n = format!("{:.1}", b / UNIT / UNIT))
    } else {
        tr!(
            "clean-size-gb",
            n = format!("{:.1}", b / UNIT / UNIT / UNIT)
        )
    }
}

/// The parameters of `worktree.clean`; `confirm` only after the user's confirmation.
pub fn params(paths: &[RepoPath], confirm: bool) -> Value {
    crate::params(CleanParams {
        paths: paths.to_vec(),
        confirm,
    })
}

/// Where the deletion stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Sent without `confirm`: the engine checks and answers with the confirmation request.
    Checking,
    /// The confirmation dialog is open; `bytes` is the total size the engine counted.
    Confirm { bytes: Option<u64> },
    /// Sent with `"confirm": true`.
    Sending,
}

/// A deletion in progress or to confirm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clean {
    pub paths: Vec<RepoPath>,
    pub phase: Phase,
}

/// Deleting untracked files, from the check to the response. Only one at a time.
#[derive(Debug, Default)]
pub struct CleanBox {
    current: Option<Clean>,
}

/// What came back from `worktree.clean`.
#[derive(Debug)]
pub enum Received {
    /// Confirmation needed: the dialog opens.
    Confirm,
    /// The files are deleted.
    Done(Box<CleanResult>),
    /// Refused or failed: the message to show. Nothing changed, except if the result cannot be
    /// read (then the deletion may have happened: the status is reread).
    Failed { message: String, reload: bool },
}

impl CleanBox {
    #[cfg(test)]
    pub fn current(&self) -> Option<&Clean> {
        self.current.as_ref()
    }

    /// The deletion awaiting confirmation, if the dialog is open.
    pub fn confirm(&self) -> Option<&Clean> {
        self.current
            .as_ref()
            .filter(|c| matches!(c.phase, Phase::Confirm { .. }))
    }

    /// Starts a deletion: the parameters of the check (without `confirm`). `None` if there is
    /// already one or if there is nothing to delete.
    pub fn start(&mut self, paths: Vec<RepoPath>) -> Option<Value> {
        if self.current.is_some() || paths.is_empty() {
            return None;
        }
        let params = params(&paths, false);
        self.current = Some(Clean {
            paths,
            phase: Phase::Checking,
        });
        Some(params)
    }

    /// The user confirmed: the parameters with `"confirm": true`.
    pub fn confirmed(&mut self) -> Option<Value> {
        let clean = self.current.as_mut()?;
        if !matches!(clean.phase, Phase::Confirm { .. }) {
            return None;
        }
        clean.phase = Phase::Sending;
        Some(params(&clean.paths, true))
    }

    /// Closes the dialog without deleting anything.
    pub fn cancel(&mut self) {
        if self.confirm().is_some() {
            self.current = None;
        }
    }

    /// The response of `worktree.clean`, in the phase it was in.
    pub fn receive(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        bytes: Option<u64>,
    ) -> Received {
        let Some(clean) = self.current.take() else {
            return Received::Failed {
                message: tr!("common-unexpected-response", command = COMMAND),
                reload: true,
            };
        };
        let label = describe(&clean.paths);
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && clean.phase == Phase::Checking =>
            {
                self.current = Some(Clean {
                    paths: clean.paths,
                    phase: Phase::Confirm { bytes },
                });
                Received::Confirm
            }
            Err(e) if kind == Some(ErrorKind::NotAllowed) => Received::Failed {
                message: tr!("clean-not-allowed", label = &label, error = e),
                reload: false,
            },
            Err(e) => Received::Failed {
                message: tr!("common-failed", label = &label, error = e),
                reload: false,
            },
            Ok(value) => match serde_json::from_value::<CleanResult>(value) {
                Ok(result) => Received::Done(Box::new(result)),
                Err(e) => Received::Failed {
                    message: crate::unexpected_result(COMMAND, e),
                    reload: true,
                },
            },
        }
    }
}

/// The message for the status bar after `worktree.clean`, with the files that could not be
/// deleted.
pub fn notice(result: &CleanResult) -> String {
    let mut text = match result.deleted.as_slice() {
        [one] => tr!(
            "clean-done-one",
            path = one.to_string_lossy().into_owned(),
            backup = &result.backup
        ),
        many => tr!("clean-done-many", n = many.len(), backup = &result.backup),
    };
    text.push_str(&crate::messages_text(&result.warnings));
    text
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_clean(&mut self, action: Action) {
        match action {
            Action::Delete(paths) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = describe_check(&paths);
                if let Some(params) = repo.clean.start(paths) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(COMMAND, params, Request::Clean(generation));
                }
            }
            Action::Confirm => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let label = repo.clean.confirm().map(|c| describe(&c.paths));
                if let Some(params) = repo.clean.confirmed() {
                    repo.write_label = label;
                    self.submit(COMMAND, params, Request::Clean(generation));
                }
            }
            Action::Cancel => {
                if let Some(repo) = &mut self.repo {
                    repo.clean.cancel();
                }
            }
        }
    }

    /// Response of `worktree.clean`: the check opens the confirmation dialog, the completed
    /// deletion brings the new status and the Undo button; a refusal goes in the stage area.
    pub(crate) fn receive_clean(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .clean
            .receive(done.result, done.error_kind, done.confirm_bytes)
        {
            Received::Confirm => {}
            Received::Done(result) => {
                let notice = notice(&result);
                let CleanResult {
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
                        self.error = tr!("clean-status-not-reread", reason = reason);
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
}

#[cfg(test)]
#[path = "clean_tests.rs"]
mod tests;
