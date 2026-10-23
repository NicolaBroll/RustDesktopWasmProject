//! `edit.undo` / `edit.redo` sul [`MemoryBackend`]: ref e HEAD spostati con gli stessi
//! controlli del `CliBackend` ([`RefRestore::check`], [`RefRestore::conflicts`]) e i ref di
//! backup dei commit che non sarebbero più raggiungibili (in
//! [`MemoryBackend::commit_backups`], con i nomi `refs/project/backup/undo-<n>`).
//!
//! L'indice e il worktree non cambiano, come con `update-ref`: l'indice è quello di prima, e le
//! voci dello stato si ricalcolano rispetto al nuovo HEAD (le modifiche del commit annullato
//! ricompaiono in stage). Le rinomine in stage tornano come cancellazione e aggiunta: il repo
//! in memoria non riconosce le rinomine. Il repo in memoria non ha ref simbolici né replace
//! object: per la raggiungibilità contano i ref, i nuovi valori e i backup.

use super::write::{TreeEntry, ZERO, entry_order, tracked_info};
use super::{HEADS, HeadRef, MemoryBackend, RefEntry};
use crate::EngineError;
use crate::git::{BACKUP_PREFIX, HeadState, RefRestore};
use project_domain::{Change, EntryKind, Oid, RepoPath, StatusEntry, TrackedInfo};
use std::collections::{BTreeMap, BTreeSet};

impl MemoryBackend {
    /// [`GitBackend::restore_refs`](crate::GitBackend::restore_refs). Un errore non cambia
    /// niente.
    pub(super) fn restore(&mut self, request: &RefRestore) -> Result<Vec<String>, EngineError> {
        request.check(self.operation)?;
        let head = self.head_state();
        let conflicts =
            request.conflicts(&head, |name| Ok(self.refs.get(name).map(|r| r.target)))?;
        if !conflicts.is_empty() {
            return Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            });
        }

        // Cosa resta raggiungibile dopo: i nuovi valori, gli altri ref e i backup.
        let moved: BTreeSet<&str> = request.refs.iter().map(|r| r.name.as_str()).collect();
        let mut keep = request.targets();
        keep.extend(
            self.refs
                .iter()
                .filter(|(name, _)| !moved.contains(name.as_str()))
                .map(|(_, entry)| entry.target),
        );
        keep.extend(self.commit_backups.iter().map(|(_, oid)| *oid));
        let reachable: BTreeSet<Oid> = keep
            .iter()
            .flat_map(|k| self.reachable(*k))
            .chain(keep.iter().copied())
            .collect();
        let kind = request.backup_kind();
        let prefix = format!("{BACKUP_PREFIX}{kind}-");
        let old_index = self.index_tree();

        let mut backups = Vec::new();
        for oid in request.abandoned() {
            if reachable.contains(&oid) {
                continue;
            }
            let n = self
                .commit_backups
                .iter()
                .filter(|(name, _)| name.starts_with(&prefix))
                .count()
                + 1;
            let name = format!("{prefix}{n}");
            self.commit_backups.push((name.clone(), oid));
            backups.push(name);
        }
        for r in &request.refs {
            match r.target {
                Some(target) => {
                    self.refs.insert(
                        r.name.clone(),
                        RefEntry {
                            target,
                            tag_object: None,
                        },
                    );
                }
                None => {
                    self.refs.remove(&r.name);
                }
            }
        }
        if let HeadState::Detached(target) = request.head_target {
            self.head = HeadRef::Detached(target);
        }
        self.rebase_entries(&old_index);
        Ok(backups)
    }

    /// HEAD come la registra il diario.
    pub(super) fn head_state(&self) -> HeadState {
        match &self.head {
            HeadRef::Branch(name) => HeadState::Branch(format!("{HEADS}{name}")),
            HeadRef::Detached(oid) => HeadState::Detached(*oid),
        }
    }

    /// Dopo uno spostamento di HEAD: le voci dello stato rispetto al nuovo HEAD, con lo stesso
    /// indice (`index`, l'albero dell'indice prima dello spostamento) e lo stesso worktree. I
    /// file in conflitto, non tracciati e ignorati restano com'erano; un file `intent-to-add`
    /// (nessuna versione nell'indice) anche.
    pub(super) fn rebase_entries(&mut self, index: &BTreeMap<RepoPath, TreeEntry>) {
        let head = self.head_tree();
        let mut tracked = BTreeMap::new();
        let mut next = Vec::new();
        for entry in std::mem::take(&mut self.entries) {
            if tracked_info(&entry).is_some() {
                tracked.insert(entry.path.clone(), entry);
            } else {
                next.push(entry);
            }
        }
        let unmerged: BTreeSet<RepoPath> = next
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.clone())
            .collect();
        let paths: BTreeSet<RepoPath> = index
            .keys()
            .chain(head.keys())
            .chain(tracked.keys())
            .cloned()
            .collect();
        for path in paths {
            if unmerged.contains(&path) {
                continue;
            }
            let existing = tracked.remove(&path);
            if let Some(entry) = &existing
                && entry.worktree == Change::Added
                && !index.contains_key(&path)
            {
                next.push(entry.clone());
                continue;
            }
            let (mode_head, oid_head) = head.get(&path).copied().unwrap_or((0, ZERO));
            let (mode_index, oid_index) = index.get(&path).copied().unwrap_or((0, ZERO));
            let (worktree, mode_worktree, submodule) = match existing.as_ref().and_then(|e| {
                tracked_info(e).map(|info| (e.worktree, info.mode_worktree, info.submodule))
            }) {
                Some(found) => found,
                None => (Change::Unmodified, mode_index, None),
            };
            let info = TrackedInfo {
                submodule,
                mode_head,
                mode_index,
                mode_worktree,
                oid_head,
                oid_index,
            };
            let entry = StatusEntry {
                path,
                index: Change::Unmodified,
                worktree,
                kind: EntryKind::Ordinary(info.clone()),
            };
            next.extend(self.finish(entry, info));
        }
        next.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        self.entries = next;
    }
}

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
