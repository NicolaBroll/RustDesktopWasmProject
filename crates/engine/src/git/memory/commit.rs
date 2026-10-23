//! Commit del [`MemoryBackend`]: un commit nuovo dall'indice descritto dalle voci dello stato.
//!
//! Come il `CliBackend`: stessi controlli prima di scrivere ([`check_commit_state`],
//! [`check_amend_published`]), messaggio pulito come `git commit -F -` senza configurazione
//! (`--cleanup=whitespace`), `amend` con lo stesso autore e gli stessi genitori e un ref di
//! backup del commit riscritto. Niente hook né firma. Le date vengono da un orologio finto
//! (un minuto dopo il commit più recente), così i test sono deterministici e la versione web
//! non dipende dall'orologio del sistema.

use super::write::{TreeEntry, ZERO, entry_order, file_type, tracked_info};
use super::{HEADS, HeadRef, MemoryBackend, REMOTES, TAGS, content_oid};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, CommitOutcome, CommitRequest, GitBackend, amend_impact, check_amend_published,
    check_commit_state, cleanup_message, split_message,
};
use project_domain::{
    Change, ChangedFile, Commit, EntryKind, Oid, Operation, RepoPath, Signature, StatusEntry,
};
use std::collections::BTreeMap;

/// Data del primo commit di un repo vuoto (14 novembre 2023), come nel repo di esempio.
const START: i64 = 1_700_000_000;
/// Secondi fra un commit e il successivo.
const STEP: i64 = 60;

pub(super) type Tree = BTreeMap<RepoPath, TreeEntry>;

impl MemoryBackend {
    /// [`GitBackend::commit`] sulle voci dello stato. Un errore non cambia niente.
    pub(super) fn create_commit(
        &mut self,
        request: &CommitRequest,
    ) -> Result<CommitOutcome, EngineError> {
        request.check_message()?;
        let status = self.status()?;
        let previous = self.head_target();
        check_commit_state(&status, previous.is_some(), request)?;
        // Con un merge in corso il commit è il commit di merge: genitori HEAD e `MERGE_HEAD`.
        let merge_heads = match (&self.operation, &self.merge_state) {
            (Some(Operation::Merge), Some(merge)) => Some(merge.heads.clone()),
            (Some(Operation::Merge), None) => {
                return Err(EngineError::NotAllowed(
                    "il repo in memoria non ha `MERGE_HEAD` per questo merge (impostato con \
                     `set_operation`): il commit di merge non è supportato, niente committato"
                        .to_owned(),
                ));
            }
            _ => None,
        };
        // `cherry_pick.continue`/`revert.continue`: il commit conclude l'operazione; per il
        // cherry-pick l'autore è quello del commit originale (come git con `CHERRY_PICK_HEAD`).
        let concluding = request.conclude.is_some() && request.conclude == self.operation;
        let pick_author = match (&self.operation, &self.merge_state) {
            (Some(Operation::CherryPick), Some(pick)) if concluding => pick
                .heads
                .first()
                .and_then(|h| self.commits.get(h))
                .map(|c| c.author.clone()),
            _ => None,
        };
        let current_branch = match &self.head {
            HeadRef::Branch(name) => Some(format!("{HEADS}{name}")),
            HeadRef::Detached(_) => None,
        };
        let mut also_on = Vec::new();
        let old = match previous {
            Some(id) if request.amend => {
                let (published, diverging) =
                    amend_impact(&self.refs_containing(id), current_branch.as_deref());
                check_amend_published(request, &published)?;
                also_on = diverging;
                Some(
                    self.commits
                        .get(&id)
                        .cloned()
                        .ok_or_else(|| EngineError::UnknownRev(id.to_string()))?,
                )
            }
            _ => None,
        };
        let (name, email) = self.identity.clone().ok_or_else(|| EngineError::Git {
            command: "commit".to_owned(),
            stderr: "Author identity unknown\n\n*** Please tell me who you are.\n\nfatal: \
                     nessuna identità nel repo in memoria (`MemoryBackend::set_identity`)"
                .to_owned(),
        })?;
        let message = cleanup_message(&request.message);
        if message.is_empty() {
            return Err(EngineError::empty_commit_message());
        }

        let tree = self.index_tree();
        let now = Signature {
            name,
            email,
            time: self.next_time(),
            offset_min: 0,
        };
        let (parents, parent_tree, author) = match &old {
            Some(old) => (
                old.parents.clone(),
                old.parents
                    .first()
                    .map(|p| self.tree_at(*p))
                    .unwrap_or_default(),
                old.author.clone(),
            ),
            None => (
                previous
                    .into_iter()
                    .chain(merge_heads.iter().flatten().copied())
                    .collect(),
                self.head_tree(),
                pick_author.unwrap_or_else(|| now.clone()),
            ),
        };
        let changes = tree_changes(&parent_tree, &tree);
        if changes.is_empty() && merge_heads.is_none() {
            return Err(if request.amend {
                EngineError::NotAllowed(
                    "`amend` renderebbe vuoto l'ultimo commit (stesso contenuto del genitore), \
                     niente committato"
                        .to_owned(),
                )
            } else {
                EngineError::NothingToCommit
            });
        }

        let (summary, body) = split_message(&message);
        let id = self.new_commit_id(&parents, &message, &tree, now.time);
        let backup = match (&old, previous) {
            (Some(_), Some(previous)) => {
                let prefix = format!("{BACKUP_PREFIX}amend-");
                let n = self
                    .commit_backups
                    .iter()
                    .filter(|(name, _)| name.starts_with(&prefix))
                    .count();
                let name = format!("{prefix}{}", n + 1);
                self.commit_backups.push((name.clone(), previous));
                Some(name)
            }
            _ => None,
        };
        let commit = Commit {
            id,
            parents,
            author,
            committer: now,
            summary,
            body,
        };
        self.insert_commit(commit.clone());
        self.set_changes(id, changes);
        let branch = match self.head.clone() {
            HeadRef::Branch(name) => {
                self.set_branch(&name, id);
                Some(format!("{HEADS}{name}"))
            }
            HeadRef::Detached(_) => {
                self.head = HeadRef::Detached(id);
                None
            }
        };
        self.commit_entries();
        if merge_heads.is_some() || concluding {
            self.operation = None;
            self.merge_state = None;
        }
        Ok(CommitOutcome {
            commit,
            previous,
            branch,
            backup,
            also_on,
        })
    }

    /// I branch locali, i branch remoti e i tag da cui il commit `id` è raggiungibile (nomi
    /// completi), come `git for-each-ref --contains`.
    fn refs_containing(&self, id: Oid) -> Vec<String> {
        self.refs
            .iter()
            .filter(|(name, entry)| {
                [HEADS, REMOTES, TAGS].iter().any(|p| name.starts_with(p))
                    && self.reachable(entry.target).contains(&id)
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// L'albero dell'indice: quello di HEAD con le modifiche in stage.
    pub(super) fn index_tree(&self) -> Tree {
        let mut tree = self.head_tree();
        for entry in &self.entries {
            let Some(info) = tracked_info(entry) else {
                continue;
            };
            if !entry.index.is_change() {
                continue;
            }
            if let EntryKind::Renamed { original, .. } = &entry.kind {
                tree.remove(original);
            }
            if info.mode_index == 0 {
                tree.remove(&entry.path);
            } else {
                tree.insert(entry.path.clone(), (info.mode_index, info.oid_index));
            }
        }
        tree
    }

    /// Un minuto dopo il commit più recente (o [`START`] in un repo vuoto).
    pub(super) fn next_time(&self) -> i64 {
        self.commits
            .values()
            .map(|c| c.committer.time)
            .max()
            .map_or(START, |t| t + STEP)
    }

    /// Un id finto che dipende dal contenuto del commit e non è già usato.
    pub(super) fn new_commit_id(
        &self,
        parents: &[Oid],
        message: &str,
        tree: &Tree,
        time: i64,
    ) -> Oid {
        let mut seed = Vec::new();
        for p in parents {
            seed.extend_from_slice(p.as_bytes());
        }
        seed.extend_from_slice(message.as_bytes());
        for (path, (mode, oid)) in tree {
            seed.extend_from_slice(path.as_bytes());
            seed.extend_from_slice(&mode.to_be_bytes());
            seed.extend_from_slice(oid.as_bytes());
        }
        seed.extend_from_slice(&time.to_be_bytes());
        loop {
            let id = content_oid(&seed);
            if !self.commits.contains_key(&id) {
                return id;
            }
            seed.push(0);
        }
    }

    /// Dopo il commit l'indice è uguale al nuovo HEAD: le modifiche in stage spariscono dallo
    /// stato, quelle fuori stage, i non tracciati e i file `intent-to-add` restano.
    fn commit_entries(&mut self) {
        let entries = std::mem::take(&mut self.entries);
        let mut next = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(mut info) = tracked_info(&entry).cloned() else {
                next.push(entry);
                continue;
            };
            if !entry.index.is_change() {
                next.push(entry);
                continue;
            }
            // Una cancellazione committata: il file non è più tracciato (se è ancora nel
            // worktree c'è già una voce non tracciata a parte).
            if info.mode_index == 0 || entry.worktree == Change::Unmodified {
                continue;
            }
            info.mode_head = info.mode_index;
            info.oid_head = info.oid_index;
            next.push(StatusEntry {
                path: entry.path,
                index: Change::Unmodified,
                worktree: entry.worktree,
                kind: EntryKind::Ordinary(info),
            });
        }
        next.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        self.entries = next;
    }
}

/// I file cambiati da `old` a `new`, per percorso (senza riconoscere le rinomine).
pub(super) fn tree_changes(old: &Tree, new: &Tree) -> Vec<ChangedFile> {
    let mut paths: Vec<&RepoPath> = old.keys().chain(new.keys()).collect();
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let before = old.get(path).copied();
            let after = new.get(path).copied();
            let change = match (before, after) {
                (Some(b), Some(a)) if b == a => return None,
                (Some((bm, _)), Some((am, _))) if file_type(bm) != file_type(am) => {
                    Change::TypeChanged
                }
                (Some(_), Some(_)) => Change::Modified,
                (Some(_), None) => Change::Deleted,
                (None, Some(_)) => Change::Added,
                (None, None) => return None,
            };
            let (old_mode, old_oid) = before.unwrap_or((0, ZERO));
            let (new_mode, new_oid) = after.unwrap_or((0, ZERO));
            Some(ChangedFile {
                path: path.clone(),
                change,
                original: None,
                score: None,
                old_mode,
                new_mode,
                old_oid,
                new_oid,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "commit_tests.rs"]
mod tests;
