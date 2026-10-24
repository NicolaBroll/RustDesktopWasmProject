//! Cherry-pick and revert on the [`MemoryBackend`] (`commit.cherry_pick`, `commit.revert`, S31), with
//! the same checks as the `CliBackend`.
//!
//! Like the in-memory merge, file by file: the commit's patch (from the chosen parent to the commit;
//! for revert, inverted) is applied to HEAD, and a file that HEAD changed differently
//! is a conflict. Without conflicts a new commit on top of HEAD (for cherry-pick with the original
//! author); with conflicts the operation **stays in progress** like the merge (`u` entries, the files
//! without conflicts staged, `CHERRY_PICK_HEAD`/`REVERT_HEAD` and `MERGE_MSG` in
//! [`MemoryMerge`]). A result equal to HEAD (commit already applied) is
//! [`EngineError::NotAllowed`] and changes nothing.

use super::merge::{MemoryMerge, merge_partial};
use super::{HEADS, HeadRef, MemoryBackend};
use crate::EngineError;
use crate::git::GitBackend;
use crate::git::{
    BACKUP_PREFIX, MergeOutcome, PickKind, PickRequest, check_pick_state, pick_empty_error, short,
    split_message,
};

impl MemoryBackend {
    /// [`GitBackend::pick`](crate::GitBackend::pick).
    pub(super) fn pick_commit(
        &mut self,
        request: &PickRequest,
    ) -> Result<MergeOutcome, EngineError> {
        let kind = request.kind;
        check_pick_state(&self.status()?, kind)?;
        if self.head != HeadRef::Branch(request.branch.clone()) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pick-head-moved",
                command = kind.command()
            )));
        }
        let full = format!("{HEADS}{}", request.branch);
        self.check_value(&full, &request.expected)?;
        let Some(picked) = self.commits.get(&request.commit).cloned() else {
            return Err(EngineError::UnknownRev(request.commit.to_string()));
        };
        let commit_tree = self.tree_at(request.commit);
        let parent_tree = request.parent.map(|p| self.tree_at(p)).unwrap_or_default();
        let (base, theirs) = match kind {
            PickKind::CherryPick => (parent_tree, commit_tree),
            PickKind::Revert => (commit_tree, parent_tree),
        };
        let ours = self.tree_at(request.expected);
        let (merged, conflicts) = merge_partial(&base, &ours, &theirs);
        if conflicts.is_empty() && merged == ours {
            return Err(pick_empty_error(kind, &request.commit));
        }
        // The conflicted files present only on their side also go in the worktree.
        let mut written = merged.clone();
        for c in &conflicts {
            if let Some(entry) = c.ours.or(c.theirs) {
                written.insert(c.path.clone(), entry);
            }
        }
        let (_, files) = self.switch_plan(&written);
        if !files.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pick-overwrite",
                command = kind.command(),
                files = files
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let mut work = self.clone();
        let (differing, _) = work.switch_plan(&merged);
        let index = work.switch_index(&differing, &merged);
        let mut outcome = MergeOutcome {
            after: request.expected,
            backup: String::new(),
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings: Vec::new(),
        };
        if conflicts.is_empty() {
            let author = match kind {
                PickKind::CherryPick => Some(picked.author.clone()),
                PickKind::Revert => None,
            };
            let (summary, body) = split_message(&request.message);
            let id = work.make_commit(vec![request.expected], &merged, author, summary, body)?;
            work.set_ref(full, id, None);
            work.rebase_entries(&index);
            outcome.after = id;
        } else {
            // The marker labels like git: `<id> (<subject>)`, `parent of ...` for the
            // revert.
            let label = format!("{} ({})", short(&request.commit), picked.summary);
            let label = match kind {
                PickKind::CherryPick => label,
                PickKind::Revert => format!("parent of {label}"),
            };
            let mut text = Vec::new();
            let mut merge_msg = format!("{}\n\n# Conflicts:\n", request.message);
            for c in &conflicts {
                work.add_conflict(c, &label);
                text.push(format!("CONFLICT (content): Merge conflict in {}", c.path));
                merge_msg.push_str(&format!("#\t{}\n", c.path));
                outcome.conflicts.push(c.path.clone());
            }
            work.rebase_entries(&index);
            work.operation = Some(kind.operation());
            work.merge_state = Some(MemoryMerge {
                heads: vec![request.commit],
                message: merge_msg,
            });
            text.push(format!(
                "error: could not {} {}... {}",
                match kind {
                    PickKind::CherryPick => "apply",
                    PickKind::Revert => "revert",
                },
                short(&request.commit),
                picked.summary
            ));
            outcome.in_progress = true;
            outcome.message = text.join("\n");
        }
        let prefix = format!("{BACKUP_PREFIX}{}-", kind.git());
        let n = work
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix) && !b.starts_with(&format!("{prefix}abort-")))
            .count()
            + 1;
        outcome.backup = format!("{prefix}{n}");
        work.commit_backups
            .push((outcome.backup.clone(), request.expected));
        *self = work;
        Ok(outcome)
    }
}
