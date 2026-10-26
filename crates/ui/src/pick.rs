//! Cherry-pick and revert (S31) from the commit menu: `commit.cherry_pick` and `commit.revert`.
//! Data only, no egui. Command contract in `impl-docs/git/cherry-pick.md`.
//!
//! It is a part of [`MergeBox`]: the start, the window of a refusal and the panel
//! of the operation in progress are those of the merge. For a merge commit the
//! parent (`mainline`) is chosen first in a small dialog; for the others it starts at once. Stopped
//! on a conflict, the cherry-pick or revert **stays in progress** and is finished or aborted from the
//! panel with `cherry_pick.continue`/`revert.continue` and `*.abort`, like the merge.

use super::{
    Dialog, ErrorKind, Failure, History, Kind, MergeBox, Op, Outcome, Received, Starting,
    reread_warning, unexpected,
};
use project_domain::{Oid, RefInfo, RepoPath, Status};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// Cherry-pick or revert of a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    CherryPick,
    Revert,
}

impl Pick {
    /// The id of the Registry command.
    pub fn command(self) -> &'static str {
        match self {
            Pick::CherryPick => "commit.cherry_pick",
            Pick::Revert => "commit.revert",
        }
    }

    /// The operation that stays in progress if it stops.
    pub fn kind(self) -> Kind {
        match self {
            Pick::CherryPick => Kind::CherryPick,
            Pick::Revert => Kind::Revert,
        }
    }
}

/// The dialog that asks from which parent of a merge commit to take the changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MainlineDialog {
    pub pick: Pick,
    /// The merge commit.
    pub rev: Oid,
    /// How to show it.
    pub what: String,
    /// The active branch, onto which it is applied.
    pub into: String,
    /// The parents in git's order: id, name (branch or abbreviated id), subject if known.
    pub parents: Vec<(Oid, String, Option<String>)>,
    /// The chosen parent, from 1.
    pub mainline: usize,
}

impl MergeBox {
    /// Starts the cherry-pick or revert of `rev` (shown as `what`) onto branch `into`.
    /// With more than one parent it opens the `mainline` dialog and gives `None`; otherwise
    /// parameters and description. `None` also if a command is in progress or a dialog is open.
    pub fn start_pick(
        &mut self,
        pick: Pick,
        rev: Oid,
        what: String,
        into: String,
        parents: Vec<(Oid, String, Option<String>)>,
    ) -> Option<(Value, String)> {
        if self.busy() || self.dialog.is_some() {
            return None;
        }
        if parents.len() > 1 {
            self.failure = None;
            self.dialog = Some(Dialog::Mainline(MainlineDialog {
                pick,
                rev,
                what,
                into,
                parents,
                mainline: 1,
            }));
            return None;
        }
        Some(self.begin_pick(pick, rev, what, into, None))
    }

    /// The parent chosen in the `mainline` dialog (from 1).
    pub fn set_mainline(&mut self, mainline: usize) {
        if let Some(Dialog::Mainline(dialog)) = &mut self.dialog
            && (1..=dialog.parents.len()).contains(&mainline)
        {
            dialog.mainline = mainline;
        }
    }

    /// Submit of the `mainline` dialog: command, parameters and description.
    pub fn submit_mainline(&mut self) -> Option<(Op, Value, String)> {
        let Some(Dialog::Mainline(dialog)) = self.dialog.take() else {
            return None;
        };
        let pick = dialog.pick;
        let (params, label) = self.begin_pick(
            pick,
            dialog.rev,
            dialog.what,
            dialog.into,
            Some(dialog.mainline),
        );
        Some((Op::Pick(pick), params, label))
    }

    fn begin_pick(
        &mut self,
        pick: Pick,
        rev: Oid,
        what: String,
        into: String,
        mainline: Option<usize>,
    ) -> (Value, String) {
        let mut params = json!({ "rev": rev.to_string() });
        if let Some(mainline) = mainline {
            params["mainline"] = json!(mainline);
        }
        let label = tr!(
            "merge-pick-label",
            title = pick.kind().title(),
            what = &what,
            into = &into
        );
        self.failure = None;
        self.starting = Some(Starting { what, into });
        (params, label)
    }

    pub(super) fn receive_pick(
        &mut self,
        pick: Pick,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(starting) = self.starting.take() else {
            return unexpected(Op::Pick(pick));
        };
        let title = pick.kind().title();
        let what = tr!(
            "merge-pick-label",
            title = title,
            what = &starting.what,
            into = &starting.into
        );
        match result {
            Err(e) => {
                // Engine refusals (local changes, already applied, detached HEAD, mainline):
                // nothing changed. Other errors (git): maybe, so the state is reread.
                let refused = matches!(
                    kind,
                    Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams | ErrorKind::UnknownRev)
                );
                let (window, explanation) = if refused {
                    (
                        tr!("merge-refused-title", title = title),
                        tr!("merge-refused", what = &what, error = &e),
                    )
                } else {
                    (
                        tr!("merge-failed-title", title = title),
                        tr!("merge-failed", what = &what, error = &e),
                    )
                };
                let git = detail.filter(|d| kind == Some(ErrorKind::Git) && !d.trim().is_empty());
                self.failure = Some(Failure {
                    title: window,
                    explanation: explanation.clone(),
                    git,
                });
                Received::Failed {
                    message: explanation,
                    reload: !refused,
                }
            }
            Ok(value) => match crate::decode::<PickResult>(Op::Pick(pick).command(), value) {
                Ok(result) => {
                    let warning = reread_warning(
                        &tr!("merge-done", title = title),
                        &result.warnings,
                        result.status_error.clone(),
                        result.refs_error.clone(),
                    );
                    Received::Done(Box::new(Outcome {
                        notice: result.notice(pick, &starting),
                        moved: result.after != result.before,
                        show_changes: result.in_progress && !result.conflicts.is_empty(),
                        status: result.status,
                        refs: result.refs,
                        history: result.history,
                        warning,
                    }))
                }
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }
}

/// Result of `commit.cherry_pick` and `commit.revert`.
#[derive(Debug, Clone, Deserialize)]
struct PickResult {
    branch: String,
    before: Oid,
    after: Oid,
    in_progress: bool,
    #[serde(default)]
    conflicts: Vec<RepoPath>,
    #[serde(default, deserialize_with = "crate::engine_text::texts")]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

impl PickResult {
    /// The message for the status bar.
    fn notice(&self, pick: Pick, starting: &Starting) -> String {
        let kind = pick.kind();
        let title = kind.title();
        let name = kind.name();
        let what = &starting.what;
        let branch = self
            .branch
            .strip_prefix("refs/heads/")
            .unwrap_or(&self.branch);
        if self.in_progress {
            return match self.conflicts.len() {
                0 => tr!(
                    "merge-pick-stopped-before-commit",
                    title = title,
                    what = what,
                    branch = branch,
                    name = name
                ),
                n => tr!(
                    "merge-pick-stopped",
                    title = title,
                    what = what,
                    branch = branch,
                    files = tr!("merge-conflict-files", n = n),
                    name = name
                ),
            };
        }
        let id = self.after.short();
        match pick {
            Pick::CherryPick => {
                tr!("merge-cherry-picked", what = what, branch = branch, id = id)
            }
            Pick::Revert => tr!("merge-reverted", what = what, branch = branch, id = id),
        }
    }
}

#[cfg(test)]
#[path = "pick_tests.rs"]
mod tests;
