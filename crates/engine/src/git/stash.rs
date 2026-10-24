//! Stash (`stash.list`, `stash.push`, `stash.apply`, `stash.pop`, `stash.drop`, S28): types and
//! checks shared by the backends (`impl-docs/git/stash.md`).
//!
//! Choices:
//!
//! - **A stash is identified by position and id.** `stash@{n}` changes on every push and drop;
//!   the commands receive the position `index` (the one shown) and, optional but recommended,
//!   the full id `expected` seen by the caller. The backend always receives the id: it recomputes
//!   the current position itself right before writing.
//! - **`apply` and `pop` never overwrite anything.** Before calling git: no operation
//!   in progress, no conflicted files, no local changes (staged, unstaged, untracked
//!   or ignored) to the files the stash changes, and no file in place of the untracked
//!   files the stash puts back ([`stash_local_changes`]). Git would do part of these
//!   checks itself, but not all before writing: with an untracked file already present it puts back the
//!   tracked files and then fails ("could not restore untracked files"), and with a local
//!   change it refuses after having already put back the untracked files.
//! - **Conflicts: left as git leaves them.** The stash is applied with a three-way merge
//!   (stash base, HEAD, stash): if HEAD changed the same files the merge stops with
//!   the conflicted files in the index. There is no operation in progress to abort (`git stash`
//!   does not leave `MERGE_HEAD`), and the conflicted files had no local changes (check
//!   above): the "ours" version is that of HEAD and the stash stays in the list (`pop` does not
//!   remove it), so nothing is lost. The result says it in `conflicts`; they are resolved with
//!   `index.stage` of the file (and then with the conflict editor, S30).
//! - **Journal** (§7): `stash.push` and `stash.drop` go in, `stash.apply` and `stash.pop` do not
//!   (see [`StashRecord`]).

use super::branch::check_no_operation;
use crate::{EngineError, Message};
use project_domain::{Change, EntryKind, Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// The stash ref: it points to the most recent stash, its reflog is the list.
pub const STASH_REF: &str = "refs/stash";

/// A stash of the list (`stash.list`), most recent first.
///
/// JSON: `{"index": 0, "name": "stash@{0}", "id", "base", "message": "On main: test",
/// "branch": "main" | null, "time", "untracked": bool}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashEntry {
    /// The position in the list: `n` of `stash@{n}`, 0 the most recent.
    pub index: usize,
    /// `stash@{n}`.
    pub name: String,
    /// The commit of the stash (the saved worktree); its tree, compared with `base`, gives
    /// the changes of the tracked files (`commit.show`/`diff.commit` with this id).
    pub id: Oid,
    /// The commit of HEAD when the stash was created (the first parent): the UI draws it
    /// next to this commit in the graph.
    pub base: Oid,
    /// The reflog message as `git stash list` shows it: `On <branch>: <message>` or
    /// `WIP on <branch>: <short id> <subject>`.
    pub message: String,
    /// The branch it was created on, read from `message`; `None` with detached HEAD (`(no
    /// branch)`) or with a message in another format (`git stash store -m`).
    pub branch: Option<String>,
    /// Date of the stash commit (Unix seconds).
    pub time: i64,
    /// True if it also contains untracked files (third parent, `include_untracked`).
    pub untracked: bool,
}

impl StashEntry {
    /// The entry at position `index` of commit `id` with parents `parents`; `None` if the
    /// commit has no parents (it is not a stash).
    pub fn new(
        index: usize,
        id: Oid,
        parents: &[Oid],
        time: i64,
        message: impl Into<String>,
    ) -> Option<Self> {
        let message = message.into();
        Some(Self {
            index,
            name: format!("stash@{{{index}}}"),
            id,
            base: *parents.first()?,
            branch: stash_branch(&message),
            message,
            time,
            untracked: parents.len() >= 3,
        })
    }
}

/// The branch of a stash message (`WIP on main: ...`, `On feature/x: ...`); `None` for
/// `(no branch)` or another format. A ref name does not contain `:`, so the first `: `
/// ends the name.
pub fn stash_branch(message: &str) -> Option<String> {
    let rest = message
        .strip_prefix("WIP on ")
        .or_else(|| message.strip_prefix("On "))?;
    let (branch, _) = rest.split_once(": ")?;
    (!branch.is_empty() && branch != "(no branch)" && !branch.contains(' '))
        .then(|| branch.to_owned())
}

/// The stash with id `id` in the list, if present.
pub fn find_stash<'a>(list: &'a [StashEntry], id: &Oid) -> Option<&'a StashEntry> {
    list.iter().find(|s| s.id == *id)
}

/// Parameters of `stash.push`: `{"message"?, "include_untracked"?, "keep_index"?, "paths"?}`.
/// The journal entry keeps them too, for the redo.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StashPush {
    /// The message (one line); without it, git writes `WIP on <branch>: <id> <subject>`.
    pub message: Option<String>,
    /// Also the untracked files (not the ignored ones): `--include-untracked`, which after saving
    /// them removes them from the worktree.
    pub include_untracked: bool,
    /// The staged changes stay in the index and the worktree too (`--keep-index`); the
    /// stash contains them anyway.
    pub keep_index: bool,
    /// Only these files or folders (literally, like `index.stage`); `None` = all.
    pub paths: Option<Vec<RepoPath>>,
}

impl StashPush {
    /// Error if the message cannot be passed to git as is: a single line, without NUL or
    /// control characters (it ends up in a `--message=` argument and in the reflog, which would
    /// reduce it to one line). An empty or whitespace-only message counts as absent.
    pub fn check(&mut self) -> Result<(), EngineError> {
        if let Some(message) = &self.message {
            if message.chars().any(char::is_control) {
                return Err(EngineError::InvalidParams {
                    id: "stash.push",
                    reason: msg!("engine-stash-message-control"),
                });
            }
            if message.trim().is_empty() {
                self.message = None;
            }
        }
        Ok(())
    }

    /// True if the status entry `path` falls within the requested `paths` (all of them without `paths`).
    fn selects(&self, path: &RepoPath) -> bool {
        self.paths
            .as_ref()
            .is_none_or(|paths| paths.iter().any(|p| super::path_matches(path, p)))
    }
}

/// The checks of `stash.push` on the state read before writing, shared by the backends:
/// operation in progress, conflicted files, branch without commits, `intent-to-add` files (with
/// these git does not save the stash: "Cannot save the current index state/worktree state") and
/// nothing to set aside among the chosen files (changes to tracked files, staged or not, and
/// with `include_untracked` the untracked ones). Errors [`EngineError::NotAllowed`], nothing changes.
pub fn check_stash_push(status: &Status, request: &StashPush) -> Result<(), EngineError> {
    const ID: &str = "stash.push";
    check_no_operation(ID, msg!("engine-busy-stash-push"), status.operation)?;
    if status.head.target().is_none() {
        return Err(EngineError::NotAllowed(msg!("engine-stash-unborn")));
    }
    let unmerged = paths_where(status, |e| matches!(e.kind, EntryKind::Unmerged(_)));
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-stash-push-conflicts",
            files = unmerged.join(", ")
        )));
    }
    let intent = paths_where(status, |e| {
        tracked(&e.kind) && e.worktree == Change::Added && e.index == Change::Unmodified
    });
    if !intent.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-stash-intent-to-add",
            files = intent.join(", ")
        )));
    }
    let any = status.entries.iter().any(|e| {
        request.selects(&e.path)
            && match e.kind {
                EntryKind::Untracked => request.include_untracked,
                EntryKind::Ignored | EntryKind::Unmerged(_) => false,
                _ => e.index.is_change() || e.worktree.is_change(),
            }
    });
    if !any {
        let reason = match (request.include_untracked, request.paths.is_some()) {
            (true, false) => msg!("engine-stash-nothing-untracked"),
            (true, true) => msg!("engine-stash-nothing-untracked-paths"),
            (false, false) => msg!("engine-stash-nothing"),
            (false, true) => msg!("engine-stash-nothing-paths"),
        };
        return Err(EngineError::NotAllowed(reason));
    }
    Ok(())
}

/// The checks of `stash.apply`/`stash.pop` on the state: operation in progress, conflicted
/// files. [`EngineError::NotAllowed`], nothing changes.
pub fn check_stash_apply(command: &'static str, status: &Status) -> Result<(), EngineError> {
    check_no_operation(command, msg!("engine-busy-stash-apply"), status.operation)?;
    let unmerged = paths_where(status, |e| matches!(e.kind, EntryKind::Unmerged(_)));
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-stash-apply-conflicts",
            files = unmerged.join(", ")
        )));
    }
    Ok(())
}

/// The files of the status that prevent applying a stash that changes the tracked files
/// `tracked` and puts back the untracked ones `untracked`: every entry (staged change or not,
/// untracked, ignored) on one of those paths, or on a folder that contains them, or
/// inside one of them. The `CliBackend` adds the worktree files that the status does not show (an
/// ignored one, a file in place of a folder).
///
/// It does not block a tracked file that **already** has the stash's version: worktree equal
/// to the index (no unstaged changes) and `in_stash(path, mode, id)` true for its
/// index entry, that is the same entry in the index saved by the stash and in its saved
/// worktree (mode 0: absent from both). This is the case of the staged changes left after a
/// push with `keep_index` (or with `paths`, which saves the whole index anyway): the merge has
/// nothing to change in that file, and without this exception the undo of the push and the pop after a
/// `keep_index` would always be rejected. A file that also has unstaged changes stays
/// blocked.
pub fn stash_local_changes(
    status: &Status,
    tracked: &[RepoPath],
    untracked: &[RepoPath],
    in_stash: impl Fn(&RepoPath, u32, Oid) -> bool,
) -> Vec<String> {
    let targets: Vec<&RepoPath> = tracked.iter().chain(untracked).collect();
    let near = |p: &RepoPath, t: &RepoPath| super::path_matches(p, t) || super::path_matches(t, p);
    let mut hits: Vec<String> = status
        .entries
        .iter()
        .filter(|e| {
            let changed = match &e.kind {
                EntryKind::Untracked | EntryKind::Ignored | EntryKind::Unmerged(_) => true,
                // Only for the tracked files of the stash: an untracked one to put back must not
                // be there at all (with mode 0 `in_stash` would be true for it too).
                EntryKind::Ordinary(info)
                    if e.worktree == Change::Unmodified
                        && !untracked.iter().any(|t| near(&e.path, t))
                        && in_stash(&e.path, info.mode_index, info.oid_index) =>
                {
                    false
                }
                _ => e.index.is_change() || e.worktree.is_change(),
            };
            changed && targets.iter().any(|t| near(&e.path, t))
        })
        .map(|e| e.path.to_string())
        .collect();
    hits.dedup();
    hits
}

/// The message key of [`EngineError::NotAllowed`] when the staged part of a stash does not
/// apply on top of the current index (`restore_index`, "conflicts in index"): the undo of
/// `stash.push` recognizes it to give a message of its own.
pub const INDEX_NOT_RESTORED: &str = "engine-stash-index-not-restored";

/// [`EngineError::NotAllowed`] of `stash.apply`/`stash.pop` when local files would be
/// overwritten (`files`): nothing was modified.
pub fn stash_blocked(command: &str, stash: &str, files: &[String]) -> EngineError {
    EngineError::NotAllowed(msg!(
        "engine-stash-blocked",
        command = command,
        stash = stash,
        files = files
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Request to [`GitBackend::stash_apply`](crate::GitBackend::stash_apply).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashApply {
    /// The command, for messages (`stash.apply`, `stash.pop`, `edit.undo`).
    pub command: &'static str,
    /// The stash to apply: it must still be in the list.
    pub id: Oid,
    /// Also restores the split between staged and unstaged (`--index`); without it, everything comes back unstaged,
    /// except new files, which git leaves staged.
    pub restore_index: bool,
}

/// Outcome of [`GitBackend::stash_apply`](crate::GitBackend::stash_apply).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashApplied {
    /// The files left in conflict (the merge of the stash with HEAD stopped): empty if the
    /// stash applied completely.
    pub conflicts: Vec<String>,
    /// Warnings found after the application.
    pub warnings: Vec<Message>,
}

/// Request to [`GitBackend::stash_drop`](crate::GitBackend::stash_drop).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashDrop {
    /// The command, for messages and the backup's reflog.
    pub command: &'static str,
    /// The stash to remove: it must still be in the list (at any position).
    pub id: Oid,
    /// The kind of the backup ref to create first (`refs/project/backup/<kind>-...`), `None`
    /// to not create it (the redo of a drop: the backup already exists).
    pub backup: Option<&'static str>,
}

/// Outcome of [`GitBackend::stash_drop`](crate::GitBackend::stash_drop).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashDropped {
    /// The backup ref on the stash commit, if created.
    pub backup: Option<String>,
    /// Warnings found after the removal (e.g. the list changed by another process).
    pub warnings: Vec<Message>,
}

/// The stash of a journal entry (`stash.push`, `stash.drop`).
///
/// - **`stash.push`**: undo = `pop` of that stash with `restore_index` (like
///   `git stash pop --index`), only if HEAD is still that of the entry and on commit `base`
///   (so the merge cannot conflict) and the stash is still in the list; the stash
///   commit stays in a `refs/project/backup/undo-...` backup. Redo = `stash.push` again
///   with the same parameters (`push`) and same HEAD: the new stash has another
///   id (the entry is updated) and contains the changes present at the time of the redo.
/// - **`stash.drop`**: undo = `git stash store` of the same commit (held by the backup), only if
///   it is not already in the list: it comes back **on top** (`stash@{0}`), not at the previous
///   position (`git stash store` cannot insert elsewhere). Redo = the drop again, wherever it is now.
/// - **`stash.apply` and `stash.pop` do not go in**: they change only index and worktree and undoing them
///   would mean removing from the files what the stash added, mixed with the changes
///   that were already there or made afterwards; the applied stash stays (apply) or is in the backup
///   `refs/project/backup/stash-...` (pop), so nothing is lost.
///
/// JSON: `{"id", "message", "base", "push": StashPush | null}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashRecord {
    pub id: Oid,
    pub message: String,
    pub base: Oid,
    /// Only for `stash.push`: the parameters, for the redo.
    #[serde(default)]
    pub push: Option<StashPush>,
}

/// Outcome of [`pop_stash`].
#[derive(Debug)]
pub struct StashPopped {
    pub applied: StashApplied,
    /// The outcome of the drop, if done (none with conflicts or if the drop failed).
    pub dropped: Option<StashDropped>,
    /// The drop failed **after** a successful apply: the changes are in the files, the stash is
    /// still in the list. It is not an error of the operation (the apply must not be repeated).
    pub drop_error: Option<EngineError>,
}

/// `apply` of the stash `id` and, if no conflicts remain, its drop (with a backup of kind
/// `backup`): `stash.pop` and the undo of `stash.push`. With conflicts the stash stays in the list
/// (like `git stash pop`) and the drop is not done. An error of the apply is the error (nothing
/// changed); an error of the drop is not: the apply is done, and is in [`StashPopped::drop_error`].
pub fn pop_stash(
    repo: &mut dyn crate::GitBackend,
    command: &'static str,
    id: Oid,
    restore_index: bool,
    backup: Option<&'static str>,
) -> Result<StashPopped, EngineError> {
    let applied = repo.stash_apply(&StashApply {
        command,
        id,
        restore_index,
    })?;
    if !applied.conflicts.is_empty() {
        return Ok(StashPopped {
            applied,
            dropped: None,
            drop_error: None,
        });
    }
    Ok(
        match repo.stash_drop(&StashDrop {
            command,
            id,
            backup,
        }) {
            Ok(dropped) => StashPopped {
                applied,
                dropped: Some(dropped),
                drop_error: None,
            },
            Err(e) => StashPopped {
                applied,
                dropped: None,
                drop_error: Some(e),
            },
        },
    )
}

fn tracked(kind: &EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
    )
}

fn paths_where(
    status: &Status,
    keep: impl Fn(&project_domain::StatusEntry) -> bool,
) -> Vec<String> {
    status
        .entries
        .iter()
        .filter(|e| keep(e))
        .map(|e| format!("`{}`", e.path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, MemoryBackend};

    #[test]
    fn branch_from_message() {
        assert_eq!(stash_branch("On main: test"), Some("main".to_owned()));
        assert_eq!(
            stash_branch("WIP on feature/x: abc1234 Subject: with two colons"),
            Some("feature/x".to_owned())
        );
        assert_eq!(stash_branch("WIP on (no branch): abc1234 x"), None);
        assert_eq!(stash_branch("free message"), None);
        assert_eq!(stash_branch("On : x"), None);
    }

    #[test]
    fn entry_from_parents() {
        let a = MemoryBackend::fake_oid(1);
        let b = MemoryBackend::fake_oid(2);
        let e = StashEntry::new(2, a, &[b, a, a], 5, "On main: x").unwrap();
        assert_eq!(e.name, "stash@{2}");
        assert_eq!((e.base, e.untracked), (b, true));
        assert!(!StashEntry::new(0, a, &[b, a], 5, "x").unwrap().untracked);
        assert_eq!(StashEntry::new(0, a, &[], 5, "x"), None);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["name"], "stash@{2}");
        assert_eq!(json["branch"], "main");
        assert_eq!(json["base"], b.to_string());
    }

    #[test]
    fn push_checks() {
        let status = MemoryBackend::sample().status().unwrap();
        assert!(check_stash_push(&status, &StashPush::default()).is_ok());
        // Only an untracked file: `include_untracked` is needed.
        let only_note = StashPush {
            paths: Some(vec![RepoPath::from("note.txt")]),
            ..StashPush::default()
        };
        let err = check_stash_push(&status, &only_note).unwrap_err();
        assert!(err.to_string().contains("include_untracked"), "{err}");
        let with_untracked = StashPush {
            include_untracked: true,
            ..only_note
        };
        assert!(check_stash_push(&status, &with_untracked).is_ok());

        let mut busy = status.clone();
        busy.operation = Some(project_domain::Operation::Rebase);
        assert!(matches!(
            check_stash_push(&busy, &StashPush::default()),
            Err(EngineError::NotAllowed(_))
        ));
        let empty = MemoryBackend::new().status().unwrap();
        let err = check_stash_push(&empty, &StashPush::default()).unwrap_err();
        assert!(err.to_string().contains("has no commits yet"), "{err}");

        let mut m = StashPush {
            message: Some("  ".to_owned()),
            ..StashPush::default()
        };
        m.check().unwrap();
        assert_eq!(m.message, None);
        let mut m = StashPush {
            message: Some("a\nb".to_owned()),
            ..StashPush::default()
        };
        assert!(matches!(m.check(), Err(EngineError::InvalidParams { .. })));
    }

    #[test]
    fn local_changes_block_the_stash_files() {
        let none = |_: &RepoPath, _: u32, _: Oid| false;
        let status = MemoryBackend::sample().status().unwrap();
        let p = |s: &str| RepoPath::from(s);
        assert_eq!(
            stash_local_changes(&status, &[p("README.md"), p("other.rs")], &[], none),
            ["README.md"]
        );
        // A folder of the stash contains a modified file; an untracked one to put back
        // is already there.
        assert_eq!(
            stash_local_changes(&status, &[p("src")], &[p("note.txt")], none),
            ["src/lib.rs", "note.txt"]
        );
        assert!(stash_local_changes(&status, &[p("Cargo.toml")], &[], none).is_empty());
        // `docs/guide.md` (`A.`) already has the stash's version: it does not block. `README.md` (`MM`)
        // has unstaged changes: it blocks even if the index matches.
        let all = |_: &RepoPath, _: u32, _: Oid| true;
        assert_eq!(
            stash_local_changes(&status, &[p("README.md"), p("docs/guide.md")], &[], all),
            ["README.md"]
        );
        // The exception does not apply to an untracked file to put back: `docs/guide.md` blocks there.
        assert_eq!(
            stash_local_changes(&status, &[], &[p("docs/guide.md")], all),
            ["docs/guide.md"]
        );
    }
}
