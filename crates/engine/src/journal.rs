//! Journal of operations for `edit.undo` and `edit.redo` (`impl-docs/ARCHITECTURE-git.md` §7).
//!
//! Every command that moves refs or HEAD (today `commit.create`, normal and `amend`, and the
//! `branch.*` commands; later merge, rebase, reset) leaves a [`JournalEntry`] with HEAD and the refs involved before and after.
//! `edit.undo` brings the refs back to the "before" values and `edit.redo` to the "after" values, **only** if
//! they still hold what the operation (or the undo) left them with: the backend checks it
//! atomically ([`GitRefs::restore_refs`](crate::GitRefs::restore_refs)) and otherwise gives
//! [`EngineError::UndoConflict`] without touching anything; the entry stays in the journal.
//!
//! Choices:
//!
//! - **In memory, per open repo.** The journal lives in the [`Session`](crate::Session) and is
//!   reset when a repo is opened (even the same one); it is not saved to disk. The CLIs run
//!   one session per invocation: several `--cmd` of the same invocation share the
//!   journal, different invocations do not. Persisting it (e.g. in `.git/project/journal`) would mean
//!   handling several app instances on the same repo and entries written by different versions; it is not
//!   needed to avoid losing data: undone commits stay in backup refs and every
//!   move is in git's reflog (`project: undo ...`).
//! - **`worktree.discard` enters the journal** even though it moves no refs: the entry has the worktree
//!   backup ([`JournalEntry::worktree`], the `git stash create` commit) and `edit.undo` puts
//!   the files back as they were ([`GitIndex::restore_worktree`](crate::GitIndex::restore_worktree)), only if in the meantime they have not
//!   changed; `edit.redo` discards them again, only if they are still as the undo left them.
//! - **`worktree.discard_lines` enters the journal** in the same way, with the file and the blobs of
//!   its bytes before and after ([`JournalEntry::lines`]): undo writes the old bytes back only if
//!   the file has exactly the written ones, redo the other way round
//!   ([`GitIndex::restore_lines`](crate::GitIndex::restore_lines)).
//! - **`worktree.ignore` enters the journal** when it writes the file ([`JournalEntry::ignore`],
//!   with the blobs of the bytes before and after): undo writes the old bytes back (or removes the
//!   file it created) only if the file has exactly the written ones, redo the other way round
//!   ([`GitIndex::restore_ignore`](crate::GitIndex::restore_ignore)).
//! - **Branches.** `branch.create` and `branch.delete` are undone by moving the ref (deleted or
//!   recreated on the previous commit, only if nobody touched it). `branch.checkout` and
//!   `branch.rename`, instead, are undone with another checkout ([`GitRefs::checkout`](crate::GitRefs::checkout): never
//!   forced, refused if it would overwrite changes) and another rename
//!   ([`GitRefs::rename_branch`](crate::GitRefs::rename_branch)), which also update worktree, reflog and configuration.
//! - **`remote.pull` enters the journal** when it moves the branch ([`JournalEntry::pull`]): for the
//!   active branch undo and redo go through [`GitRefs::reset_branch`](crate::GitRefs::reset_branch) (like `git reset --keep`:
//!   they update index and worktree, refuse if a file to restore has local changes), for
//!   another branch they only move the ref. The fetch and the push do not.
//! - **`merge.start` and `merge.continue` enter the journal** when the merge concludes
//!   ([`JournalEntry::merge`]): undo and redo like the pull of the active branch. A merge stopped on a
//!   conflict does not: the branch has not moved, and while the merge is in progress undo and redo are
//!   refused; neither does `merge.abort` (recovery is its index and worktree backup).
//! - **`remote.add` and `remote.remove` enter the journal** ([`JournalEntry::remote_add`],
//!   [`JournalEntry::remote_remove`]): the entry holds the remote ([`RemoteSnapshot`]:
//!   configuration, remote-tracking branches, upstreams that use it); undo and redo remove or
//!   put it back, only if in the meantime it has not changed or been recreated.
//! - **`stash.push` and `stash.drop` enter the journal** ([`JournalEntry::stash_push`],
//!   [`JournalEntry::stash_drop`], with the stash in [`JournalEntry::stash`]): the undo of a push is
//!   the `pop` of that stash (same HEAD, stash still in the list), the redo another push with
//!   the same parameters; the undo of a drop puts the stash back on top of the list
//!   (`git stash store`), the redo drops it again. `stash.apply` and `stash.pop` do not: they change
//!   only index and worktree, and the applied stash stays in the list or in a backup (see
//!   [`StashRecord`]).
//! - **Stage and unstage do not enter the journal**: they move no refs. Their recovery is the index
//!   backup (§7). Nor does such a new entry empty the redo.
//! - **Undo of a commit = `git reset --soft`.** Only the ref moves: index and worktree are not
//!   touched, so the changes of the undone commit reappear staged against the new
//!   HEAD (and `edit.redo` redoes the same commit, which still exists). The first commit of a
//!   branch is undone by deleting the branch: HEAD stays on the branch, which goes back to having no commits.
//! - A commit that after the undo would no longer be reachable from any ref is first saved in
//!   `refs/project/backup/undo-...` (§7); if the backup fails the undo is not done.
//! - The journal keeps at most [`Journal::MAX_ENTRIES`] entries: the oldest drop out.

use crate::{CleanBackup, CleanOutcome, CleanRestore, clean_description};
use crate::{
    CommitOutcome, ConfigEntry, DiscardOutcome, EngineError, GitBackend, HeadState, LogQuery,
    MergeMode, Message, PullMode, RefMove, RefRestore, RemoteSnapshot, ResetMode, ResetRecord,
    ResetRequest, RestoreOutcome, StashDrop, StashEntry, StashPush, StashRecord, UpstreamChange,
    WorktreeBackup, WorktreeRestore, branch_short, check_no_operation, discard_description,
    find_stash, pop_stash, remove_remote, restore_remote, upstream_label,
};
use crate::{DiscardLinesOutcome, LinesBackup, LinesRestore, discard_lines_description};
use crate::{IgnoreOutcome, IgnoreRecord, IgnoreRestore, ignore_description};
use project_domain::Oid;
use serde::{Deserialize, Serialize};

/// How a merge or a pull moved the branch: chooses the description of the journal entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Integration {
    FastForward,
    Merge,
    Rebase,
    /// A squash merge: one ordinary commit with the changes of the merged rev.
    Squash,
}

impl From<&MergeMode> for Integration {
    fn from(mode: &MergeMode) -> Self {
        match mode {
            MergeMode::FastForward => Integration::FastForward,
            MergeMode::Merge { .. } => Integration::Merge,
            MergeMode::Squash => Integration::Squash,
        }
    }
}

impl From<&PullMode> for Integration {
    fn from(mode: &PullMode) -> Self {
        match mode {
            PullMode::FastForward => Integration::FastForward,
            PullMode::Merge { .. } => Integration::Merge,
            PullMode::Rebase { .. } => Integration::Rebase,
        }
    }
}

/// The commands whose entries move the HEAD branch with index and worktree
/// ([`JournalEntry::moves_worktree`]).
const WORKTREE_COMMANDS: &[&str] = &[
    "remote.pull",
    "merge.start",
    "merge.continue",
    "commit.cherry_pick",
    "commit.revert",
    "cherry_pick.continue",
    "revert.continue",
    "rebase.start",
    "rebase.continue",
];

/// A ref moved by an operation: full name, value before and after (`None` = the ref did not
/// exist / was deleted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefChange {
    pub name: String,
    pub before: Option<Oid>,
    pub after: Option<Oid>,
}

/// A branch moved by an integration (pull, merge, cherry-pick, revert): the argument of the
/// [`JournalEntry`] constructors of those commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchMove<'a> {
    /// The branch, full name.
    pub branch: &'a str,
    pub before: Oid,
    pub after: Oid,
    /// HEAD, the same before and after (the branch itself if it is the active branch).
    pub head: HeadState,
}

/// HEAD before and after an operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadChange {
    pub before: HeadState,
    pub after: HeadState,
}

/// A journal entry: a command that moved refs.
///
/// JSON: `{"id": n, "command": "commit.create", "description": "Commit a0744b4 \"...\"",
/// "head": {"before": HeadState, "after": HeadState}, "refs": [{"name", "before", "after"}],
/// "backup": string | null, "undo_backups": [string], "worktree": WorktreeBackup | null}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Sequential in the session (from the first open repo), to recognize the entry.
    pub id: u64,
    /// The id of the command that produced it (e.g. `commit.create`).
    pub command: String,
    /// Description for people, e.g. `Commit a0744b4 “Update README”`: in JSON a
    /// [`Message`] (`{"text", "key", "args"}`), which the UI shows in its language.
    pub description: Message,
    pub head: HeadChange,
    /// The refs moved (for a commit on a branch, the branch; with detached HEAD none).
    pub refs: Vec<RefChange>,
    /// The backup saved by the operation itself (e.g. the commit rewritten by `amend`).
    pub backup: Option<String>,
    /// The backup refs created by undo and redo of this entry for commits that would otherwise no
    /// longer be reachable.
    #[serde(default)]
    pub undo_backups: Vec<String>,
    /// Only for `worktree.discard`: the worktree backup (`git stash create` commit) and the
    /// discarded files. With this field undo and redo put the files back, they do not move refs.
    #[serde(default)]
    pub worktree: Option<WorktreeBackup>,
    /// Only for `branch.delete`: the `branch.<name>.*` configuration removed with the branch, which
    /// undo rewrites (if in the meantime the repo has no new one) and redo removes again.
    #[serde(default)]
    pub branch_config: Vec<ConfigEntry>,
    /// Only for `remote.add` and `remote.remove`: the remote as it was after the addition / before
    /// the removal (configuration, remote-tracking branches, upstreams that use it). Undo and redo remove
    /// or put it back ([`remove_remote`], [`restore_remote`]), only if in the meantime it has not
    /// changed / been recreated. In the JSON the URLs are masked, so it is not
    /// read back from the JSON (`skip_deserializing`). After an undo that skipped some upstreams
    /// (set elsewhere) the entry keeps only what was put back.
    #[serde(default, skip_deserializing)]
    pub remote: Option<RemoteSnapshot>,
    /// Only for `stash.push` and `stash.drop`: the stash (id, message, base and, for the push, the
    /// parameters). Undo and redo put it back or remove it from the list ([`StashRecord`]); after the
    /// redo of a push the entry has the id of the new stash.
    #[serde(default)]
    pub stash: Option<StashRecord>,
    /// Only for `branch.reset`: the mode and the stash of a `hard` ([`ResetRecord`]). Undo and redo
    /// go through [`GitRefs::reset`](crate::GitRefs::reset) with the same mode, strict (nothing may need a backup).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset: Option<ResetRecord>,
    /// Only for `branch.create` with `track` and `branch.set_upstream`: the upstream of the
    /// branch before and after ([`UpstreamChange`]). Undo writes `before` back, redo `after`,
    /// only if the repo still has the other value (for `branch.create` after the ref was
    /// deleted / recreated: then a mismatch is a warning; for `branch.set_upstream`
    /// [`EngineError::UndoConflict`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamChange>,
    /// Only for `worktree.clean`: the backup commit and the deleted files ([`CleanBackup`]). Undo
    /// recreates them where the path is absent again, redo deletes them again if they still match
    /// the backup; neither moves refs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean: Option<CleanBackup>,
    /// Only for `worktree.discard_lines`: the file with the blobs of its bytes before and after
    /// ([`LinesBackup`]). Undo writes the old bytes back only if the file has exactly the written
    /// ones, redo the other way round; neither moves refs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<LinesBackup>,
    /// Only for `worktree.ignore`: the file with the blobs of its bytes before and after
    /// ([`IgnoreRecord`]). Undo writes the old bytes back (or removes the file it created) only if
    /// the file has exactly the written ones, redo the other way round; neither moves refs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ignore: Option<IgnoreRecord>,
}

impl JournalEntry {
    /// The entry of a successful `commit.create`; `None` if it moved nothing (an `amend` that
    /// produced the same commit) or if the result is not coherent (detached HEAD without
    /// a previous commit).
    pub fn commit(outcome: &CommitOutcome, amend: bool) -> Option<Self> {
        let new = outcome.commit.id;
        if outcome.previous == Some(new) {
            return None;
        }
        let (head, refs) = match &outcome.branch {
            Some(branch) => (
                HeadChange {
                    before: HeadState::Branch(branch.clone()),
                    after: HeadState::Branch(branch.clone()),
                },
                vec![RefChange {
                    name: branch.clone(),
                    before: outcome.previous,
                    after: Some(new),
                }],
            ),
            None => (
                HeadChange {
                    before: HeadState::Detached(outcome.previous?),
                    after: HeadState::Detached(new),
                },
                Vec::new(),
            ),
        };
        let summary = &outcome.commit.summary;
        let description = match (amend, outcome.previous) {
            (true, Some(old)) => msg!(
                "engine-journal-amend",
                id = new.short(),
                summary = summary,
                old = old.short()
            ),
            _ => msg!("engine-journal-commit", id = new.short(), summary = summary),
        };
        Some(Self {
            id: 0,
            command: "commit.create".to_owned(),
            description,
            head,
            refs,
            backup: outcome.backup.clone(),
            undo_backups: Vec::new(),
            worktree: None,
            branch_config: Vec::new(),
            remote: None,
            stash: None,
            reset: None,
            upstream: None,
            clean: None,
            lines: None,
            ignore: None,
        })
    }

    /// The entry of a successful `worktree.discard`, with HEAD as it was (it does not move) and the backup.
    pub fn discard(outcome: &DiscardOutcome, head: HeadState) -> Self {
        Self {
            id: 0,
            command: "worktree.discard".to_owned(),
            description: discard_description(&outcome.files),
            head: HeadChange {
                before: head.clone(),
                after: head,
            },
            refs: Vec::new(),
            backup: Some(outcome.backup.clone()),
            undo_backups: Vec::new(),
            worktree: Some(WorktreeBackup {
                stash: outcome.stash,
                files: outcome.files.clone(),
                raw: outcome.raw,
            }),
            branch_config: Vec::new(),
            remote: None,
            stash: None,
            reset: None,
            upstream: None,
            clean: None,
            lines: None,
            ignore: None,
        }
    }

    /// The entry of a successful `worktree.discard_lines`, with HEAD as it was (it does not move)
    /// and the backup.
    pub fn discard_lines(outcome: &DiscardLinesOutcome, head: HeadState) -> Self {
        Self {
            id: 0,
            command: "worktree.discard_lines".to_owned(),
            description: discard_lines_description(&outcome.file.path),
            head: HeadChange {
                before: head.clone(),
                after: head,
            },
            refs: Vec::new(),
            backup: Some(outcome.backup.clone()),
            undo_backups: Vec::new(),
            worktree: None,
            branch_config: Vec::new(),
            remote: None,
            stash: None,
            reset: None,
            upstream: None,
            clean: None,
            lines: Some(outcome.file.clone()),
            ignore: None,
        }
    }

    /// The entry of a successful `worktree.clean`, with HEAD as it was (it does not move) and the
    /// backup.
    pub fn clean(outcome: &CleanOutcome, head: HeadState) -> Self {
        Self {
            id: 0,
            command: "worktree.clean".to_owned(),
            description: clean_description(&outcome.files),
            head: HeadChange {
                before: head.clone(),
                after: head,
            },
            refs: Vec::new(),
            backup: Some(outcome.backup.clone()),
            undo_backups: Vec::new(),
            worktree: None,
            branch_config: Vec::new(),
            remote: None,
            stash: None,
            reset: None,
            upstream: None,
            clean: Some(CleanBackup {
                commit: outcome.commit,
                files: outcome.files.clone(),
            }),
            lines: None,
            ignore: None,
        }
    }

    /// The entry of a `worktree.ignore` that wrote the file, with HEAD as it was (it does not
    /// move) and the backup. Only called when [`IgnoreOutcome::record`] is set.
    pub fn ignore(outcome: &IgnoreOutcome, head: HeadState) -> Self {
        Self {
            id: 0,
            command: "worktree.ignore".to_owned(),
            description: ignore_description(outcome.file, &outcome.added),
            head: HeadChange {
                before: head.clone(),
                after: head,
            },
            refs: Vec::new(),
            backup: outcome.backup.clone(),
            undo_backups: Vec::new(),
            worktree: None,
            branch_config: Vec::new(),
            remote: None,
            stash: None,
            reset: None,
            upstream: None,
            clean: None,
            lines: None,
            ignore: outcome.record.clone(),
        }
    }

    /// The entry of a successful `branch.create`: the branch `branch` (full name) created on
    /// `target`, HEAD as it was. Undo deletes it (only if `target` still holds), redo
    /// recreates it (only if it does not exist).
    pub fn branch_create(branch: &str, target: Oid, head: HeadState) -> Self {
        Self::branch_entry(
            "branch.create",
            msg!(
                "engine-journal-branch-create",
                name = branch_short(branch),
                id = target.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: None,
                after: Some(target),
            }],
            None,
        )
    }

    /// The entry of a successful `branch.create` with `track`: as [`JournalEntry::branch_create`]
    /// plus the upstream written (`change`). Undo deletes the branch and then puts `before` back
    /// (only if the upstream is still `after`, otherwise a warning); redo recreates both.
    pub fn branch_create_tracking(
        branch: &str,
        target: Oid,
        head: HeadState,
        change: UpstreamChange,
    ) -> Self {
        let mut entry = Self::branch_create(branch, target, head);
        entry.description = msg!(
            "engine-journal-branch-create-track",
            name = branch_short(branch),
            id = target.short(),
            upstream = upstream_label(&change.after).unwrap_or_default()
        );
        entry.upstream = Some(change);
        entry
    }

    /// The entry of a successful `branch.set_upstream`: no ref moves, HEAD as it was; the
    /// upstream of the branch (full name `branch`) from `change.before` to `change.after`.
    /// Undo and redo write the other value only if the repo still has the one the operation
    /// (or the undo) left ([`EngineError::UndoConflict`] otherwise).
    pub fn set_upstream(branch: &str, head: HeadState, change: UpstreamChange) -> Self {
        let name = branch_short(branch);
        let description = match upstream_label(&change.after) {
            Some(upstream) => msg!(
                "engine-journal-upstream-set",
                name = name,
                upstream = upstream
            ),
            None => msg!(
                "engine-journal-upstream-unset",
                name = name,
                previous = upstream_label(&change.before).unwrap_or_default()
            ),
        };
        let mut entry = Self::branch_entry(
            "branch.set_upstream",
            description,
            HeadChange {
                before: head.clone(),
                after: head,
            },
            Vec::new(),
            None,
        );
        entry.upstream = Some(change);
        entry
    }

    /// Undo and redo of the upstream of a [`JournalEntry::set_upstream`]: the branch must still
    /// exist and have the upstream the operation (or the undo) left, otherwise
    /// [`EngineError::UndoConflict`] and nothing changes.
    fn apply_upstream(
        &self,
        repo: &mut dyn GitBackend,
        redo: bool,
    ) -> Result<RestoreOutcome, EngineError> {
        let command = if redo { "edit.redo" } else { "edit.undo" };
        let Some(change) = &self.upstream else {
            return Err(EngineError::NotAllowed(msg!(
                "engine-undo-bad-entry",
                description = self.description
            )));
        };
        check_no_operation(
            command,
            msg!("engine-busy-upstream"),
            repo.status()?.operation,
        )?;
        let (expected, target) = if redo {
            (&change.before, &change.after)
        } else {
            (&change.after, &change.before)
        };
        let conflict = |what: Message| EngineError::UndoConflict {
            command,
            refs: vec![what],
        };
        let full = crate::branch_ref(&change.branch);
        if !repo.refs()?.iter().any(|r| r.name == full) {
            return Err(conflict(msg!(
                "engine-undo-upstream-branch-gone",
                name = change.branch.escape_debug()
            )));
        }
        let now = repo.local_upstream(&change.branch)?;
        if now != *expected {
            return Err(conflict(msg!(
                "engine-undo-upstream-changed",
                name = change.branch.escape_debug(),
                expected = upstream_label(expected).unwrap_or_else(|| "-".to_owned()),
                now = upstream_label(&now).unwrap_or_else(|| "-".to_owned())
            )));
        }
        let warnings = repo.write_upstream(&change.branch, expected, target)?;
        Ok(RestoreOutcome {
            backups: Vec::new(),
            warnings,
        })
    }

    /// After undo/redo of a `branch.create` with `track` moved the ref: the upstream back to
    /// `before` (undo) or to `after` (redo), only if the repo has the other one; otherwise (or
    /// if the write fails) a warning, since the ref has already moved.
    fn restore_create_upstream(&self, repo: &mut dyn GitBackend, redo: bool) -> Vec<Message> {
        let Some(change) = &self.upstream else {
            return Vec::new();
        };
        let (expected, target) = if redo {
            (&change.before, &change.after)
        } else {
            (&change.after, &change.before)
        };
        let label = |e: &[ConfigEntry]| upstream_label(e).unwrap_or_else(|| "-".to_owned());
        match repo.local_upstream(&change.branch) {
            Ok(now) if now == *expected => repo
                .write_upstream(&change.branch, expected, target)
                .unwrap_or_else(|e| {
                    vec![msg!(
                        "engine-undo-upstream-not-written",
                        name = change.branch.escape_debug(),
                        error = e
                    )]
                }),
            Ok(now) => vec![msg!(
                "engine-undo-upstream-left",
                name = change.branch.escape_debug(),
                now = label(&now),
                expected = label(expected)
            )],
            Err(e) => vec![msg!(
                "engine-undo-upstream-not-written",
                name = change.branch.escape_debug(),
                error = e
            )],
        }
    }

    /// The entry of a successful `tag.create`: the tag `tag` (full name) created with the value
    /// `value` (the commit, or the tag object of an annotated tag) on commit `target`, HEAD as it
    /// was. Undo deletes it (only if it still has `value`), redo recreates it (only if it does not
    /// exist).
    pub fn tag_create(tag: &str, value: Oid, target: Oid, head: HeadState) -> Self {
        Self::branch_entry(
            "tag.create",
            msg!(
                "engine-journal-tag-create",
                name = tag.strip_prefix(crate::TAGS_PREFIX).unwrap_or(tag),
                id = target.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: tag.to_owned(),
                before: None,
                after: Some(value),
            }],
            None,
        )
    }

    /// The entry of a successful `tag.delete`: the tag `tag` (full name) that had the value
    /// `value` (the commit, or the tag object of an annotated tag) on commit `target`, with the backup
    /// ref. Undo recreates it on `value` (the same tag object) only if it is still absent, redo
    /// deletes it again only if it still has `value`.
    pub fn tag_delete(tag: &str, value: Oid, target: Oid, head: HeadState, backup: String) -> Self {
        Self::branch_entry(
            "tag.delete",
            msg!(
                "engine-journal-tag-delete",
                name = tag.strip_prefix(crate::TAGS_PREFIX).unwrap_or(tag),
                id = target.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: tag.to_owned(),
                before: Some(value),
                after: None,
            }],
            Some(backup),
        )
    }

    /// The entry of a successful `branch.delete`: the branch `branch` (full name) that held
    /// `target`, with the backup ref. Undo recreates it on `target` (only if in the meantime
    /// nobody created a branch with the same name), redo deletes it again (only if it
    /// still holds `target`).
    pub fn branch_delete(
        branch: &str,
        target: Oid,
        head: HeadState,
        backup: String,
        config: Vec<ConfigEntry>,
    ) -> Self {
        let mut entry = Self::branch_entry(
            "branch.delete",
            msg!(
                "engine-journal-branch-delete",
                name = branch_short(branch),
                id = target.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(target),
                after: None,
            }],
            Some(backup),
        );
        entry.branch_config = config;
        entry
    }

    /// The entry of a successful `branch.rename`: `from` became `to` (full names) on
    /// `target`; HEAD before and after (changes if it was the HEAD branch). Undo and redo rename
    /// again ([`GitRefs::rename_branch`](crate::GitRefs::rename_branch)), so reflog and configuration follow the branch.
    pub fn branch_rename(from: &str, to: &str, target: Oid, head: HeadChange) -> Self {
        Self::branch_entry(
            "branch.rename",
            msg!(
                "engine-journal-branch-rename",
                from = branch_short(from),
                to = branch_short(to)
            ),
            head,
            vec![
                RefChange {
                    name: from.to_owned(),
                    before: Some(target),
                    after: None,
                },
                RefChange {
                    name: to.to_owned(),
                    before: None,
                    after: Some(target),
                },
            ],
            None,
        )
    }

    /// The entry of a successful `branch.checkout`: HEAD from `before` to `after`, no ref
    /// moved. Undo and redo redo a safe checkout ([`GitRefs::checkout`](crate::GitRefs::checkout)), which never
    /// overwrites local changes.
    pub fn checkout(before: HeadState, after: HeadState) -> Self {
        Self::head_switch("branch.checkout", before, after, None)
    }

    /// The entry of a successful `commit.checkout` (HEAD detached at `after`): same undo and
    /// redo as [`JournalEntry::checkout`]; `backup` is the `refs/project/backup/detached-...`
    /// saved when leaving a detached HEAD that no ref reached.
    pub fn commit_checkout(before: HeadState, after: HeadState, backup: Option<String>) -> Self {
        Self::head_switch("commit.checkout", before, after, backup)
    }

    fn head_switch(
        command: &str,
        before: HeadState,
        after: HeadState,
        backup: Option<String>,
    ) -> Self {
        let name = |h: &HeadState| match h {
            HeadState::Branch(b) => msg!("engine-journal-head-branch", name = branch_short(b)),
            HeadState::Detached(oid) => msg!("engine-journal-head-detached", id = oid.short()),
        };
        Self::branch_entry(
            command,
            msg!(
                "engine-journal-checkout",
                after = name(&after),
                before = name(&before)
            ),
            HeadChange { before, after },
            Vec::new(),
            backup,
        )
    }

    /// The entry of a successful `remote.pull` that moved the branch `branch` (full name)
    /// from `before` to `after`; `head` is HEAD (same before and after: the branch itself if it was the
    /// active branch), `strategy` `fast_forward`/`merge`/`rebase`, `upstream` the remote-tracking branch
    /// (short name), `backup` the ref of the branch before the pull. Undo and redo of the active branch
    /// go through [`GitRefs::reset_branch`](crate::GitRefs::reset_branch) (they also update index and worktree, like
    /// `git reset --keep`); of another branch through [`GitRefs::restore_refs`](crate::GitRefs::restore_refs).
    pub fn pull(
        moved: BranchMove<'_>,
        strategy: Integration,
        upstream: &str,
        backup: String,
    ) -> Self {
        let BranchMove {
            branch,
            before,
            after,
            head,
        } = moved;
        let how = match strategy {
            Integration::FastForward => msg!("engine-journal-pull-ff"),
            Integration::Rebase => msg!("engine-journal-pull-rebase", upstream = upstream),
            Integration::Merge | Integration::Squash => {
                msg!("engine-journal-pull-merge", upstream = upstream)
            }
        };
        Self::branch_entry(
            "remote.pull",
            msg!(
                "engine-journal-pull",
                branch = branch_short(branch),
                how = how,
                before = before.short(),
                after = after.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(before),
                after: Some(after),
            }],
            Some(backup),
        )
    }

    /// The entry of a concluded merge (successful `merge.start`, or `merge.continue`, `command`):
    /// the HEAD branch `branch` (full name) from `before` to `after` (the merge commit or
    /// the fast-forward one), HEAD same before and after; `strategy` `fast_forward`/`merge`,
    /// `source` the name of what was merged (`feature`, `origin/x`, an id), `backup` the ref
    /// of the branch before the merge (if saved). Undo and redo as for a pull of the active branch:
    /// [`GitRefs::reset_branch`](crate::GitRefs::reset_branch) (like `git reset --keep`).
    pub fn merge(
        command: &str,
        moved: BranchMove<'_>,
        strategy: Integration,
        source: &str,
        backup: Option<String>,
    ) -> Self {
        let BranchMove {
            branch,
            before,
            after,
            head,
        } = moved;
        let strategy = match strategy {
            Integration::FastForward => "fast_forward",
            Integration::Squash => "squash",
            Integration::Merge | Integration::Rebase => "merge",
        };
        Self::branch_entry(
            command,
            msg!(
                "engine-journal-merge",
                source = source,
                branch = branch_short(branch),
                strategy = strategy,
                before = before.short(),
                after = after.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(before),
                after: Some(after),
            }],
            backup,
        )
    }

    /// The entry of a concluded cherry-pick or revert (`commit.cherry_pick`, `commit.revert`,
    /// `cherry_pick.continue`, `revert.continue`): the branch `branch` (full name) from
    /// `before` to `after` (a new commit on top of `before`), HEAD same before and after; `source`
    /// the commit applied or undone, `backup` the ref of the branch before (if saved). Undo and
    /// redo like the merge: [`GitRefs::reset_branch`](crate::GitRefs::reset_branch) (like `git reset --keep`).
    pub fn pick(
        command: &str,
        kind: crate::PickKind,
        moved: BranchMove<'_>,
        source: &str,
        backup: Option<String>,
    ) -> Self {
        let BranchMove {
            branch,
            before,
            after,
            head,
        } = moved;
        Self::branch_entry(
            command,
            msg!(
                "engine-journal-pick",
                kind = match kind {
                    crate::PickKind::CherryPick => "cherry_pick",
                    crate::PickKind::Revert => "revert",
                },
                source = source,
                branch = branch_short(branch),
                before = before.short(),
                after = after.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(before),
                after: Some(after),
            }],
            backup,
        )
    }

    /// The entry of a concluded interactive rebase (`rebase.start`, or `rebase.continue`,
    /// `command`): the branch `branch` (full name) from `before` to `after`, HEAD same before and
    /// after; `commits` the commits of the range rewritten, `backup` the ref of the branch before.
    /// Undo and redo like the merge: [`GitRefs::reset_branch`](crate::GitRefs::reset_branch) (like `git reset --keep`).
    pub fn rebase(
        command: &str,
        branch: &str,
        before: Oid,
        after: Oid,
        commits: usize,
        backup: Option<String>,
    ) -> Self {
        let head = HeadState::Branch(branch.to_owned());
        Self::branch_entry(
            command,
            msg!(
                "engine-journal-rebase",
                branch = branch_short(branch),
                n = commits,
                before = before.short(),
                after = after.short()
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(before),
                after: Some(after),
            }],
            backup,
        )
    }

    /// The entry of a successful `branch.reset`: HEAD (`head`, the same branch before and after,
    /// or detached from `before` to `after`) and, on a branch, the branch moved; `backup` the ref of
    /// the local changes saved by a `hard` (with `stash`, the `git stash create` commit). Undo and
    /// redo go through [`GitRefs::reset`](crate::GitRefs::reset) with the same `mode`: index and worktree (for `mixed`
    /// and `hard`) follow the ref, but only if nothing would need a new backup; the local
    /// changes overwritten by a `hard` stay in the backup (the undo says so).
    pub fn reset(
        branch: Option<&str>,
        before: Oid,
        after: Oid,
        mode: ResetMode,
        backup: Option<String>,
        stash: Option<Oid>,
    ) -> Self {
        let (head, refs, name) = match branch {
            Some(b) => (
                HeadChange {
                    before: HeadState::Branch(b.to_owned()),
                    after: HeadState::Branch(b.to_owned()),
                },
                vec![RefChange {
                    name: b.to_owned(),
                    before: Some(before),
                    after: Some(after),
                }],
                branch_short(b).to_owned(),
            ),
            None => (
                HeadChange {
                    before: HeadState::Detached(before),
                    after: HeadState::Detached(after),
                },
                Vec::new(),
                "HEAD".to_owned(),
            ),
        };
        let mut entry = Self::branch_entry(
            "branch.reset",
            msg!(
                "engine-journal-reset",
                branch = name,
                mode = mode.as_str(),
                before = before.short(),
                after = after.short()
            ),
            head,
            refs,
            backup,
        );
        entry.reset = Some(ResetRecord { mode, stash });
        entry
    }

    /// True if the entry moves the HEAD branch with index and worktree (a pull of the active
    /// branch, a merge, a cherry-pick or revert): undo and redo use
    /// [`GitRefs::reset_branch`](crate::GitRefs::reset_branch).
    fn moves_worktree(&self) -> bool {
        WORKTREE_COMMANDS.contains(&self.command.as_str())
            && matches!(&self.head.after, HeadState::Branch(b) if self.refs.len() == 1 && self.refs[0].name == *b)
    }

    fn branch_entry(
        command: &str,
        description: Message,
        head: HeadChange,
        refs: Vec<RefChange>,
        backup: Option<String>,
    ) -> Self {
        Self {
            id: 0,
            command: command.to_owned(),
            description,
            head,
            refs,
            backup,
            undo_backups: Vec::new(),
            worktree: None,
            branch_config: Vec::new(),
            remote: None,
            stash: None,
            reset: None,
            upstream: None,
            clean: None,
            lines: None,
            ignore: None,
        }
    }

    /// The entry of a successful `remote.add`: `snapshot` is the remote right after (with the
    /// remote-tracking branches of the fetch, if done), `head` HEAD (does not change). Undo removes configuration and
    /// remote-tracking branches only if they are still exactly like this (and no branch took the remote
    /// as upstream); redo puts them back only if the remote does not exist.
    pub fn remote_add(snapshot: RemoteSnapshot, head: HeadState, fetched: bool) -> Self {
        let description = if fetched {
            msg!(
                "engine-journal-remote-add-fetched",
                name = snapshot.name,
                n = snapshot.refs.len()
            )
        } else {
            msg!("engine-journal-remote-add", name = snapshot.name)
        };
        Self::remote_entry("remote.add", description, head, snapshot, false)
    }

    /// The entry of a successful `remote.remove`: `snapshot` is the remote as it was before, `backup`
    /// the first backup ref of the commits that remained only in remote-tracking branches (the others in
    /// `undo_backups`). Undo puts it back (remote-tracking branches, configuration, upstream) only if
    /// nobody recreated a remote with that name; redo removes it again only if it is still
    /// as it was.
    pub fn remote_remove(snapshot: RemoteSnapshot, head: HeadState, backups: &[String]) -> Self {
        let upstreams = snapshot.upstream_branches().len();
        let description = msg!(
            "engine-journal-remote-remove",
            name = snapshot.name,
            n = snapshot.refs.len(),
            upstreams = upstreams
        );
        let mut entry = Self::remote_entry("remote.remove", description, head, snapshot, true);
        entry.backup = backups.first().cloned();
        entry.undo_backups = backups.iter().skip(1).cloned().collect();
        entry
    }

    fn remote_entry(
        command: &str,
        description: Message,
        head: HeadState,
        snapshot: RemoteSnapshot,
        removed: bool,
    ) -> Self {
        let refs = snapshot
            .refs
            .iter()
            .map(|r| RefChange {
                name: r.name.clone(),
                before: removed.then_some(r.target),
                after: (!removed).then_some(r.target),
            })
            .collect();
        let mut entry = Self::branch_entry(
            command,
            description,
            HeadChange {
                before: head.clone(),
                after: head,
            },
            refs,
            None,
        );
        entry.remote = Some(snapshot);
        entry
    }

    /// The entry of a successful `stash.push`: the stash created (`stash`), HEAD (does not change) and the
    /// parameters, for the redo. Undo = `pop` of the stash with the index, redo = another push.
    pub fn stash_push(stash: &StashEntry, head: HeadState, request: StashPush) -> Self {
        let mut entry = Self::branch_entry(
            "stash.push",
            msg!(
                "engine-journal-stash-push",
                id = stash.id.short(),
                message = stash.message
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            Vec::new(),
            None,
        );
        entry.stash = Some(StashRecord {
            id: stash.id,
            message: stash.message.clone(),
            base: stash.base,
            push: Some(request),
        });
        entry
    }

    /// The entry of a successful `stash.drop`: the stash dropped, with the backup ref that
    /// keeps its commit. Undo = back in the list, on top (`git stash store`); redo = drop.
    pub fn stash_drop(stash: &StashEntry, head: HeadState, backup: Option<String>) -> Self {
        let mut entry = Self::branch_entry(
            "stash.drop",
            msg!(
                "engine-journal-stash-drop",
                name = stash.name,
                id = stash.id.short(),
                message = stash.message
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            Vec::new(),
            backup,
        );
        entry.stash = Some(StashRecord {
            id: stash.id,
            message: stash.message.clone(),
            base: stash.base,
            push: None,
        });
        entry
    }

    /// Undo and redo of `stash.push` and `stash.drop` (see [`StashRecord`]). Checks first,
    /// which without writing anything give [`EngineError::UndoConflict`] if the stash or HEAD are
    /// no longer as the operation (or the undo) left them.
    fn apply_stash(
        &mut self,
        repo: &mut dyn GitBackend,
        redo: bool,
    ) -> Result<RestoreOutcome, EngineError> {
        let command = if redo { "edit.redo" } else { "edit.undo" };
        let Some(record) = self.stash.clone() else {
            return Err(EngineError::NotAllowed(msg!(
                "engine-undo-bad-entry",
                description = self.description
            )));
        };
        let conflict = |what: Message| EngineError::UndoConflict {
            command,
            refs: vec![what],
        };
        let present = find_stash(&repo.stashes()?, &record.id).is_some();
        let short_id = record.id.short();
        if self.command == "stash.drop" {
            if redo {
                if !present {
                    return Err(conflict(msg!(
                        "engine-undo-stash-gone",
                        short_id = short_id
                    )));
                }
                // The drop's backup is usually already there; if it no longer holds the stash (removed or
                // moved by hand), a new one before dropping it.
                let kept = self.backup.as_deref().is_some_and(|b| {
                    repo.log(&LogQuery::page(0, 1).from_rev(b))
                        .ok()
                        .and_then(|c| c.first().map(|c| c.id))
                        == Some(record.id)
                });
                let dropped = repo.stash_drop(&StashDrop {
                    command,
                    id: record.id,
                    backup: (!kept).then_some("redo"),
                })?;
                return Ok(RestoreOutcome {
                    backups: dropped.backup.into_iter().collect(),
                    warnings: dropped.warnings,
                });
            }
            if present {
                return Err(conflict(msg!(
                    "engine-undo-stash-present",
                    short_id = short_id
                )));
            }
            repo.stash_store(&record.id, &record.message)?;
            return Ok(RestoreOutcome::default());
        }

        // `stash.push`: the entry's HEAD, on the commit the stash was created on (so
        // the undo's `pop` cannot conflict and the redo sets aside against
        // the same commit).
        check_no_operation(
            command,
            msg!("engine-busy-stash-undo"),
            repo.status()?.operation,
        )?;
        let head = repo.head()?;
        let state = HeadState::from_head(&head);
        let expected = if redo {
            &self.head.before
        } else {
            &self.head.after
        };
        if state != *expected || head.target() != Some(record.base) {
            let base = record.base.short();
            return Err(conflict(match head.target() {
                Some(t) => msg!(
                    "engine-undo-stash-head",
                    expected = expected,
                    base = base,
                    now = state,
                    commit = t.short()
                ),
                None => msg!(
                    "engine-undo-stash-head-unborn",
                    expected = expected,
                    base = base,
                    now = state
                ),
            }));
        }
        if redo {
            let push = record.push.clone().unwrap_or_default();
            let stash = repo.stash_push(&push)?;
            self.stash = Some(StashRecord {
                id: stash.id,
                message: stash.message,
                ..record
            });
            return Ok(RestoreOutcome::default());
        }
        if !present {
            return Err(conflict(msg!(
                "engine-undo-stash-gone",
                short_id = short_id
            )));
        }
        let popped =
            pop_stash(repo, command, record.id, true, Some("undo")).map_err(|e| match e {
                // Undo always uses `restore_index`: it cannot "retry without".
                EngineError::NotAllowed(m) if m.key() == crate::INDEX_NOT_RESTORED => {
                    EngineError::NotAllowed(msg!("engine-undo-stash-index", short_id = short_id))
                }
                other => other,
            })?;
        let applied = popped.applied;
        if let Some(e) = popped.drop_error {
            return Err(EngineError::GitWithReason {
                command: "stash".to_owned(),
                reason: msg!(
                    "engine-undo-stash-not-dropped",
                    short_id = short_id,
                    error = e
                ),
            });
        }
        let Some(dropped) = popped.dropped else {
            return Err(EngineError::GitWithReason {
                command: "stash".to_owned(),
                reason: msg!(
                    "engine-undo-stash-conflicts",
                    short_id = short_id,
                    files = applied.conflicts.join(", ")
                ),
            });
        };
        let mut warnings = applied.warnings;
        warnings.extend(dropped.warnings);
        Ok(RestoreOutcome {
            backups: dropped.backup.into_iter().collect(),
            warnings,
        })
    }

    /// The request to the backend to undo (`redo == false`) or redo a discard; `None` if
    /// the entry is not a `worktree.discard`.
    pub fn worktree_request(&self, redo: bool) -> Option<WorktreeRestore> {
        let backup = self.worktree.as_ref()?;
        Some(WorktreeRestore {
            command: if redo { "edit.redo" } else { "edit.undo" },
            redo,
            stash: backup.stash,
            files: backup.files.clone(),
            raw: backup.raw,
        })
    }

    /// Before the undo or redo of a discard (whole files or lines): no operation in progress (it
    /// would rewrite merge files) and HEAD the recorded one (`head.after` for the undo,
    /// `head.before` for the redo); otherwise an error and nothing changes.
    fn check_worktree_state(
        &self,
        repo: &dyn GitBackend,
        command: &'static str,
        redo: bool,
    ) -> Result<(), EngineError> {
        check_no_operation(
            command,
            msg!("engine-busy-worktree"),
            repo.status()?.operation,
        )?;
        let expected = if redo {
            &self.head.before
        } else {
            &self.head.after
        };
        let now = HeadState::from_head(&repo.head()?);
        if now != *expected {
            return Err(EngineError::UndoConflict {
                command,
                refs: vec![msg!("engine-head-changed", expected = expected, now = now)],
            });
        }
        Ok(())
    }

    /// Runs undo or redo of the entry on the backend: the files for a discard, the refs otherwise.
    ///
    /// For a discard HEAD must be the recorded one (`head.after` for the undo,
    /// `head.before` for the redo): the files are compared with the index, but the operation
    /// belongs to that state of the repo; with HEAD moved (a checkout, a commit from a
    /// terminal) [`EngineError::UndoConflict`] and nothing changes.
    fn apply(
        &mut self,
        repo: &mut dyn GitBackend,
        redo: bool,
    ) -> Result<RestoreOutcome, EngineError> {
        if let Some(request) = self.worktree_request(redo) {
            self.check_worktree_state(repo, request.command, redo)?;
            return repo.restore_worktree(&request);
        }
        if let Some(file) = &self.lines {
            let request = LinesRestore {
                command: if redo { "edit.redo" } else { "edit.undo" },
                redo,
                file: file.clone(),
            };
            self.check_worktree_state(repo, request.command, redo)?;
            return repo.restore_lines(&request);
        }
        if let Some(backup) = &self.clean {
            // Untracked files do not depend on HEAD: only the paths are checked (by the backend),
            // and never in the middle of a merge, rebase...
            let request = CleanRestore {
                command: if redo { "edit.redo" } else { "edit.undo" },
                redo,
                commit: backup.commit,
                files: backup.files.clone(),
            };
            check_no_operation(
                request.command,
                msg!("engine-busy-worktree"),
                repo.status()?.operation,
            )?;
            return repo.restore_cleaned(&request);
        }
        if let Some(record) = &self.ignore {
            // Only the bytes of the file are checked (by the backend): they do not depend on
            // HEAD; never in the middle of a merge, rebase...
            let request = IgnoreRestore {
                command: if redo { "edit.redo" } else { "edit.undo" },
                redo,
                record: record.clone(),
            };
            check_no_operation(
                request.command,
                msg!("engine-busy-worktree"),
                repo.status()?.operation,
            )?;
            return repo.restore_ignore(&request);
        }
        if self.stash.is_some() {
            return self.apply_stash(repo, redo);
        }
        if self.command == "branch.set_upstream" {
            return self.apply_upstream(repo, redo);
        }
        let request = if redo {
            self.redo_request()
        } else {
            self.undo_request()
        };
        match self.command.as_str() {
            "branch.checkout" | "commit.checkout" => {
                Self::check_unchanged(repo, &request, msg!("engine-busy-checkout"))?;
                repo.checkout(&request.head_target)
            }
            "branch.rename" => {
                Self::check_unchanged(repo, &request, msg!("engine-busy-branch-rename"))?;
                // From what exists now (expected) to what must exist afterwards (target).
                let from = request.refs.iter().find(|r| r.target.is_none());
                let to = request.refs.iter().find(|r| r.expected.is_none());
                match (from, to) {
                    (
                        Some(RefMove {
                            name: from,
                            expected: Some(oid),
                            ..
                        }),
                        Some(RefMove { name: to, .. }),
                    ) => repo.rename_branch(branch_short(from), branch_short(to), oid),
                    _ => Err(EngineError::NotAllowed(msg!(
                        "engine-undo-bad-entry",
                        description = self.description
                    ))),
                }
            }
            "branch.delete" => {
                let mut outcome = repo.restore_refs(&request)?;
                // The ref is already recreated / deleted: the configuration only gives warnings.
                if let Some(r) = self.refs.first() {
                    outcome.warnings.extend(repo.restore_branch_config(
                        branch_short(&r.name),
                        &self.branch_config,
                        redo,
                    ));
                }
                Ok(outcome)
            }
            "branch.create" if self.upstream.is_some() => {
                let mut outcome = repo.restore_refs(&request)?;
                // The ref is already deleted / recreated: the upstream only gives warnings.
                outcome
                    .warnings
                    .extend(self.restore_create_upstream(repo, redo));
                Ok(outcome)
            }
            "remote.add" | "remote.remove" => {
                let Some(snapshot) = self.remote.clone() else {
                    return Err(EngineError::NotAllowed(msg!(
                        "engine-undo-bad-entry",
                        description = self.description
                    )));
                };
                let dependents = self.command == "remote.remove";
                // Undo of an addition and redo of a removal remove the remote.
                if redo == dependents {
                    remove_remote(repo, &snapshot, request.command, dependents, request.reflog)
                } else {
                    let (outcome, written) = restore_remote(
                        repo,
                        &snapshot,
                        request.command,
                        dependents,
                        request.reflog,
                    )?;
                    // Undo of a removal with skipped upstreams: the redo must compare (and
                    // remove) only what was actually put back.
                    if dependents {
                        self.remote = Some(written);
                    }
                    Ok(outcome)
                }
            }
            "branch.reset" => {
                let Some(record) = self.reset.clone() else {
                    return Err(EngineError::NotAllowed(msg!(
                        "engine-undo-bad-entry",
                        description = self.description
                    )));
                };
                Self::check_unchanged(repo, &request, msg!("engine-busy-reset"))?;
                let outcome = repo.reset(&ResetRequest {
                    mode: record.mode,
                    refs: request,
                    files: Vec::new(),
                    strict: true,
                })?;
                let mut warnings = outcome.warnings;
                if !redo
                    && record.mode == ResetMode::Hard
                    && let Some(backup) = &self.backup
                {
                    warnings.push(msg!(
                        "engine-reset-undo-backup",
                        backup = backup,
                        stash = record.stash.map(|s| s.to_string()).unwrap_or_default()
                    ));
                }
                Ok(RestoreOutcome {
                    backups: outcome.backups,
                    warnings,
                })
            }
            _ if self.moves_worktree() => {
                Self::check_unchanged(repo, &request, msg!("engine-busy-branch-move"))?;
                repo.reset_branch(&request)
            }
            _ => repo.restore_refs(&request),
        }
    }

    /// For undo and redo of checkout and rename, which do not go through
    /// [`GitRefs::restore_refs`](crate::GitRefs::restore_refs): the same checks ([`check_no_operation`], HEAD and the refs
    /// as the operation left them, otherwise [`EngineError::UndoConflict`]) before
    /// calling the backend, which re-checks the refs right before writing.
    fn check_unchanged(
        repo: &dyn GitBackend,
        request: &RefRestore,
        what: Message,
    ) -> Result<(), EngineError> {
        check_no_operation(request.command, what, repo.status()?.operation)?;
        let refs = repo.refs()?;
        let head = HeadState::from_head(&repo.head()?);
        let conflicts = request.conflicts(&head, |name| {
            Ok(refs.iter().find(|r| r.name == name).map(|r| r.target))
        })?;
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            })
        }
    }

    /// The request to the backend to undo the entry: from "after" to "before".
    pub fn undo_request(&self) -> RefRestore {
        RefRestore {
            command: "edit.undo",
            head_expected: self.head.after.clone(),
            head_target: self.head.before.clone(),
            refs: self
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: r.after,
                    target: r.before,
                })
                .collect(),
            reflog: format!("project: undo {}", self.description),
        }
    }

    /// The request to the backend to redo the undone entry: from "before" to "after".
    pub fn redo_request(&self) -> RefRestore {
        RefRestore {
            command: "edit.redo",
            head_expected: self.head.before.clone(),
            head_target: self.head.after.clone(),
            refs: self
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: r.before,
                    target: r.after,
                })
                .collect(),
            reflog: format!("project: redo {}", self.description),
        }
    }
}

/// The journal of an open repo: the done entries (the last is the one `edit.undo` undoes) and
/// the undone ones (the last is the one `edit.redo` redoes).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Journal {
    done: Vec<JournalEntry>,
    undone: Vec<JournalEntry>,
    next_id: u64,
}

impl Journal {
    /// Maximum entries kept (done and undone together); the oldest drop out.
    pub const MAX_ENTRIES: usize = 100;

    /// Empty journal.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty journal that numbers entries from `next_id` (the session continues the numbering
    /// when it opens another repo, so an id never refers to two different entries).
    pub(crate) fn starting_at(next_id: u64) -> Self {
        Self {
            next_id,
            ..Self::default()
        }
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Records a new operation and empties the undone entries (they can no longer be redone:
    /// they started from a state the new operation changed). Returns the id given to the
    /// entry.
    pub fn record(&mut self, mut entry: JournalEntry) -> u64 {
        self.next_id += 1;
        entry.id = self.next_id;
        self.done.push(entry);
        self.undone.clear();
        if self.done.len() > Self::MAX_ENTRIES {
            let extra = self.done.len() - Self::MAX_ENTRIES;
            self.done.drain(..extra);
        }
        self.next_id
    }

    /// The done entries, from the oldest; the last is the one `edit.undo` undoes.
    pub fn done(&self) -> &[JournalEntry] {
        &self.done
    }

    /// The undone entries; the last is the one `edit.redo` redoes.
    pub fn undone(&self) -> &[JournalEntry] {
        &self.undone
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Undoes the last done entry: asks the backend to bring the refs back to the "before" values
    /// ([`JournalEntry::undo_request`]) or, for a discard, to put the backup files back
    /// ([`JournalEntry::worktree_request`]). If it succeeds the entry moves among the undone ones, with any
    /// backup refs created in `undo_backups`; if not (also for
    /// [`EngineError::UndoConflict`]) it stays where it is and nothing has changed. Without entries,
    /// [`EngineError::NotAllowed`]. Returns the entry and the outcome (backups created now, warnings).
    pub fn undo(
        &mut self,
        repo: &mut dyn GitBackend,
    ) -> Result<(JournalEntry, RestoreOutcome), EngineError> {
        let entry = self
            .done
            .last_mut()
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-undo-nothing")))?;
        let outcome = entry.apply(repo, false)?;
        let mut entry = self.done.pop().expect("there is at least one entry");
        entry.undo_backups.extend(outcome.backups.iter().cloned());
        self.undone.push(entry.clone());
        Ok((entry, outcome))
    }

    /// Redoes the last undone entry, like [`Journal::undo`] in reverse
    /// ([`JournalEntry::redo_request`]): the refs must still hold "before".
    pub fn redo(
        &mut self,
        repo: &mut dyn GitBackend,
    ) -> Result<(JournalEntry, RestoreOutcome), EngineError> {
        let entry = self
            .undone
            .last_mut()
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-undo-nothing-to-redo")))?;
        let outcome = entry.apply(repo, true)?;
        let mut entry = self.undone.pop().expect("there is at least one entry");
        entry.undo_backups.extend(outcome.backups.iter().cloned());
        self.done.push(entry.clone());
        Ok((entry, outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitIndex;
    use crate::{CommitRequest, MemoryBackend};

    fn entry(n: u64) -> JournalEntry {
        JournalEntry {
            id: 0,
            command: "commit.create".to_owned(),
            description: Message::plain(format!("entry {n}")),
            head: HeadChange {
                before: HeadState::Branch("refs/heads/main".to_owned()),
                after: HeadState::Branch("refs/heads/main".to_owned()),
            },
            refs: Vec::new(),
            backup: None,
            undo_backups: Vec::new(),
            worktree: None,
            branch_config: Vec::new(),
            remote: None,
            stash: None,
            reset: None,
            upstream: None,
            clean: None,
            lines: None,
            ignore: None,
        }
    }

    #[test]
    fn record_numbers_entries_clears_redo_and_keeps_the_newest() {
        let mut j = Journal::new();
        assert!(!j.can_undo() && !j.can_redo());
        assert_eq!(j.record(entry(1)), 1);
        assert_eq!(j.record(entry(2)), 2);
        j.undone.push(entry(9));
        assert!(j.can_redo());
        j.record(entry(3));
        assert!(!j.can_redo(), "a new operation empties the redo");
        for n in 0..Journal::MAX_ENTRIES as u64 {
            j.record(entry(10 + n));
        }
        assert_eq!(j.done().len(), Journal::MAX_ENTRIES);
        assert_eq!(j.done()[0].description.to_string(), "entry 10");
        assert_eq!(Journal::starting_at(7).record(entry(1)), 8);
    }

    #[test]
    fn entry_of_a_commit() {
        let mut repo = MemoryBackend::sample();
        let out = repo.commit(&CommitRequest::new("Aggiorna")).unwrap();
        let e = JournalEntry::commit(&out, false).unwrap();
        assert_eq!(e.command, "commit.create");
        assert_eq!(
            e.description.to_string(),
            format!("Commit {} “Aggiorna”", out.commit.id.short())
        );
        assert_eq!(
            e.refs,
            [RefChange {
                name: "refs/heads/main".to_owned(),
                before: out.previous,
                after: Some(out.commit.id),
            }]
        );
        let undo = e.undo_request();
        assert_eq!(undo.refs[0].expected, Some(out.commit.id));
        assert_eq!(undo.refs[0].target, out.previous);
        assert!(undo.reflog.starts_with("project: undo Commit "));
        let redo = e.redo_request();
        assert_eq!(redo.refs[0].expected, out.previous);
        assert_eq!(redo.command, "edit.redo");

        // `amend`: description with the previous commit, and the backup.
        let amend = repo
            .commit(&CommitRequest::new("Aggiorna meglio").amend())
            .unwrap();
        let e = JournalEntry::commit(&amend, true).unwrap();
        assert!(e.description.starts_with("Amend "), "{}", e.description);
        assert!(e.description.contains(&out.commit.id.short()));
        assert_eq!(e.backup, amend.backup);

        // An `amend` that changes nothing does not go into the journal.
        let mut same = amend;
        same.previous = Some(same.commit.id);
        assert_eq!(JournalEntry::commit(&same, true), None);

        // Detached HEAD: HEAD moves, no ref.
        let mut detached = out.clone();
        detached.branch = None;
        let e = JournalEntry::commit(&detached, false).unwrap();
        assert!(e.refs.is_empty());
        assert_eq!(e.head.before, HeadState::Detached(out.previous.unwrap()));
        assert_eq!(e.head.after, HeadState::Detached(out.commit.id));
    }

    #[test]
    fn json_shape() {
        let e = entry(1);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": 0,
                "command": "commit.create",
                "description": { "text": "entry 1", "key": "engine-other", "args": { "text": "entry 1" } },
                "head": {
                    "before": { "branch": "refs/heads/main" },
                    "after": { "branch": "refs/heads/main" }
                },
                "refs": [],
                "backup": null,
                "undo_backups": [],
                "worktree": null,
                "branch_config": [],
                "remote": null,
                "stash": null
            })
        );
    }
}
