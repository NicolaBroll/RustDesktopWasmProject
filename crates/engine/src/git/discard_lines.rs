//! Discarding single lines of the unstaged changes, with backup (`worktree.discard_lines`,
//! `impl-docs/git/discard.md`).
//!
//! The chosen changed lines of the `diff.worktree` diff of one file go back to the index version,
//! the others stay in the worktree. The patch is [`discard_line_patch`] (the
//! [`PatchDirection::Unstage`](project_domain::PatchDirection) rule applied to the worktree diff,
//! whose new side is the worktree) and the new content is computed with the strict
//! [`apply_patch`](crate::apply_patch) on the clean worktree content.
//!
//! Choices, shared by the backends:
//!
//! - **The same refusals as `worktree.discard`** ([`discard_lines_target`], which uses
//!   [`discard_plan`]): branch without commits, conflicted or `intent-to-add` files in the repo,
//!   untracked or submodule, something in the way; and those of the line commands (binaries,
//!   links, type changes, renames, files deleted from the worktree: only a modified file).
//! - **One file, written whole.** The backup is the exact current bytes and mode of the file (a
//!   commit held by `refs/project/backup/discard-...`); the journal entry ([`LinesBackup`]) has the
//!   blobs of the bytes before and of the bytes written, so undo and redo are byte comparisons:
//!   undo writes the old bytes back only if the file has exactly the written ones, redo writes the
//!   new ones again only if it has exactly the old ones. The worktree discard entry
//!   ([`WorktreeBackup`](crate::WorktreeBackup)) is not reused: its undo requires the worktree
//!   equal to the index, which after discarding only some lines is not the case.
//! - **The index is never touched**, and the mode of the worktree file is kept.

use super::discard_plan;
use crate::{EngineError, LinePatch, Message};
use project_domain::{Change, LineKind, Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

pub use super::lines::{DiscardLines, discard_line_patch};

/// Outcome of [`GitIndex::discard_lines`](crate::GitIndex::discard_lines).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardLinesOutcome {
    /// The file, its mode (kept) and the blobs of the bytes before and after.
    #[serde(flatten)]
    pub file: LinesBackup,
    /// The backup ref (`refs/project/backup/discard-...`): a commit with only the file, with its
    /// exact bytes and mode before the write (`git show <backup>:<path>`).
    pub backup: String,
    /// Something not as expected after the write (the file reread is not what was written): the
    /// backup stays.
    pub warnings: Vec<Message>,
}

/// The file of a `worktree.discard_lines` in a journal entry: undo writes back the bytes of
/// `before`, redo those of `after`, each only if the file has exactly the other ones.
///
/// JSON: `{"path", "mode", "before": oid, "after": oid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinesBackup {
    pub path: RepoPath,
    /// The worktree mode (`0o100644` or `0o100755`), the same before and after.
    pub mode: u32,
    /// Blob with the exact worktree bytes before the discard (no filters).
    pub before: Oid,
    /// Blob with the exact bytes written (no filters).
    pub after: Oid,
}

/// What `edit.undo`/`edit.redo` of a `worktree.discard_lines` ask of the backend
/// ([`GitIndex::restore_lines`](crate::GitIndex::restore_lines)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinesRestore {
    /// `edit.undo` or `edit.redo`, for errors.
    pub command: &'static str,
    /// `false` (undo): the file must have the bytes of `after` and gets those of `before`;
    /// `true` (redo): the other way round.
    pub redo: bool,
    pub file: LinesBackup,
}

impl LinesRestore {
    /// The blobs (expected now, to write).
    pub fn blobs(&self) -> (Oid, Oid) {
        if self.redo {
            (self.file.before, self.file.after)
        } else {
            (self.file.after, self.file.before)
        }
    }

    /// [`EngineError::UndoConflict`]: the file is not as the operation (or the undo) left it.
    pub fn conflict(&self) -> EngineError {
        let path = &self.file.path;
        EngineError::UndoConflict {
            command: self.command,
            refs: vec![if self.redo {
                msg!("engine-discard-worktree-changed-redo", path = path)
            } else {
                msg!("engine-discard-worktree-changed", path = path)
            }],
        }
    }
}

/// The file `worktree.discard_lines` of `path` would write, with the checks of [`discard_plan`]
/// (see there) on `status`: it must be a tracked file with only a content change in the worktree
/// ([`Change::Modified`]); otherwise [`EngineError::NotAllowed`] (discard the whole file).
/// Returns the index entry's mode and id.
pub fn discard_lines_target(status: &Status, path: &RepoPath) -> Result<(u32, Oid), EngineError> {
    let files = discard_plan(status, std::slice::from_ref(path))?;
    match &files[..] {
        [f] if f.path == *path && f.change == Change::Modified => Ok((f.mode, f.oid)),
        _ => Err(EngineError::NotAllowed(msg!(
            "engine-discard-lines-whole-file",
            path = path
        ))),
    }
}

/// The reason of [`EngineError::ConfirmRequired`] of `worktree.discard_lines`: how many worktree
/// lines would be lost and how many index lines would come back.
pub fn discard_lines_confirm(path: &RepoPath, prepared: &LinePatch) -> Message {
    let count = |kind| {
        prepared
            .patch
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .filter(|l| l.kind == kind)
            .count()
    };
    msg!(
        "engine-discard-lines-confirm",
        path = path,
        lost = count(LineKind::Removed),
        back = count(LineKind::Added)
    )
}

/// The description for the journal: `Discard lines of “a.txt”`.
pub fn discard_lines_description(path: &RepoPath) -> Message {
    msg!("engine-journal-discard-lines", path = path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitRead, MemoryBackend};

    #[test]
    fn target_of_the_sample() {
        // README.md `MM`, docs/guide.md `A.`, src/lib.rs `.M`, note.txt `??`.
        let status = MemoryBackend::sample().status().unwrap();
        assert!(discard_lines_target(&status, &RepoPath::from("src/lib.rs")).is_ok());
        assert!(discard_lines_target(&status, &RepoPath::from("README.md")).is_ok());
        for path in ["note.txt", "docs/guide.md", "src"] {
            let err = discard_lines_target(&status, &RepoPath::from(path)).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{path}: {err}");
        }
    }

    #[test]
    fn restore_blobs_and_conflict() {
        let file = LinesBackup {
            path: RepoPath::from("a"),
            mode: 0o100644,
            before: MemoryBackend::fake_oid(1),
            after: MemoryBackend::fake_oid(2),
        };
        let undo = LinesRestore {
            command: "edit.undo",
            redo: false,
            file: file.clone(),
        };
        assert_eq!(undo.blobs(), (file.after, file.before));
        let redo = LinesRestore {
            command: "edit.redo",
            redo: true,
            file: file.clone(),
        };
        assert_eq!(redo.blobs(), (file.before, file.after));
        assert!(matches!(
            redo.conflict(),
            EngineError::UndoConflict {
                command: "edit.redo",
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(&file).unwrap(),
            serde_json::json!({
                "path": "a",
                "mode": 0o100644,
                "before": file.before.to_string(),
                "after": file.after.to_string(),
            })
        );
    }
}
