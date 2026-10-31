//! Comparing two revisions (S53): the details panel shows the files that change between two
//! commits (`diff.range`) in place of the selected commit. Data only, no egui: tested with unit
//! tests. Contract in `impl-docs/git/reading.md`, "`diff.range`".
//!
//! - **Opening**: Ctrl+click (Cmd on macOS) on a second commit of the list, or "Compare with
//!   the selected commit" in its menu: the older commit is `from` when one is an ancestor of the
//!   other, otherwise the commit selected first. "Compare with `<current branch>`" in a branch's
//!   menu shows what the branch brings since it diverged from HEAD (`merge_base: true`).
//! - **Requests**: one `diff.range` without `path` for the file list, in the background; every
//!   request has a number and an answer whose number is not the one of the open comparison
//!   (closed, swapped, replaced) is discarded. A file's diff is a [`DiffSource::Range`] on the
//!   resolved ids, through the ordinary diff view.
//! - **Closing**: a plain click or a keyboard move in the list, or Esc (after the diff).

use crate::diff::{DiffSource, DiffTarget};
use crate::{Done, Request, Tab};
use project_domain::{ChangedFile, Commit, FileDiff, Head, Oid};
use project_engine::{DiffRangeParams, DiffRangeResult};
use std::collections::{HashMap, HashSet};

/// The id of the command that compares two revisions.
pub const RANGE: &str = "diff.range";

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Compares the selected commit with the commit at row `index` of the list.
    With(usize),
    /// What branch `rev` brings since it diverged from HEAD; `name` is how it is shown.
    Branch { rev: String, name: String },
    /// Exchanges the two sides.
    Swap,
    /// Asks again after an error.
    Retry,
    /// Back to the selected commit.
    Close,
}

/// One side of a comparison: the revision passed to `diff.range` and how it is shown before
/// the answer arrives (the subject of a commit, the name of a branch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub rev: String,
    pub label: String,
    /// The commit, when known before asking (a commit of the list).
    pub id: Option<Oid>,
}

impl Side {
    pub fn commit(commit: &Commit) -> Self {
        Self {
            rev: commit.id.to_string(),
            label: commit.summary.clone(),
            id: Some(commit.id),
        }
    }

    pub fn rev(rev: String, label: String) -> Self {
        Self {
            rev,
            label,
            id: None,
        }
    }
}

/// What the answer brings: the resolved commits and the changed files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compared {
    pub from: Oid,
    pub to: Oid,
    /// With `merge_base`: the merge base the files are compared with.
    pub base: Option<Oid>,
    pub files: Vec<ChangedFile>,
}

impl Compared {
    /// The diff source of a file of this comparison.
    pub fn source(&self, merge_base: bool) -> DiffSource {
        DiffSource::Range {
            from: self.from,
            to: self.to,
            merge_base,
        }
    }
}

/// The open comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    pub from: Side,
    pub to: Side,
    pub merge_base: bool,
    /// `None` while `diff.range` works.
    pub answer: Option<Result<Compared, String>>,
}

impl Comparison {
    /// True if `target` is the diff of a file of this comparison.
    pub fn owns(&self, target: &DiffTarget) -> bool {
        match (&self.answer, target.source) {
            (Some(Ok(c)), source) => source == c.source(self.merge_base),
            _ => false,
        }
    }
}

/// A request to send.
#[derive(Debug, Clone, PartialEq)]
pub struct Want {
    pub seq: u64,
    pub params: serde_json::Value,
}

#[derive(Debug, Default)]
pub struct CompareBox {
    open: Option<Comparison>,
    /// Number of the request of the open comparison; `sent` once it has gone.
    seq: u64,
    sent: bool,
}

impl CompareBox {
    /// Opens the comparison of `from` and `to` (replacing the one open).
    pub fn open(&mut self, from: Side, to: Side, merge_base: bool) {
        self.open = Some(Comparison {
            from,
            to,
            merge_base,
            answer: None,
        });
        self.ask_again();
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    pub fn view(&self) -> Option<&Comparison> {
        self.open.as_ref()
    }

    /// Exchanges `from` and `to` and asks again.
    pub fn swap(&mut self) {
        if let Some(open) = &mut self.open {
            std::mem::swap(&mut open.from, &mut open.to);
            open.answer = None;
            self.ask_again();
        }
    }

    /// After an error, asks again.
    pub fn retry(&mut self) {
        if let Some(open) = &mut self.open
            && matches!(open.answer, Some(Err(_)))
        {
            open.answer = None;
            self.ask_again();
        }
    }

    fn ask_again(&mut self) {
        self.seq += 1;
        self.sent = false;
    }

    /// The request to send, once per comparison.
    pub fn want(&mut self) -> Option<Want> {
        let open = self.open.as_ref()?;
        if self.sent {
            return None;
        }
        self.sent = true;
        let params = crate::params(DiffRangeParams {
            from: open.from.rev.clone(),
            to: open.to.rev.clone(),
            merge_base: open.merge_base,
            path: None,
        });
        Some(Want {
            seq: self.seq,
            params,
        })
    }

    /// The answer to request `seq`: kept only if it is the one of the open comparison.
    pub fn receive(&mut self, seq: u64, result: Result<DiffRangeResult, String>) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        if seq != self.seq || open.answer.is_some() {
            return false;
        }
        open.answer = Some(result.map(|r| Compared {
            from: r.from,
            to: r.to,
            base: r.base,
            files: r.diff.files.iter().map(changed_file).collect(),
        }));
        true
    }

    /// True if commit `id` is one of the two sides: the list highlights it.
    pub fn marks(&self, id: Oid) -> bool {
        self.open.as_ref().is_some_and(|o| {
            let ids = match &o.answer {
                Some(Ok(c)) => [Some(c.from), Some(c.to)],
                _ => [o.from.id, o.to.id],
            };
            ids.contains(&Some(id))
        })
    }
}

/// The row of the files list for a file of the diff.
pub fn changed_file(file: &FileDiff) -> ChangedFile {
    let zero = Oid::from_bytes([0; 20]);
    let moved = file.old_path != file.new_path;
    ChangedFile {
        path: file.new_path.clone(),
        change: file.change,
        original: moved.then(|| file.old_path.clone()),
        score: file.similarity,
        old_mode: file.old_mode.unwrap_or(0),
        new_mode: file.new_mode.unwrap_or(0),
        old_oid: file.old_oid.unwrap_or(zero),
        new_oid: file.new_oid.unwrap_or(zero),
    }
}

/// The two sides for commits `first` (selected first) and `second` of the list: the older is
/// `from` when one is an ancestor of the other, otherwise `first`.
pub fn order(commits: &[Commit], first: usize, second: usize) -> (usize, usize) {
    if is_ancestor(commits, second, first) {
        (second, first)
    } else {
        (first, second)
    }
}

/// True if `commits[ancestor]` is reachable from `commits[descendant]` through the parents. The
/// list shows children before their parents, so every path between them stays within the rows
/// in between: the loaded commits are enough.
fn is_ancestor(commits: &[Commit], ancestor: usize, descendant: usize) -> bool {
    if ancestor <= descendant || ancestor >= commits.len() {
        return false;
    }
    let rows = &commits[descendant..=ancestor];
    let target = commits[ancestor].id;
    let within: HashMap<Oid, &Commit> = rows.iter().map(|c| (c.id, c)).collect();
    let mut seen = HashSet::new();
    let mut stack = vec![commits[descendant].id];
    while let Some(id) = stack.pop() {
        if id == target {
            return true;
        }
        if !seen.insert(id) {
            continue;
        }
        if let Some(commit) = within.get(&id) {
            stack.extend(commit.parents.iter().copied());
        }
    }
    false
}

impl Tab {
    pub(crate) fn apply_compare(&mut self, action: Action) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        match action {
            Action::With(index) => {
                let commits = repo.list.commits();
                let first = repo.list.selected().filter(|_| !repo.worktree_selected);
                match first {
                    Some(first) if first != index && index < commits.len() => {
                        let (from, to) = order(commits, first, index);
                        let (from, to) = (Side::commit(&commits[from]), Side::commit(&commits[to]));
                        repo.stash.select(None);
                        repo.diff.close();
                        repo.compare.open(from, to, false);
                    }
                    // Nothing else selected: an ordinary selection.
                    _ => {
                        repo.compare.close();
                        repo.stash.select(None);
                        repo.select_worktree(false);
                        repo.list.select(index);
                    }
                }
            }
            Action::Branch { rev, name } => {
                let current = match &repo.head {
                    Head::Branch { name, .. } => name.clone(),
                    Head::Detached(_) => "HEAD".to_owned(),
                };
                repo.stash.select(None);
                repo.select_worktree(false);
                repo.diff.close();
                repo.compare.open(
                    Side::rev("HEAD".to_owned(), current),
                    Side::rev(rev, name),
                    true,
                );
            }
            Action::Swap => {
                repo.diff.close();
                repo.compare.swap();
            }
            Action::Retry => repo.compare.retry(),
            Action::Close => {
                repo.diff.close();
                repo.compare.close();
            }
        }
    }

    /// Sends the request of the open comparison, if needed.
    pub(crate) fn request_compare(&mut self) {
        let generation = self.generation;
        let Some(want) = self.repo.as_mut().and_then(|r| r.compare.want()) else {
            return;
        };
        self.submit(RANGE, want.params, Request::Compare(generation, want.seq));
    }

    pub(crate) fn receive_compare(&mut self, generation: u64, seq: u64, done: Done) {
        let result = done
            .result
            .and_then(|value| crate::decode::<DiffRangeResult>(&done.command, value));
        if let Err(e) = &result {
            log::warn!("{}: {e}", done.command);
        }
        if let Some(repo) = self.current(generation) {
            repo.compare.receive(seq, result);
        }
    }
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
