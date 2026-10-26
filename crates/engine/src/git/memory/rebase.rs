//! Interactive rebase on the [`MemoryBackend`] (`rebase.start`, `rebase.continue`,
//! `rebase.abort`, S32) and the rebase of the pull, with the same checks as the `CliBackend`.
//!
//! Like the in-memory cherry-pick, file by file: each commit of the plan is applied on top of
//! the commit reached so far (base = its parent, theirs = the commit), a file changed differently
//! is a conflict. HEAD is detached during the rebase, as in git, and the branch moves only at the
//! end. A conflict **stops the rebase in progress** (`u` entries with markers, the remaining
//! steps in [`MemoryRebase`]); `rebase.continue` commits the resolution (the index) and goes on;
//! `rebase.abort` puts the branch, index and worktree back. A `squash`/`fixup` rebuilds the
//! commit of its group on the group's parent; the message of a group is the one of the plan,
//! otherwise git's default (the messages of `pick` and `squash` joined by a blank line).

use super::commit::Tree;
use super::merge::merge_partial;
use super::{HEADS, HeadRef, MemoryBackend};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, GitBackend, MergeAborted, MergeOutcome, PickKind, RebaseRecord, RebaseRequest,
    check_rebase_request, check_rebase_state, cleanup_message, pick_message, split_message,
};
use project_domain::{Commit, Oid, Operation, RebaseAction, RebasePlan, Signature};

/// The rebase in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MemoryRebase {
    pub(super) record: RebaseRecord,
    /// The steps still to apply; with `stopped` the first is the one stopped on.
    ops: Vec<RebaseOp>,
    /// The parent of the commit of the current group: `squash`/`fixup` rebuild it there.
    group_parent: Oid,
    /// The pull (git's default `--empty=drop`): a commit whose changes are already there is
    /// dropped. The interactive rebase keeps it (`--empty=keep`).
    drop_empty: bool,
    /// True if stopped on a conflict of `ops[0]`.
    stopped: bool,
}

/// One commit to apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RebaseOp {
    source: Oid,
    /// The first commit of its group (a `pick`/`reword`): a new commit; otherwise the group's
    /// commit is rebuilt.
    first: bool,
    /// The message of the group's commit.
    message: String,
    /// The author of the group's commit (that of its first commit).
    author: Signature,
}

impl MemoryBackend {
    /// The commits to apply for `plan`, one per step that is not `drop`.
    fn plan_ops(&self, plan: &RebasePlan) -> Result<Vec<RebaseOp>, EngineError> {
        let commit = |id: &Oid| -> Result<&Commit, EngineError> {
            self.commits
                .get(id)
                .ok_or_else(|| EngineError::UnknownRev(id.to_string()))
        };
        let mut ops = Vec::new();
        let kept: Vec<_> = plan
            .steps
            .iter()
            .filter(|s| s.action != RebaseAction::Drop)
            .collect();
        let mut i = 0;
        while i < kept.len() {
            let start = i;
            i += 1;
            while i < kept.len() && kept[i].action.is_combine() {
                i += 1;
            }
            let group = &kept[start..i];
            let first = commit(&group[0].commit)?;
            let explicit = group.iter().rev().find_map(|s| s.message.clone());
            let message = match explicit {
                Some(m) => m,
                None => {
                    let mut parts = Vec::new();
                    for s in group {
                        if s.action != RebaseAction::Fixup {
                            let c = commit(&s.commit)?;
                            parts.push(pick_message(PickKind::CherryPick, c, None, false));
                        }
                    }
                    parts.join("\n\n")
                }
            };
            for (n, s) in group.iter().enumerate() {
                ops.push(RebaseOp {
                    source: s.commit,
                    first: n == 0,
                    message: message.clone(),
                    author: first.author.clone(),
                });
            }
        }
        Ok(ops)
    }

    /// HEAD detached on `target`, index and worktree from a clean state to its tree.
    fn detach_to(&mut self, target: Oid) {
        let tree = self.tree_at(target);
        let (differing, _) = self.switch_plan(&tree);
        let index = self.switch_index(&differing, &tree);
        self.head = HeadRef::Detached(target);
        self.rebase_entries(&index);
    }

    /// The files that the commits `ids` (and the trees in between) would write in place of an
    /// untracked or ignored file: the rebase goes through all of them.
    pub(super) fn rebase_blocked(&self, ids: impl IntoIterator<Item = Oid>) -> Vec<String> {
        let mut hit: Vec<String> = Vec::new();
        for id in ids {
            let (_, files) = self.switch_plan(&self.tree_at(id));
            for file in files {
                if !hit.contains(&file) {
                    hit.push(file);
                }
            }
        }
        hit
    }

    /// [`GitBackend::rebase`].
    pub(super) fn rebase_start(
        &mut self,
        request: &RebaseRequest,
    ) -> Result<MergeOutcome, EngineError> {
        check_rebase_state(&self.status()?)?;
        if self.head != HeadRef::Branch(request.branch.clone()) {
            return Err(EngineError::NotAllowed(msg!("engine-rebase-head-moved")));
        }
        let full = format!("{HEADS}{}", request.branch);
        self.check_value(&full, &request.expected)?;
        check_rebase_request(&*self, request)?;
        let blocked =
            self.rebase_blocked(std::iter::once(request.plan.base).chain(request.commits.clone()));
        if !blocked.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-rebase-untracked",
                files = crate::quoted_list(&blocked)
            )));
        }
        let ops = self.plan_ops(&request.plan)?;
        let mut work = self.clone();
        let backup = work.rebase_backup("rebase", request.expected);
        let state = MemoryRebase {
            record: request.record(&backup),
            ops,
            group_parent: request.plan.base,
            drop_empty: false,
            stopped: false,
        };
        work.detach_to(request.plan.base);
        let mut outcome = work.rebase_run(state)?;
        outcome.backup = backup;
        if !outcome.in_progress {
            outcome.after = work.head_target().unwrap_or(request.expected);
        } else {
            outcome.after = request.expected;
        }
        *self = work;
        Ok(outcome)
    }

    /// The rebase of the pull: the commits `todo` (oldest first) replayed on `onto` with their
    /// message and author, those already upstream dropped. Stopped on a conflict it stays in
    /// progress like `rebase.start`. To call on a copy, after the checks; `record` is the pull's.
    pub(super) fn pull_rebase(
        &mut self,
        todo: &[Commit],
        record: RebaseRecord,
    ) -> Result<MergeOutcome, EngineError> {
        let ops = todo
            .iter()
            .map(|c| RebaseOp {
                source: c.id,
                first: true,
                message: pick_message(PickKind::CherryPick, c, None, false),
                author: c.author.clone(),
            })
            .collect();
        let onto = record.onto;
        let state = MemoryRebase {
            record,
            ops,
            group_parent: onto,
            drop_empty: true,
            stopped: false,
        };
        self.detach_to(onto);
        self.rebase_run(state)
    }

    /// The backup `refs/project/backup/<what>-<n>` of `target`.
    pub(super) fn rebase_backup(&mut self, what: &str, target: Oid) -> String {
        let prefix = format!("{BACKUP_PREFIX}{what}-");
        let n = self
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix) && !b.starts_with(&format!("{prefix}abort-")))
            .count()
            + 1;
        let backup = format!("{prefix}{n}");
        self.commit_backups.push((backup.clone(), target));
        backup
    }

    /// Applies the steps of `state` from HEAD: concluded (the branch on the last commit, HEAD on
    /// the branch) or stopped on a conflict (in progress).
    fn rebase_run(&mut self, mut state: MemoryRebase) -> Result<MergeOutcome, EngineError> {
        let mut outcome = MergeOutcome {
            after: state.record.before,
            backup: state.record.backup.clone(),
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings: Vec::new(),
        };
        while let Some(op) = state.ops.first().cloned() {
            let tip = self.head_target().unwrap_or(state.record.onto);
            let picked = self
                .commits
                .get(&op.source)
                .cloned()
                .ok_or_else(|| EngineError::UnknownRev(op.source.to_string()))?;
            let base = picked
                .parents
                .first()
                .map(|p| self.tree_at(*p))
                .unwrap_or_default();
            let ours = self.tree_at(tip);
            let (merged, conflicts) = merge_partial(&base, &ours, &self.tree_at(op.source));
            if !conflicts.is_empty() {
                let (differing, _) = self.switch_plan(&merged);
                let index = self.switch_index(&differing, &merged);
                let label = format!("{} ({})", op.source.short(), picked.summary);
                let mut text = Vec::new();
                for c in &conflicts {
                    self.add_conflict(c, &label);
                    text.push(format!("CONFLICT (content): Merge conflict in {}", c.path));
                    outcome.conflicts.push(c.path.clone());
                }
                self.rebase_entries(&index);
                text.push(format!(
                    "error: could not apply {}... {}",
                    op.source.short(),
                    picked.summary
                ));
                self.operation = Some(Operation::Rebase);
                state.stopped = true;
                self.rebase_state = Some(state);
                outcome.in_progress = true;
                outcome.message = text.join("\n");
                return Ok(outcome);
            }
            let commit = self.rebase_commit(&mut state, &op, &merged, &picked)?;
            if let Some(commit) = commit {
                let (differing, _) = self.switch_plan(&merged);
                let index = self.switch_index(&differing, &merged);
                self.head = HeadRef::Detached(commit);
                self.rebase_entries(&index);
            }
            state.ops.remove(0);
        }
        outcome.after = self.rebase_finish(&state);
        Ok(outcome)
    }

    /// The commit of `op` with the tree `tree`, on top of HEAD (first of its group) or rebuilt on
    /// the group's parent; `None` if dropped (empty for the pull). An unchanged leading `pick`
    /// keeps the original commit, as git does.
    fn rebase_commit(
        &mut self,
        state: &mut MemoryRebase,
        op: &RebaseOp,
        tree: &Tree,
        picked: &Commit,
    ) -> Result<Option<Oid>, EngineError> {
        let tip = self.head_target().unwrap_or(state.record.onto);
        if op.first {
            if state.drop_empty && *tree == self.tree_at(tip) {
                return Ok(None);
            }
            state.group_parent = tip;
            if picked.parents.first() == Some(&tip)
                && *tree == self.tree_at(op.source)
                && op.message == pick_message(PickKind::CherryPick, picked, None, false)
            {
                return Ok(Some(op.source));
            }
        }
        let parent = if op.first { tip } else { state.group_parent };
        let (summary, body) = split_message(&cleanup_message(&op.message));
        let id = self.make_commit(vec![parent], tree, Some(op.author.clone()), summary, body)?;
        Ok(Some(id))
    }

    /// The branch on the commit reached, HEAD back on it, nothing in progress.
    fn rebase_finish(&mut self, state: &MemoryRebase) -> Oid {
        let tip = self.head_target().unwrap_or(state.record.onto);
        let index = self.index_tree();
        self.set_ref(format!("{HEADS}{}", state.record.branch), tip, None);
        self.head = HeadRef::Branch(state.record.branch.clone());
        self.rebase_entries(&index);
        self.operation = None;
        self.rebase_state = None;
        tip
    }

    /// [`GitBackend::rebase_continue`].
    pub(super) fn rebase_go_on(&mut self) -> Result<MergeOutcome, EngineError> {
        let status = self.status()?;
        crate::git::merge::check_in_progress("rebase.continue", Operation::Rebase, &status)?;
        let conflicts = crate::git::conflicted_paths(&status);
        if !conflicts.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-rebase-continue-conflicts",
                files = crate::quoted_list(&conflicts)
            )));
        }
        let Some(mut state) = self.rebase_state.clone() else {
            return Err(EngineError::NotAllowed(msg!(
                "engine-rebase-memory-no-state"
            )));
        };
        let mut work = self.clone();
        let mut warnings = Vec::new();
        if state.stopped {
            let op = state.ops.remove(0);
            state.stopped = false;
            let tree = work.index_tree();
            let tip = work.head_target().unwrap_or(state.record.onto);
            let picked = work
                .commits
                .get(&op.source)
                .cloned()
                .ok_or_else(|| EngineError::UnknownRev(op.source.to_string()))?;
            if op.first && tree == work.tree_at(tip) {
                // Resolved to nothing: git drops the commit and goes on.
                state.group_parent = work
                    .commits
                    .get(&tip)
                    .and_then(|c| c.parents.first().copied())
                    .unwrap_or(tip);
                warnings.push(msg!(
                    "engine-rebase-dropped-empty",
                    commit = op.source.short()
                ));
            } else {
                let commit = work.rebase_commit(&mut state, &op, &tree, &picked)?;
                if let Some(commit) = commit {
                    work.head = HeadRef::Detached(commit);
                    work.rebase_entries(&tree);
                }
            }
        }
        work.operation = Some(Operation::Rebase);
        let mut outcome = work.rebase_run(state)?;
        outcome.warnings.extend(warnings);
        *self = work;
        Ok(outcome)
    }

    /// [`GitBackend::rebase_abort`].
    pub(super) fn rebase_stop(&mut self) -> Result<MergeAborted, EngineError> {
        crate::git::merge::check_in_progress("rebase.abort", Operation::Rebase, &self.status()?)?;
        let state = self.rebase_state.clone();
        let mut work = self.clone();
        let aborted = work.abort_operation("rebase", "rebase.abort")?;
        work.rebase_state = None;
        if let Some(state) = state {
            // Like `git rebase --abort`: back to the branch as it was (`reset --hard`).
            let tree = work.tree_at(state.record.before);
            work.entries.retain(|e| !tree.contains_key(&e.path));
            let (differing, _) = work.switch_plan(&tree);
            let index = work.switch_index(&differing, &tree);
            work.head = HeadRef::Branch(state.record.branch);
            work.rebase_entries(&index);
        }
        *self = work;
        Ok(aborted)
    }
}
