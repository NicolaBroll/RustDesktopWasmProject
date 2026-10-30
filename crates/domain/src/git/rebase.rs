//! Plan of an interactive rebase: which commits of `base..HEAD` to keep, reword, combine or drop.
//!
//! [`RebasePlan::validate`] checks the plan against the commits of the range,
//! [`RebasePlan::result`] gives the commits it produces (for the `MemoryBackend` and the preview)
//! and [`RebasePlan::todo`] the `git-rebase-todo` text for the `CliBackend`.
//!
//! A *group* is a `pick` or `reword` step followed by the `squash`/`fixup` steps that come after it;
//! `drop` steps do not interrupt a group (the dropped commit simply disappears).

use super::Oid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

/// What to do with a commit, as in the todo of `git rebase -i`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RebaseAction {
    /// Keep the commit with its message.
    Pick,
    /// Keep the commit with a new message ([`RebaseStep::message`], required).
    Reword,
    /// Combine with the previous commit, joining the messages.
    Squash,
    /// Combine with the previous commit, keeping the previous commit's message.
    Fixup,
    /// Remove the commit.
    Drop,
}

impl RebaseAction {
    /// True for `squash` and `fixup`.
    pub fn is_combine(self) -> bool {
        matches!(self, RebaseAction::Squash | RebaseAction::Fixup)
    }
}

/// One step of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RebaseStep {
    pub action: RebaseAction,
    pub commit: Oid,
    /// New message: required for `reword`, allowed only on the last `squash`/`fixup` of a group
    /// (the message of the combined commit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The plan: `steps` in the order of application (oldest first, like the git todo), covering
/// exactly the commits of `base..HEAD`; `base` itself is not rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RebasePlan {
    pub base: Oid,
    pub steps: Vec<RebaseStep>,
}

/// A commit produced by the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebasedCommit {
    /// `sources[0]` is the picked or reworded commit, then those squashed or fixed up into it.
    pub sources: Vec<Oid>,
    /// `None` = git's default: the original message for a `pick`, the combination of the messages
    /// for a group with `squash`, the first commit's message for a group with only `fixup`.
    pub message: Option<String>,
}

/// The `git-rebase-todo` text and the messages to apply at its `break` stops.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RebaseTodo {
    /// One instruction per line, each terminated by `\n`.
    pub text: String,
    /// One message per `break` line, in the same order: applied with `commit --amend`.
    pub messages: Vec<String>,
}

/// Why a plan is not valid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanErrorReason {
    #[error("commit {0} is not in the range being rebased")]
    UnknownCommit(Oid),
    #[error("commit {0} appears more than once")]
    DuplicateCommit(Oid),
    #[error("commit {0} of the range is missing from the plan (use `drop` to remove it)")]
    MissingCommit(Oid),
    #[error("every commit is dropped")]
    AllDropped,
    #[error(
        "the first commit kept cannot be squashed or fixed up: there is nothing to combine it with"
    )]
    NothingToCombine,
    #[error("`reword` needs a message")]
    MissingMessage,
    #[error("a message is allowed only on `reword` and on the last `squash`/`fixup` of a group")]
    UnexpectedMessage,
    #[error(
        "`reword` cannot start a group with `squash`/`fixup`: use `pick` and put the message on the last step of the group"
    )]
    RewordInGroup,
    #[error("the message is empty")]
    EmptyMessage,
    #[error("the message contains a NUL byte")]
    NulInMessage,
}

/// Error of [`RebasePlan::validate`]: the step (0-based index in `steps`) and the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    /// `None` when the error concerns the plan as a whole (missing commit, all dropped).
    pub step: Option<usize>,
    pub reason: PlanErrorReason,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.step {
            // 1-based for people.
            Some(i) => write!(f, "step {}: {}", i + 1, self.reason),
            None => write!(f, "{}", self.reason),
        }
    }
}

impl std::error::Error for PlanError {}

impl RebasePlan {
    /// The default plan: `pick` of every commit (`commits` = `base..HEAD`, oldest first).
    pub fn pick_all(base: Oid, commits: &[Oid]) -> Self {
        let steps = commits
            .iter()
            .map(|&commit| RebaseStep {
                action: RebaseAction::Pick,
                commit,
                message: None,
            })
            .collect();
        Self { base, steps }
    }

    /// Checks the plan against `commits` (`base..HEAD`, oldest first): every commit exactly once,
    /// at least one step that is not `drop`, the first kept step not `squash`/`fixup`, messages only
    /// where allowed (required for `reword`), not blank and without NUL.
    pub fn validate(&self, commits: &[Oid]) -> Result<(), PlanError> {
        let err = |step, reason| PlanError { step, reason };
        let range: HashSet<Oid> = commits.iter().copied().collect();
        let mut seen = HashSet::new();
        for (i, step) in self.steps.iter().enumerate() {
            if !range.contains(&step.commit) {
                return Err(err(Some(i), PlanErrorReason::UnknownCommit(step.commit)));
            }
            if !seen.insert(step.commit) {
                return Err(err(Some(i), PlanErrorReason::DuplicateCommit(step.commit)));
            }
        }
        if let Some(&missing) = commits.iter().find(|c| !seen.contains(c)) {
            return Err(err(None, PlanErrorReason::MissingCommit(missing)));
        }
        let Some(first) = self.kept().next() else {
            return Err(err(None, PlanErrorReason::AllDropped));
        };
        if self.steps[first].action.is_combine() {
            return Err(err(Some(first), PlanErrorReason::NothingToCombine));
        }
        for (i, step) in self.steps.iter().enumerate() {
            let next_combines = self
                .next_kept(i)
                .is_some_and(|n| self.steps[n].action.is_combine());
            let allowed = match step.action {
                RebaseAction::Reword => {
                    if next_combines {
                        return Err(err(Some(i), PlanErrorReason::RewordInGroup));
                    }
                    if step.message.is_none() {
                        return Err(err(Some(i), PlanErrorReason::MissingMessage));
                    }
                    true
                }
                RebaseAction::Squash | RebaseAction::Fixup => !next_combines,
                RebaseAction::Pick | RebaseAction::Drop => false,
            };
            if let Some(message) = &step.message {
                if !allowed {
                    return Err(err(Some(i), PlanErrorReason::UnexpectedMessage));
                }
                if message.trim().is_empty() {
                    return Err(err(Some(i), PlanErrorReason::EmptyMessage));
                }
                if message.contains('\0') {
                    return Err(err(Some(i), PlanErrorReason::NulInMessage));
                }
            }
        }
        Ok(())
    }

    /// True if the plan changes nothing: all `pick`, in the order of `commits`, without messages.
    pub fn is_noop(&self, commits: &[Oid]) -> bool {
        self.steps.len() == commits.len()
            && self.steps.iter().zip(commits).all(|(s, c)| {
                s.action == RebaseAction::Pick && s.commit == *c && s.message.is_none()
            })
    }

    /// The commits the plan produces, in order. Assumes a plan that passed [`Self::validate`].
    pub fn result(&self) -> Vec<RebasedCommit> {
        let mut out: Vec<RebasedCommit> = Vec::new();
        for step in &self.steps {
            match step.action {
                RebaseAction::Drop => {}
                RebaseAction::Pick | RebaseAction::Reword => out.push(RebasedCommit {
                    sources: vec![step.commit],
                    message: step.message.clone(),
                }),
                RebaseAction::Squash | RebaseAction::Fixup => {
                    if let Some(group) = out.last_mut() {
                        group.sources.push(step.commit);
                        if step.message.is_some() {
                            group.message = step.message.clone();
                        }
                    }
                }
            }
        }
        out
    }

    /// The `git-rebase-todo` text: `pick|squash|fixup <full oid>` in the order of the plan, a
    /// `break` after each `reword` (written as `pick`) and after each `squash`/`fixup` with a
    /// message; the `drop` lines all at the end, so they never sit inside a group.
    pub fn todo(&self) -> RebaseTodo {
        let mut text = String::new();
        let mut messages = Vec::new();
        for step in &self.steps {
            let word = match step.action {
                RebaseAction::Pick | RebaseAction::Reword => "pick",
                RebaseAction::Squash => "squash",
                RebaseAction::Fixup => "fixup",
                RebaseAction::Drop => continue,
            };
            text.push_str(&format!("{word} {}\n", step.commit));
            if let Some(message) = &step.message {
                text.push_str("break\n");
                messages.push(message.clone());
            }
        }
        for step in self.steps.iter().filter(|s| s.action == RebaseAction::Drop) {
            text.push_str(&format!("drop {}\n", step.commit));
        }
        RebaseTodo { text, messages }
    }

    /// Indices of the steps that are not `drop`.
    fn kept(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.steps.len()).filter(|&i| self.steps[i].action != RebaseAction::Drop)
    }

    /// Index of the first step after `i` that is not `drop`.
    fn next_kept(&self, i: usize) -> Option<usize> {
        self.kept().find(|&n| n > i)
    }
}
