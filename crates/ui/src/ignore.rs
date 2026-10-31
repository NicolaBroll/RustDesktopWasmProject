//! Ignoring untracked files (`worktree.ignore`, `impl-docs/git/ignore.md`). Data only, no egui:
//! tested with unit tests.
//!
//! The "Ignore" submenu of an untracked file offers one pattern per choice ("This file", "All
//! *.ext files", "Folder dir/" for each parent folder), built by
//! [`ignore_pattern`], in the root `.gitignore` (shared once
//! committed) or in `.git/info/exclude` (only this clone). No confirmation: nothing is lost, the
//! engine keeps the old bytes and "Undo" puts them back.

use crate::ErrorKind;
use crate::stage::StageItem;
use crate::{Done, Request, Tab};
use project_domain::{Change, IgnoreKind, IgnorePattern, RepoPath, ignore_pattern};
use project_engine::{IgnoreFile, IgnoreParams, IgnoreResult};
use project_i18n::tr;
use serde_json::{Value, json};

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Appends the pattern to the file.
    Ignore(IgnorePattern, IgnoreFile),
}

/// The id of the Registry command.
pub const COMMAND: &str = "worktree.ignore";

/// True if the file can be ignored from the stage area: untracked (ignoring a tracked file does
/// not stop git from tracking it).
pub fn ignorable(item: &StageItem) -> bool {
    item.change == Change::Untracked && !item.is_conflict()
}

/// One entry of the "Ignore" submenu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// The text of the entry: "This file", "All *.log files", "Folder build/".
    pub label: String,
    /// The pattern appended to the file.
    pub pattern: IgnorePattern,
}

/// The choices for `path`: the file, its extension (if it has one), then each parent folder from
/// the closest to the root.
pub fn choices(path: &RepoPath) -> Vec<Choice> {
    let mut out = Vec::new();
    if let Some(pattern) = ignore_pattern(path, IgnoreKind::Path) {
        out.push(Choice {
            label: tr!("ignore-this-file"),
            pattern,
        });
    }
    if let Some(pattern) = ignore_pattern(path, IgnoreKind::Extension) {
        out.push(Choice {
            label: tr!("ignore-extension", pattern = pattern.to_string()),
            pattern,
        });
    }
    let bytes = path.as_bytes();
    for (i, _) in bytes.iter().enumerate().rev().filter(|(_, b)| **b == b'/') {
        let folder = RepoPath::from_bytes(&bytes[..i]);
        if let Some(pattern) = ignore_pattern(&folder, IgnoreKind::Folder) {
            out.push(Choice {
                label: tr!(
                    "ignore-folder",
                    folder = folder.to_string_lossy().into_owned()
                ),
                pattern,
            });
        }
    }
    out
}

/// The hover text of an entry: the exact line and the file it goes into.
pub fn help(pattern: &IgnorePattern, file: IgnoreFile) -> String {
    tr!(
        "ignore-entry-help",
        pattern = pattern.to_string(),
        file = file.label()
    )
}

/// Name of the operation for messages, e.g. "Ignoring “*.log” in .gitignore".
pub fn describe(pattern: &IgnorePattern, file: IgnoreFile) -> String {
    tr!(
        "ignore-label",
        pattern = pattern.to_string(),
        file = file.label()
    )
}

/// The parameters of `worktree.ignore`.
pub fn params(pattern: &IgnorePattern, file: IgnoreFile) -> Value {
    crate::params(IgnoreParams {
        patterns: vec![pattern.clone()],
        file,
    })
}

/// An ignore in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ignore {
    pub pattern: IgnorePattern,
    pub file: IgnoreFile,
}

/// The ignore sent, until its response. Only one at a time.
#[derive(Debug, Default)]
pub struct IgnoreBox {
    current: Option<Ignore>,
}

/// What came back from `worktree.ignore`.
#[derive(Debug)]
pub enum Received {
    /// The pattern is in the file (or was already there).
    Done(Box<IgnoreResult>),
    /// Refused or failed: the message to show. Nothing changed, except if the result cannot be
    /// read (then the file may have been written: the status is reread).
    Failed { message: String, reload: bool },
}

impl IgnoreBox {
    #[cfg(test)]
    pub fn current(&self) -> Option<&Ignore> {
        self.current.as_ref()
    }

    /// Starts an ignore: its parameters, `None` if one is already in progress.
    pub fn start(&mut self, pattern: IgnorePattern, file: IgnoreFile) -> Option<Value> {
        if self.current.is_some() {
            return None;
        }
        let params = params(&pattern, file);
        self.current = Some(Ignore { pattern, file });
        Some(params)
    }

    /// The response of `worktree.ignore`.
    pub fn receive(&mut self, result: Result<Value, String>, kind: Option<ErrorKind>) -> Received {
        let Some(ignore) = self.current.take() else {
            return Received::Failed {
                message: tr!("common-unexpected-response", command = COMMAND),
                reload: true,
            };
        };
        let label = describe(&ignore.pattern, ignore.file);
        match result {
            Err(e) if kind == Some(ErrorKind::NotAllowed) => Received::Failed {
                message: tr!("ignore-not-allowed", label = &label, error = e),
                reload: false,
            },
            Err(e) => Received::Failed {
                message: tr!("common-failed", label = &label, error = e),
                reload: false,
            },
            Ok(value) => match serde_json::from_value::<IgnoreResult>(value) {
                Ok(result) => Received::Done(Box::new(result)),
                Err(e) => Received::Failed {
                    message: crate::unexpected_result(COMMAND, e),
                    reload: true,
                },
            },
        }
    }
}

/// The message for the status bar after `worktree.ignore`: what was added and where, how many
/// files became ignored, the warnings (tracked files matched).
pub fn notice(result: &IgnoreResult) -> String {
    let mut text = match result.added.as_slice() {
        [] => {
            let patterns: Vec<String> = result.already.iter().map(|p| p.to_string()).collect();
            tr!(
                "ignore-done-already",
                pattern = patterns.join(", "),
                file = &result.label
            )
        }
        added => {
            let patterns: Vec<String> = added.iter().map(|p| p.to_string()).collect();
            tr!(
                "ignore-done",
                pattern = patterns.join(", "),
                file = &result.label,
                n = result.ignored.len()
            )
        }
    };
    text.push_str(&crate::messages_text(&result.warnings));
    text
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_ignore(&mut self, action: Action) {
        match action {
            Action::Ignore(pattern, file) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = describe(&pattern, file);
                if let Some(params) = repo.ignore.start(pattern, file) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(COMMAND, params, Request::Ignore(generation));
                }
            }
        }
    }

    /// Response of `worktree.ignore`: the new status (the ignored files leave the list, the
    /// `.gitignore` change appears) and the Undo button; a refusal goes in the stage area.
    pub(crate) fn receive_ignore(&mut self, generation: u64, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo.ignore.receive(done.result, done.error_kind) {
            Received::Done(result) => {
                let notice = notice(&result);
                let IgnoreResult {
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
                        self.error = tr!("ignore-status-not-reread", reason = reason);
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
#[path = "ignore_tests.rs"]
mod tests;
