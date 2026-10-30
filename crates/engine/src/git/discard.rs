//! Discarding unstaged changes, with backup (`worktree.discard`,
//! `impl-docs/git/discard.md`).
//!
//! Discarding restores the worktree files to the staged version (like `git restore --worktree`):
//! what was in the worktree no longer exists anywhere else. Before writing, the
//! backends save the worktree with `git stash create` (a commit that touches neither the index nor
//! the
//! worktree) and a `refs/project/backup/discard-...` ref that holds it, so gc does not
//! delete it; the journal entry ([`WorktreeBackup`]) lets `edit.undo` put the files back
//! as they were.
//!
//! Choices, shared by the backends ([`discard_plan`]):
//!
//! - **Worktree only.** The index is not touched: an `MM` file becomes `M.` (the staged version
//!   stays). To drop the stage too there is `index.unstage`.
//! - **Only tracked files with unstaged changes** (Y ∉ {`.`, `?`, `!`}): modified,
//!   deleted, type-changed. An untracked file requested by name is rejected: discarding it
//!   would mean deleting it, and `git stash create` does not save untracked files, so there
//!   would be no backup. Inside a folder untracked and ignored files stay where they are.
//! - **Submodules**: rejected if requested by name, skipped inside a folder (`git restore`
//!   does not enter the submodule).
//! - **When `git stash create` cannot save the backup**, nothing is discarded: branch without
//!   commits, conflicted files (even outside the requested paths: "Cannot save the current
//!   index state") and `intent-to-add` files (`git add -N`, "not uptodate. Cannot merge").
//! - `--assume-unchanged`/`--skip-worktree` files do not have Y ≠ `.` and are not touched.

use super::path_matches;
use crate::{EngineError, Message, MessageArg};
use project_domain::{Change, EntryKind, Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// A discarded file: the path, which unstaged change it had (`Modified`, `Deleted`,
/// `TypeChanged`) and the index entry (mode and id) the worktree was restored to.
///
/// JSON: `{"path", "change", "mode", "oid"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardedFile {
    pub path: RepoPath,
    pub change: Change,
    /// Mode of the index entry (e.g. `0o100644`).
    pub mode: u32,
    /// Blob id in the index.
    pub oid: Oid,
}

/// Outcome of [`GitIndex::discard`](crate::GitIndex::discard).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscardOutcome {
    /// The files restored to the staged version, in path order.
    pub files: Vec<DiscardedFile>,
    /// The backup ref (`refs/project/backup/discard-...`) that holds the commit of
    /// `git stash create`.
    pub backup: String,
    /// The commit of `git stash create`: its tree has the worktree content of the tracked
    /// files before the discard (`git show <stash>:<path>`).
    pub stash: Oid,
    /// Only if for some file the worktree bytes are not those `git restore` would recreate
    /// from the `stash` commit (`clean` filters, line-ending conversions such as
    /// `core.autocrlf=input`):
    /// a commit (child of `stash`) with only the files saved byte for byte, without filters. Undo
    /// writes back those exact bytes. The backup ref points to this commit when present.
    #[serde(default)]
    pub raw: Option<Oid>,
}

/// The worktree backup in a journal entry: the commit of `git stash create` and the
/// discarded files. `edit.undo` puts those files back as they were in the commit, `edit.redo`
/// discards them
/// again.
///
/// JSON: `{"stash": oid, "files": [DiscardedFile]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeBackup {
    pub stash: Oid,
    pub files: Vec<DiscardedFile>,
    /// See [`DiscardOutcome::raw`].
    #[serde(default)]
    pub raw: Option<Oid>,
}

/// What `edit.undo`/`edit.redo` of a discard ask of the backend
/// ([`GitIndex::restore_worktree`](crate::GitIndex::restore_worktree)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRestore {
    /// `edit.undo` or `edit.redo`, for errors.
    pub command: &'static str,
    /// `false` (undo): the files must still be as the discard left them (worktree
    /// equal to the index, index entry the recorded one) and go back as in `stash`.
    /// `true` (redo): the files must be as in `stash` (with the same index entry) and
    /// go back to the staged version; `stash` already holds them, no new backup.
    pub redo: bool,
    pub stash: Oid,
    pub files: Vec<DiscardedFile>,
    /// See [`DiscardOutcome::raw`].
    pub raw: Option<Oid>,
}

impl WorktreeRestore {
    /// The file paths, in the order of the entry.
    pub fn paths(&self) -> Vec<RepoPath> {
        self.files.iter().map(|f| f.path.clone()).collect()
    }

    /// The differences between the expected and the current index (`index` gives mode and id of the
    /// entry of
    /// a path, `None` if absent), one per line, for [`EngineError::UndoConflict`].
    pub fn index_conflicts(
        &self,
        mut index: impl FnMut(&RepoPath) -> Option<(u32, Oid)>,
    ) -> Vec<Message> {
        self.files
            .iter()
            .filter(|f| index(&f.path) != Some((f.mode, f.oid)))
            .map(|f| msg!("engine-discard-index-changed", path = f.path))
            .collect()
    }

    /// [`EngineError::UndoConflict`] with `conflicts`, if not empty.
    pub fn conflict_error(&self, conflicts: Vec<Message>) -> Result<(), EngineError> {
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(EngineError::UndoConflict {
                command: self.command,
                refs: conflicts,
            })
        }
    }

    /// The conflict line of a worktree file that is not as expected.
    pub fn worktree_conflict(&self, path: &RepoPath) -> Message {
        if self.redo {
            msg!("engine-discard-worktree-changed-redo", path = path)
        } else {
            msg!("engine-discard-worktree-changed", path = path)
        }
    }
}

/// The files that `worktree.discard` of `paths` would restore to the staged version, in
/// path order, with the checks shared by the backends (see the module). Nothing has changed when it
/// fails:
///
/// - branch without commits, conflicted or `intent-to-add` file in the repo →
///   [`EngineError::NotAllowed`]: `git stash create` would not save the backup;
/// - an untracked or ignored file, or a submodule with changes, requested by name →
///   [`EngineError::NotAllowed`];
/// - no unstaged changes in the paths → [`EngineError::NotAllowed`].
pub fn discard_plan(
    status: &Status,
    paths: &[RepoPath],
) -> Result<Vec<DiscardedFile>, EngineError> {
    if paths.is_empty() {
        return Err(EngineError::NotAllowed(msg!("engine-discard-no-paths")));
    }
    if status.head.target().is_none() {
        return Err(EngineError::NotAllowed(msg!("engine-discard-unborn")));
    }
    let conflicts: Vec<&RepoPath> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| &e.path)
        .collect();
    if !conflicts.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-discard-conflicts",
            files = list(&conflicts)
        )));
    }
    let intent: Vec<&RepoPath> = status
        .entries
        .iter()
        .filter(|e| tracked(&e.kind) && e.worktree == Change::Added)
        .map(|e| &e.path)
        .collect();
    if !intent.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-discard-intent-to-add",
            files = list(&intent)
        )));
    }

    let mut files = Vec::new();
    let mut untracked = Vec::new();
    let mut submodules = Vec::new();
    for entry in &status.entries {
        if !paths.iter().any(|p| path_matches(&entry.path, p)) {
            continue;
        }
        let named = paths.contains(&entry.path);
        let info = match &entry.kind {
            EntryKind::Ordinary(info)
            | EntryKind::Renamed { info, .. }
            | EntryKind::Copied { info, .. } => info,
            EntryKind::Untracked | EntryKind::Ignored => {
                if named {
                    untracked.push(&entry.path);
                }
                continue;
            }
            EntryKind::Unmerged(_) => continue,
        };
        if !entry.worktree.is_change() {
            continue;
        }
        if info.submodule.is_some() {
            if named {
                submodules.push(&entry.path);
            }
            continue;
        }
        files.push(DiscardedFile {
            path: entry.path.clone(),
            change: entry.worktree,
            mode: info.mode_index,
            oid: info.oid_index,
        });
    }
    // An untracked or ignored entry in place of a folder of the path of a file to discard
    // (e.g. `top/d` created after deleting the folder with `top/d/x`), or in place of the file
    // itself (an untracked folder `x/`): `git restore` would delete it to recreate the
    // file, and the backup does not contain it.
    let in_the_way: Vec<&RepoPath> = status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored))
        .map(|e| &e.path)
        .filter(|u| {
            let bytes = u.as_bytes();
            let bare = RepoPath::from_bytes(bytes.strip_suffix(b"/").unwrap_or(bytes));
            files.iter().any(|f| path_matches(&f.path, &bare))
        })
        .collect();
    if !in_the_way.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-discard-in-the-way",
            files = list(&in_the_way)
        )));
    }
    if !untracked.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-discard-untracked",
            files = list(&untracked)
        )));
    }
    if !submodules.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-discard-submodules",
            files = list(&submodules)
        )));
    }
    if files.is_empty() {
        let wanted: Vec<&RepoPath> = paths.iter().collect();
        return Err(EngineError::NotAllowed(msg!(
            "engine-discard-nothing",
            paths = list(&wanted)
        )));
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// [`EngineError::NotAllowed`] if the plan computed now (`now`) is not the confirmed one
/// (`expected`).
pub fn check_same_plan(
    now: &[DiscardedFile],
    expected: &[DiscardedFile],
) -> Result<(), EngineError> {
    if now == expected {
        return Ok(());
    }
    let names = |files: &[DiscardedFile]| -> Vec<String> {
        files.iter().map(|f| format!("`{}`", f.path)).collect()
    };
    Err(EngineError::NotAllowed(msg!(
        "engine-discard-changed",
        confirmed = names(expected).join(", "),
        now = names(now).join(", ")
    )))
}

/// The description of a discard for the journal: `Discard changes to “a.txt”` or `Discard
/// changes to 3 files (“a”, “b”, “c”)` (at most three names).
pub fn discard_description(files: &[DiscardedFile]) -> Message {
    match files {
        [one] => msg!("engine-journal-discard-one", path = one.path),
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
                "engine-journal-discard-many",
                n = files.len(),
                names = MessageArg::Joined(names, ", ".into())
            )
        }
    }
}

fn tracked(kind: &EntryKind) -> bool {
    matches!(
        kind,
        EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
    )
}

fn list(paths: &[&RepoPath]) -> String {
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
    use project_domain::{Head, StatusEntry, TrackedInfo, UnmergedInfo};

    fn p(s: &str) -> RepoPath {
        RepoPath::from(s)
    }

    #[test]
    fn plan_of_the_sample() {
        // README.md `MM`, docs/guide.md `A.`, src/lib.rs `.M`, note.txt `??`.
        let status = MemoryBackend::sample().status().unwrap();
        let files = discard_plan(&status, &[p("README.md"), p("src"), p("docs")]).unwrap();
        let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(paths, [p("README.md"), p("src/lib.rs")]);
        assert!(files.iter().all(|f| f.change == Change::Modified));

        // An untracked file requested by name: rejected; nothing else to discard: rejected.
        let err = discard_plan(&status, &[p("note.txt"), p("README.md")]).unwrap_err();
        assert!(err.to_string().contains("untracked"), "{err}");
        let err = discard_plan(&status, &[p("docs")]).unwrap_err();
        assert!(err.to_string().contains("no unstaged changes"), "{err}");
        // A prefix that is not a folder includes nothing.
        assert!(discard_plan(&status, &[p("READ")]).is_err());
    }

    fn entry(path: &str, worktree: Change, kind: EntryKind) -> StatusEntry {
        StatusEntry {
            path: p(path),
            index: Change::Unmodified,
            worktree,
            kind,
        }
    }

    fn info() -> TrackedInfo {
        TrackedInfo {
            submodule: None,
            mode_head: 0o100644,
            mode_index: 0o100644,
            mode_worktree: 0o100644,
            oid_head: MemoryBackend::fake_oid(1),
            oid_index: MemoryBackend::fake_oid(1),
        }
    }

    #[test]
    fn plan_refusals() {
        let mut status = MemoryBackend::sample().status().unwrap();
        status.entries = vec![
            entry("-a", Change::Deleted, EntryKind::Ordinary(info())),
            entry("d/n", Change::Untracked, EntryKind::Untracked),
        ];
        let files = discard_plan(&status, &[p("-a"), p("d")]);
        // `d` has only an untracked file (skipped), `-a` is deleted in the worktree.
        assert_eq!(files.unwrap()[0].change, Change::Deleted);

        let mut unborn = status.clone();
        unborn.head = Head::Branch {
            name: "main".into(),
            target: None,
        };
        assert!(matches!(
            discard_plan(&unborn, &[p("-a")]),
            Err(EngineError::NotAllowed(m)) if m.contains("has no commits yet")
        ));

        let mut conflict = status.clone();
        conflict.entries.push(StatusEntry {
            path: p("z"),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: 0,
                mode_ours: 0,
                mode_theirs: 0,
                mode_worktree: 0,
                oid_base: MemoryBackend::fake_oid(1),
                oid_ours: MemoryBackend::fake_oid(1),
                oid_theirs: MemoryBackend::fake_oid(1),
            }),
        });
        assert!(matches!(
            discard_plan(&conflict, &[p("-a")]),
            Err(EngineError::NotAllowed(m)) if m.contains("conflicted")
        ));

        let mut intent = status.clone();
        intent
            .entries
            .push(entry("i", Change::Added, EntryKind::Ordinary(info())));
        assert!(matches!(
            discard_plan(&intent, &[p("-a")]),
            Err(EngineError::NotAllowed(m)) if m.contains("intent-to-add")
        ));

        let mut sub = status;
        let mut sub_info = info();
        sub_info.submodule = Some(project_domain::SubmoduleStatus {
            commit_changed: true,
            modified: false,
            untracked: false,
        });
        sub.entries
            .push(entry("s", Change::Modified, EntryKind::Ordinary(sub_info)));
        assert!(matches!(
            discard_plan(&sub, &[p("s")]),
            Err(EngineError::NotAllowed(m)) if m.contains("submodule")
        ));
    }

    #[test]
    fn untracked_in_the_way_is_refused() {
        let mut status = MemoryBackend::sample().status().unwrap();
        status.entries = vec![
            entry("top/d/x", Change::Deleted, EntryKind::Ordinary(info())),
            entry("top/d", Change::Untracked, EntryKind::Untracked),
        ];
        for wanted in ["top", "top/d/x"] {
            assert!(matches!(
                discard_plan(&status, &[p(wanted)]),
                Err(EngineError::NotAllowed(m)) if m.contains("`top/d`")
            ));
        }
        // An untracked folder in place of the file.
        status.entries = vec![
            entry("x", Change::Deleted, EntryKind::Ordinary(info())),
            entry("x/", Change::Untracked, EntryKind::Untracked),
        ];
        assert!(discard_plan(&status, &[p("x")]).is_err());
        // An untracked file next to it (not on the path) does not count.
        status.entries = vec![
            entry("top/d/x", Change::Deleted, EntryKind::Ordinary(info())),
            entry("top/dd", Change::Untracked, EntryKind::Untracked),
        ];
        assert!(discard_plan(&status, &[p("top")]).is_ok());
    }

    #[test]
    fn descriptions() {
        let file = |s: &str| DiscardedFile {
            path: p(s),
            change: Change::Modified,
            mode: 0o100644,
            oid: MemoryBackend::fake_oid(1),
        };
        assert_eq!(
            discard_description(&[file("a")]).to_string(),
            "Discard changes to “a”"
        );
        assert_eq!(
            discard_description(&[file("a"), file("b"), file("c"), file("d")]).to_string(),
            "Discard changes to 4 files (“a”, “b”, “c”, ...)"
        );
    }

    #[test]
    fn restore_conflicts() {
        let f = DiscardedFile {
            path: p("a"),
            change: Change::Modified,
            mode: 0o100644,
            oid: MemoryBackend::fake_oid(1),
        };
        let r = WorktreeRestore {
            command: "edit.undo",
            redo: false,
            stash: MemoryBackend::fake_oid(9),
            files: vec![f],
            raw: None,
        };
        assert!(
            r.index_conflicts(|_| Some((0o100644, MemoryBackend::fake_oid(1))))
                .is_empty()
        );
        assert_eq!(r.index_conflicts(|_| None).len(), 1);
        assert!(r.conflict_error(Vec::new()).is_ok());
        assert!(matches!(
            r.conflict_error(vec![msg!("engine-ref-missing")]),
            Err(EngineError::UndoConflict {
                command: "edit.undo",
                ..
            })
        ));
    }

    #[test]
    fn json_shapes() {
        let f = DiscardedFile {
            path: p("a"),
            change: Change::Deleted,
            mode: 0o100644,
            oid: MemoryBackend::fake_oid(1),
        };
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            serde_json::json!({
                "path": "a",
                "change": "Deleted",
                "mode": 0o100644,
                "oid": MemoryBackend::fake_oid(1).to_string(),
            })
        );
    }
}
