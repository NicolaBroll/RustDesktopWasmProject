//! `worktree.clean` and its undo/redo on the [`MemoryBackend`], with the same checks as the
//! `CliBackend` ([`clean_plan`], [`check_same_clean_plan`]).
//!
//! The backup commit is a fake id registered in [`MemoryBackend::clean_backups`] with the ref
//! `refs/project/backup/clean-<n>` and the content of every deleted file. Untracked files
//! are saved with mode `0o100644` (the `MemoryBackend` has no executable bit or links for them).

use super::MemoryBackend;
use super::write::entry_order;
use crate::GitRead;
use crate::git::{
    BACKUP_PREFIX, CleanOutcome, CleanRestore, CleanedFile, check_same_clean_plan, clean_plan,
};
use crate::{EngineError, RestoreOutcome};
use project_domain::{Change, EntryKind, Oid, RepoPath, StatusEntry};
use std::collections::BTreeMap;

/// The backup of a clean: ref name, (fake) commit and the content of the deleted files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanStash {
    pub name: String,
    pub commit: Oid,
    pub files: BTreeMap<RepoPath, Vec<u8>>,
}

impl MemoryBackend {
    /// The backups saved by `worktree.clean`, in order.
    pub fn clean_backups(&self) -> &[CleanStash] {
        &self.clean_backups
    }

    /// Removes the file `path` from the worktree (and its untracked entry), as if deleted by
    /// another process.
    pub fn remove_worktree_file(&mut self, path: &RepoPath) {
        self.worktree.remove(path);
        self.entries
            .retain(|e| !(e.path == *path && matches!(e.kind, EntryKind::Untracked)));
    }

    /// Adds an untracked file `path` with `content` (entry and worktree).
    pub fn add_untracked(&mut self, path: impl Into<RepoPath>, content: impl Into<Vec<u8>>) {
        let path = path.into();
        self.worktree.insert(path.clone(), content.into());
        self.entries.retain(|e| e.path != path);
        self.entries.push(untracked_entry(&path));
        self.entries
            .sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
    }

    /// [`GitIndex::clean_plan`](crate::GitIndex::clean_plan).
    pub(super) fn plan_clean(&self, paths: &[RepoPath]) -> Result<Vec<CleanedFile>, EngineError> {
        let files = clean_plan(&self.status()?, paths)?;
        Ok(files
            .into_iter()
            .map(|path| CleanedFile {
                size: self.worktree.get(&path).map_or(0, |c| c.len() as u64),
                mode: 0o100644,
                path,
            })
            .collect())
    }

    /// [`GitIndex::clean`](crate::GitIndex::clean). An error changes nothing.
    pub(super) fn clean_paths(
        &mut self,
        paths: &[RepoPath],
        expected: &[CleanedFile],
    ) -> Result<CleanOutcome, EngineError> {
        let files = self.plan_clean(paths)?;
        check_same_clean_plan(&files, expected)?;
        let n = self.clean_backups.len() as u64 + 1;
        let commit = MemoryBackend::fake_oid(0x434c_4541_4e00_0000 + n);
        let name = format!("{BACKUP_PREFIX}clean-{n}");
        let saved = files
            .iter()
            .map(|f| {
                let content = self.worktree.get(&f.path).cloned().unwrap_or_default();
                (f.path.clone(), content)
            })
            .collect();
        self.clean_backups.push(CleanStash {
            name: name.clone(),
            commit,
            files: saved,
        });
        for f in &files {
            self.remove_worktree_file(&f.path);
        }
        Ok(CleanOutcome {
            files,
            backup: name,
            commit,
            dirs: Vec::new(),
            warnings: Vec::new(),
        })
    }

    /// [`GitIndex::restore_cleaned`](crate::GitIndex::restore_cleaned). An error changes
    /// nothing.
    pub(super) fn restore_clean(
        &mut self,
        request: &CleanRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        let stash = self
            .clean_backups
            .iter()
            .find(|b| b.commit == request.commit)
            .cloned()
            .ok_or_else(|| EngineError::GitWithReason {
                command: "restore".to_owned(),
                reason: msg!("engine-memory-backup-missing", stash = request.commit),
            })?;
        let conflicts: Vec<RepoPath> = request
            .paths()
            .into_iter()
            .filter(|path| {
                let entry = self.entries.iter().find(|e| e.path == *path);
                if request.redo {
                    // As the undo left it: untracked, with the backup's content.
                    !(entry.is_some_and(|e| matches!(e.kind, EntryKind::Untracked))
                        && self.worktree.get(path) == stash.files.get(path))
                } else {
                    // As the clean left it: nothing at that path.
                    entry.is_some() || self.worktree.contains_key(path)
                }
            })
            .collect();
        request.conflict_error(&conflicts)?;
        for path in request.paths() {
            if request.redo {
                self.remove_worktree_file(&path);
            } else {
                let content = stash.files.get(&path).cloned().unwrap_or_default();
                self.add_untracked(path, content);
            }
        }
        Ok(RestoreOutcome::default())
    }
}

fn untracked_entry(path: &RepoPath) -> StatusEntry {
    StatusEntry {
        path: path.clone(),
        index: Change::Untracked,
        worktree: Change::Untracked,
        kind: EntryKind::Untracked,
    }
}

#[cfg(test)]
mod tests {
    use crate::{CleanRestore, EngineError, GitIndex, GitRead, MemoryBackend, clean_description};
    use project_domain::{EntryKind, RepoPath};

    fn p(s: &str) -> RepoPath {
        RepoPath::from(s)
    }

    fn untracked(repo: &MemoryBackend) -> Vec<RepoPath> {
        repo.status()
            .unwrap()
            .entries
            .into_iter()
            .filter(|e| matches!(e.kind, EntryKind::Untracked))
            .map(|e| e.path)
            .collect()
    }

    #[test]
    fn clean_undo_redo() {
        let mut repo = MemoryBackend::sample();
        repo.add_untracked("d/x", "x\n");
        repo.add_untracked("d/y", "yy\n");
        let plan = repo.clean_plan(&[p("d"), p("note.txt")]).unwrap();
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].size, 2);
        assert_eq!(
            clean_description(&plan).to_string(),
            "Delete 3 untracked files (“d/x”, “d/y”, “note.txt”)"
        );
        // Another plan than the confirmed one: nothing deleted.
        assert!(matches!(
            repo.clean(&[p("d")], &plan),
            Err(EngineError::NotAllowed(_))
        ));
        assert_eq!(untracked(&repo).len(), 3);

        let out = repo.clean(&[p("d"), p("note.txt")], &plan).unwrap();
        assert_eq!(out.backup, "refs/project/backup/clean-1");
        assert!(untracked(&repo).is_empty());
        assert_eq!(repo.clean_backups()[0].files[&p("d/y")], b"yy\n");
        // The tracked files are untouched.
        assert!(repo.worktree_file(&p("src/lib.rs")).is_some());

        let mut request = CleanRestore {
            command: "edit.undo",
            redo: false,
            commit: out.commit,
            files: out.files,
        };
        // A file recreated in the meantime: nothing restored.
        repo.add_untracked("d/x", "new\n");
        assert!(matches!(
            repo.restore_cleaned(&request),
            Err(EngineError::UndoConflict { refs, .. }) if refs.len() == 1
        ));
        assert_eq!(repo.worktree_file(&p("d/x")), Some(&b"new\n"[..]));
        repo.remove_worktree_file(&p("d/x"));
        repo.restore_cleaned(&request).unwrap();
        assert_eq!(untracked(&repo), [p("d/x"), p("d/y"), p("note.txt")]);
        assert_eq!(repo.worktree_file(&p("d/x")), Some(&b"x\n"[..]));

        // Redo: only if still as in the backup.
        request.redo = true;
        request.command = "edit.redo";
        repo.add_untracked("d/y", "changed\n");
        assert!(matches!(
            repo.restore_cleaned(&request),
            Err(EngineError::UndoConflict { .. })
        ));
        repo.add_untracked("d/y", "yy\n");
        repo.restore_cleaned(&request).unwrap();
        assert!(untracked(&repo).is_empty());
    }
}
