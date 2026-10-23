//! Editor dei conflitti sul [`MemoryBackend`] (`conflict.show`, `conflict.resolve`, S30), con
//! gli stessi controlli del `CliBackend`.
//!
//! Le voci `u` dello stato (da [`merge_three_way`](MemoryBackend::merge_three_way) o
//! [`set_entries`](MemoryBackend::set_entries)) hanno gli id dei tre stadi; i contenuti sono nei
//! blob. Il backup del worktree è un commit senza genitori con il solo file, nello store dei
//! backup dei commit (come gli altri ref `refs/project/backup/...`).

use super::{MemoryBackend, commit::Tree};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, ConflictResolution, ConflictResolved, ConflictVersions, DEFAULT_MARKER_SIZE,
    GitBackend, check_conflict, check_digest,
};
use project_domain::{Commit, EntryKind, Oid, RepoPath, Signature};

/// Modo di un file normale, per un file che il worktree non aveva.
const FILE_MODE: u32 = 0o100644;

impl MemoryBackend {
    /// Il contenuto di uno stadio: `None` se manca (modo 0).
    fn stage_blob(&self, mode: u32, oid: Oid) -> Result<Option<Vec<u8>>, EngineError> {
        if mode == 0 {
            return Ok(None);
        }
        self.blobs
            .get(&oid)
            .cloned()
            .map(Some)
            .ok_or_else(|| EngineError::Git {
                command: "cat-file".to_owned(),
                stderr: format!("fatal: blob sconosciuto {oid}"),
            })
    }

    /// [`GitBackend::conflict_versions`].
    pub(super) fn conflict_files(&self, path: &RepoPath) -> Result<ConflictVersions, EngineError> {
        let status = self.status()?;
        let info = check_conflict("conflict.show", &status, path)?;
        Ok(ConflictVersions {
            path: path.clone(),
            kind: info.conflict,
            base: self.stage_blob(info.mode_base, info.oid_base)?,
            ours: self.stage_blob(info.mode_ours, info.oid_ours)?,
            theirs: self.stage_blob(info.mode_theirs, info.oid_theirs)?,
            worktree: self.worktree.get(path).cloned(),
            marker_size: DEFAULT_MARKER_SIZE,
        })
    }

    /// [`GitBackend::resolve_conflict`]. Un errore non cambia niente.
    pub(super) fn resolve_file(
        &mut self,
        request: &ConflictResolution,
    ) -> Result<ConflictResolved, EngineError> {
        let path = &request.path;
        let status = self.status()?;
        let info = check_conflict("conflict.resolve", &status, path)?.clone();
        let current = self.worktree.get(path).cloned();
        check_digest(path, &request.digest, current.as_deref(), "")?;

        let mut work = self.clone();
        let backup = match current {
            Some(bytes) => Some(work.conflict_backup(path, info.mode_worktree, bytes)?),
            None => None,
        };
        // Il file nuovo nel worktree, e il suo modo nella voce `u` che lo stage legge.
        let mode = match &request.content {
            Some(content) => {
                work.worktree.insert(path.clone(), content.clone());
                if info.mode_worktree == 0 {
                    FILE_MODE
                } else {
                    info.mode_worktree
                }
            }
            None => {
                work.worktree.remove(path);
                0
            }
        };
        for entry in &mut work.entries {
            if let EntryKind::Unmerged(u) = &mut entry.kind
                && entry.path == *path
            {
                u.mode_worktree = mode;
            }
        }
        let index_backup = work.stage_paths(std::slice::from_ref(path))?;
        *self = work;
        Ok(ConflictResolved {
            backup,
            index_backup,
        })
    }

    /// Il backup dei byte del worktree di `path`: un commit senza genitori con il solo file.
    fn conflict_backup(
        &mut self,
        path: &RepoPath,
        mode: u32,
        bytes: Vec<u8>,
    ) -> Result<String, EngineError> {
        let mode = if mode == 0 { FILE_MODE } else { mode };
        let blob = self.store_blob(bytes);
        let tree: Tree = [(path.clone(), (mode, blob))].into_iter().collect();
        let message = format!("project: backup del worktree prima di conflict.resolve\n\n{path}\n");
        let sig = Signature {
            name: "Project".to_owned(),
            email: "backup@project.invalid".to_owned(),
            time: self.next_time(),
            offset_min: 0,
        };
        let id = self.new_commit_id(&[], &message, &tree, sig.time);
        let (summary, body) = crate::git::split_message(&message);
        self.insert_commit(Commit {
            id,
            parents: Vec::new(),
            author: sig.clone(),
            committer: sig,
            summary,
            body,
        });
        self.set_changes(id, super::commit::tree_changes(&Tree::new(), &tree));
        let prefix = format!("{BACKUP_PREFIX}conflict-");
        let n = self
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix))
            .count()
            + 1;
        let name = format!("{prefix}{n}");
        self.commit_backups.push((name.clone(), id));
        Ok(name)
    }
}
