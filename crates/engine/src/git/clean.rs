//! Deleting untracked files, with backup (`worktree.clean`, `impl-docs/git/discard.md`
//! "Deleting untracked files").
//!
//! `worktree.discard` never touches untracked files: their content exists nowhere else, and
//! `git stash create` does not save them. `worktree.clean` is the safe `git clean` for named
//! files: before deleting, the backends save the exact bytes and mode of every file in a commit
//! (built with a temporary index, the user's index is never touched) held by a
//! `refs/project/backup/clean-...` ref; if the backup fails nothing is deleted. The journal entry
//! ([`CleanBackup`]) lets `edit.undo` recreate the files, only where the path is absent again.
//!
//! Choices, shared by the backends ([`clean_plan`]):
//!
//! - **Only untracked, non-ignored files, as `repo.status` lists them** (the status is read
//!   with `--untracked-files=all`, so every file is its own entry whatever
//!   `status.showUntrackedFiles` says). A folder means the untracked files under it; tracked,
//!   ignored and conflicted files inside it stay. A requested path with no untracked file
//!   (tracked, ignored, conflicted, missing) is refused, and nothing is deleted.
//! - **Nested repositories** (`git status` lists them as one entry `dir/`) are refused, by name
//!   or inside a requested folder: their history is not in the backup. Submodules are tracked,
//!   so they never appear as untracked files.
//! - **Links** are deleted as links (the target is never followed) and saved as links.

use super::path_matches;
use crate::{EngineError, Message, MessageArg};
use project_domain::{EntryKind, Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// An untracked file that `worktree.clean` deletes: the path, the mode it is saved with
/// (`0o100644`, `0o100755` for an executable, `0o120000` for a link) and its size in bytes
/// (for a link, the length of the target).
///
/// JSON: `{"path", "mode", "size"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanedFile {
    pub path: RepoPath,
    pub mode: u32,
    pub size: u64,
}

/// Outcome of [`GitIndex::clean`](crate::GitIndex::clean).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanOutcome {
    /// The files deleted, in path order (all those of the plan, unless `warnings` says
    /// otherwise).
    pub files: Vec<CleanedFile>,
    /// The backup ref (`refs/project/backup/clean-...`) that holds `commit`.
    pub backup: String,
    /// The backup commit: its tree has every file of the plan with its exact bytes and mode
    /// (`git show <commit>:<path>`). It has no parent.
    pub commit: Oid,
    /// The folders removed because the deletion left them empty, deepest first.
    pub dirs: Vec<RepoPath>,
    /// Something that did not go as planned after the backup (a file that could not be
    /// deleted): the file stays, and so does the backup.
    pub warnings: Vec<Message>,
}

/// The backup of a clean in a journal entry: the commit and the deleted files.
///
/// JSON: `{"commit": oid, "files": [CleanedFile]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanBackup {
    pub commit: Oid,
    pub files: Vec<CleanedFile>,
}

/// What `edit.undo`/`edit.redo` of a clean ask of the backend
/// ([`GitIndex::restore_cleaned`](crate::GitIndex::restore_cleaned)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanRestore {
    /// `edit.undo` or `edit.redo`, for errors.
    pub command: &'static str,
    /// `false` (undo): every path must be absent (nothing in its place, no file or link in
    /// place of one of its folders) and the files are recreated from `commit`. `true` (redo):
    /// every file must be untracked and equal to the backup (bytes and mode), and is deleted
    /// again; no new backup.
    pub redo: bool,
    pub commit: Oid,
    pub files: Vec<CleanedFile>,
}

impl CleanRestore {
    /// The file paths, in the order of the entry.
    pub fn paths(&self) -> Vec<RepoPath> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }

    /// [`EngineError::UndoConflict`] with one line for each of `paths`, if not empty.
    pub fn conflict_error(&self, paths: &[RepoPath]) -> Result<(), EngineError> {
        if paths.is_empty() {
            return Ok(());
        }
        Err(EngineError::UndoConflict {
            command: self.command,
            refs: paths
                .iter()
                .map(|p| {
                    if self.redo {
                        msg!("engine-clean-changed-redo", path = p)
                    } else {
                        msg!("engine-clean-recreated", path = p)
                    }
                })
                .collect(),
        })
    }
}

/// The untracked files that `worktree.clean` of `paths` would delete, in path order, with the
/// checks shared by the backends (see the module). Nothing has changed when it fails:
///
/// - empty `paths` → [`EngineError::NotAllowed`];
/// - a nested repository by name or inside a requested folder → [`EngineError::NotAllowed`];
/// - a requested path without untracked files (tracked, ignored, conflicted, missing) →
///   [`EngineError::NotAllowed`].
pub fn clean_plan(status: &Status, paths: &[RepoPath]) -> Result<Vec<RepoPath>, EngineError> {
    if paths.is_empty() {
        return Err(EngineError::NotAllowed(msg!("engine-clean-no-paths")));
    }
    let mut files = Vec::new();
    let mut nested = Vec::new();
    let mut not_untracked = Vec::new();
    for wanted in paths {
        let mut found = false;
        for entry in &status.entries {
            if !matches!(entry.kind, EntryKind::Untracked) {
                continue;
            }
            let bytes = entry.path.as_bytes();
            let (bare, dir) = match bytes.strip_suffix(b"/") {
                Some(bare) => (RepoPath::from_bytes(bare), true),
                None => (entry.path.clone(), false),
            };
            if !path_matches(&bare, wanted) {
                continue;
            }
            found = true;
            if dir {
                nested.push(entry.path.clone());
            } else {
                files.push(entry.path.clone());
            }
        }
        if !found {
            not_untracked.push(wanted.clone());
        }
    }
    if !nested.is_empty() {
        nested.sort();
        nested.dedup();
        return Err(EngineError::NotAllowed(msg!(
            "engine-clean-nested-repo",
            files = list(&nested)
        )));
    }
    if !not_untracked.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-clean-not-untracked",
            files = list(&not_untracked),
            n = not_untracked.len()
        )));
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// [`EngineError::NotAllowed`] if the plan computed now (`now`) is not the confirmed one
/// (`expected`): a file more or less, or one whose mode or size changed.
pub fn check_same_clean_plan(
    now: &[CleanedFile],
    expected: &[CleanedFile],
) -> Result<(), EngineError> {
    if now == expected {
        return Ok(());
    }
    let names = |files: &[CleanedFile]| -> String {
        files
            .iter()
            .map(|f| format!("`{}` ({} B)", f.path, f.size))
            .collect::<Vec<_>>()
            .join(", ")
    };
    Err(EngineError::NotAllowed(msg!(
        "engine-clean-plan-changed",
        confirmed = names(expected),
        now = names(now)
    )))
}

/// The description of a clean for the journal: `Delete “a.txt”` or `Delete 3 untracked files
/// (“a”, “b”, “c”)` (at most three names).
pub fn clean_description(files: &[CleanedFile]) -> Message {
    match files {
        [one] => msg!("engine-journal-clean-one", path = one.path),
        _ => {
            let mut names: Vec<Message> = files
                .iter()
                .take(3)
                .map(|f| msg!("engine-journal-quoted", name = f.path))
                .collect();
            if files.len() > 3 {
                names.push(Message::plain("..."));
            }
            msg!(
                "engine-journal-clean-many",
                n = files.len(),
                names = MessageArg::Joined(names, ", ".into())
            )
        }
    }
}

/// `` `a`, `b` `` for messages.
pub(crate) fn list(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitRead, MemoryBackend};
    use project_domain::{Change, StatusEntry, TrackedInfo};

    fn p(s: &str) -> RepoPath {
        RepoPath::from(s)
    }

    fn untracked(path: &str) -> StatusEntry {
        StatusEntry {
            path: p(path),
            index: Change::Untracked,
            worktree: Change::Untracked,
            kind: EntryKind::Untracked,
        }
    }

    fn status(entries: Vec<StatusEntry>) -> Status {
        let mut status = MemoryBackend::sample().status().unwrap();
        status.entries = entries;
        status
    }

    #[test]
    fn plan_of_files_and_folders() {
        let s = status(vec![
            untracked("-a"),
            untracked("d/x"),
            untracked("d/e/y"),
            untracked("dd"),
            StatusEntry {
                path: p("d/t"),
                index: Change::Unmodified,
                worktree: Change::Modified,
                kind: EntryKind::Ordinary(TrackedInfo {
                    submodule: None,
                    mode_head: 0o100644,
                    mode_index: 0o100644,
                    mode_worktree: 0o100644,
                    oid_head: MemoryBackend::fake_oid(1),
                    oid_index: MemoryBackend::fake_oid(1),
                }),
            },
        ]);
        // A folder: only the untracked files under it (not `dd`, not the tracked `d/t`).
        assert_eq!(
            clean_plan(&s, &[p("d"), p("-a"), p("d/x")]).unwrap(),
            [p("-a"), p("d/e/y"), p("d/x")]
        );
        // A tracked file, a missing one, a prefix that is not a folder: refused, all listed.
        let err = clean_plan(&s, &[p("d/t"), p("nope"), p("d/e/")]).unwrap_err();
        assert!(
            matches!(&err, EngineError::NotAllowed(m) if m.contains("`d/t`") && m.contains("`nope`")),
            "{err}"
        );
        assert!(clean_plan(&s, &[p("d"), p("ignored.log")]).is_err());
        assert!(clean_plan(&s, &[]).is_err());
    }

    #[test]
    fn nested_repos_are_refused() {
        let s = status(vec![untracked("d/x"), untracked("d/inner/")]);
        for wanted in ["d", "d/inner"] {
            assert!(matches!(
                clean_plan(&s, &[p(wanted)]),
                Err(EngineError::NotAllowed(m)) if m.contains("`d/inner/`")
            ));
        }
        // Inside the nested repo: not an untracked file of this repo.
        assert!(matches!(
            clean_plan(&s, &[p("d/inner/f")]),
            Err(EngineError::NotAllowed(m)) if m.contains("`d/inner/f`")
        ));
        assert_eq!(clean_plan(&s, &[p("d/x")]).unwrap(), [p("d/x")]);
    }

    #[test]
    fn same_plan_and_descriptions() {
        let f = |path: &str, size| CleanedFile {
            path: p(path),
            mode: 0o100644,
            size,
        };
        assert!(check_same_clean_plan(&[f("a", 1)], &[f("a", 1)]).is_ok());
        assert!(matches!(
            check_same_clean_plan(&[f("a", 2)], &[f("a", 1)]),
            Err(EngineError::NotAllowed(m)) if m.contains("`a` (1 B)")
        ));
        assert_eq!(clean_description(&[f("a", 1)]).to_string(), "Delete “a”");
        assert_eq!(
            clean_description(&[f("a", 1), f("b", 1), f("c", 1), f("d", 1)]).to_string(),
            "Delete 4 untracked files (“a”, “b”, “c”, ...)"
        );
        let r = CleanRestore {
            command: "edit.undo",
            redo: false,
            commit: MemoryBackend::fake_oid(1),
            files: vec![f("a", 1)],
        };
        assert_eq!(r.paths(), [p("a")]);
        assert!(r.conflict_error(&[]).is_ok());
        assert!(matches!(
            r.conflict_error(&[p("a")]),
            Err(EngineError::UndoConflict { command: "edit.undo", refs }) if refs.len() == 1
        ));
    }

    #[test]
    fn json_shapes() {
        let f = CleanedFile {
            path: p("a"),
            mode: 0o100755,
            size: 3,
        };
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({ "path": "a", "mode": 0o100755, "size": 3 })
        );
    }
}
