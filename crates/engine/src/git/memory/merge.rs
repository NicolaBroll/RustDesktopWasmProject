//! Merge sul [`MemoryBackend`] (`merge.start`, `merge.abort`, `merge.continue`, S29), con gli
//! stessi controlli del `CliBackend`.
//!
//! Come il pull in memoria, il merge lavora file per file: un file cambiato in modo diverso
//! dai due lati è un conflitto (anche se git saprebbe unire le righe). Come nel pull in merge
//! (al contrario del pull in rebase, che si annulla), **un conflitto lascia il merge in corso**: i file uniti senza conflitti sono in stage, quelli
//! in conflitto diventano voci `u` dello stato (con i marcatori nel worktree se i contenuti
//! sono noti), `MERGE_HEAD` e `MERGE_MSG` sono tenuti in [`MemoryMerge`]. Il commit
//! ([`GitBackend::commit`](crate::GitBackend::commit)) con un merge in corso crea il commit di
//! merge; l'annullamento salva indice e worktree in un commit di backup a forma di stash e riporta tutti i file
//! tracciati a HEAD (più severo di `git merge --abort`, che tiene le modifiche fuori stage dei
//! file che il merge non ha toccato: qui finiscono solo nel backup).

use super::commit::Tree;
use super::write::{TreeEntry, ZERO, entry_order, tracked_info};
use super::{HEADS, HeadRef, MemoryBackend};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, GitBackend, MergeAborted, MergeMode, MergeOutcome, MergeRequest, OperationState,
    check_merge_state, check_merging, short, split_message,
};
use project_domain::{
    Change, Commit, Conflict, EntryKind, Oid, Operation, RepoPath, Signature, StatusEntry,
    UnmergedInfo,
};

/// Il merge in corso: `MERGE_HEAD` e `MERGE_MSG`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MemoryMerge {
    pub(super) heads: Vec<Oid>,
    pub(super) message: String,
}

/// Un file in conflitto: le versioni della base, nostra e loro (assenti se il file non c'è).
struct Conflicted {
    path: RepoPath,
    base: Option<TreeEntry>,
    ours: Option<TreeEntry>,
    theirs: Option<TreeEntry>,
}

/// Il merge a tre vie file per file: l'albero unito (nei file in conflitto la versione nostra)
/// e i conflitti.
fn merge_partial(base: &Tree, ours: &Tree, theirs: &Tree) -> (Tree, Vec<Conflicted>) {
    let mut paths: Vec<&RepoPath> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    paths.sort();
    paths.dedup();
    let mut out = Tree::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let (b, o, t) = (base.get(path), ours.get(path), theirs.get(path));
        let result = if o == t || t == b {
            o
        } else if o == b {
            t
        } else {
            conflicts.push(Conflicted {
                path: path.clone(),
                base: b.copied(),
                ours: o.copied(),
                theirs: t.copied(),
            });
            o
        };
        if let Some(entry) = result {
            out.insert(path.clone(), *entry);
        }
    }
    (out, conflicts)
}

impl MemoryBackend {
    /// [`GitBackend::merge`].
    pub(super) fn merge_commit(
        &mut self,
        request: &MergeRequest,
    ) -> Result<MergeOutcome, EngineError> {
        check_merge_state(&self.status()?, &request.mode)?;
        if self.head != HeadRef::Branch(request.branch.clone()) {
            return Err(EngineError::NotAllowed(
                "HEAD è cambiata mentre si preparava il merge: niente modificato, riprovare"
                    .to_owned(),
            ));
        }
        let full = format!("{HEADS}{}", request.branch);
        self.check_value(&full, &request.expected)?;
        if !self.commits.contains_key(&request.target) {
            return Err(EngineError::UnknownRev(request.target.to_string()));
        }
        let mut work = self.clone();
        let overwritten = |files: Vec<String>| {
            EngineError::NotAllowed(format!(
                "il merge sovrascriverebbe modifiche locali o file non tracciati ({}): \
                 committarli, metterli da parte o spostarli e riprovare; niente modificato",
                files
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        };
        let mut outcome = MergeOutcome {
            after: request.expected,
            backup: String::new(),
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings: Vec::new(),
        };
        match request.mode {
            MergeMode::FastForward => {
                if !work.reachable(request.target).contains(&request.expected) {
                    return Err(EngineError::NotAllowed(format!(
                        "`{}` non contiene `{}`: il fast-forward non si può fare, niente \
                         modificato",
                        request.target_name, request.branch
                    )));
                }
                let new_tree = work.tree_at(request.target);
                let (differing, files) = work.switch_plan(&new_tree);
                if !files.is_empty() {
                    return Err(overwritten(files));
                }
                let index = work.switch_index(&differing, &new_tree);
                work.set_ref(full, request.target, None);
                work.rebase_entries(&index);
                outcome.after = request.target;
            }
            MergeMode::Merge { .. } => {
                let merged = work.merge_three_way(
                    &full,
                    request.expected,
                    request.target,
                    &request.target_name,
                    &request.message,
                    "il merge",
                )?;
                outcome.after = merged.after;
                outcome.in_progress = merged.in_progress;
                outcome.conflicts = merged.conflicts;
                outcome.message = merged.message;
            }
        }
        let prefix = format!("{BACKUP_PREFIX}merge-");
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

    /// Il commit di merge di `target` in HEAD (il branch `full`, che vale `expected`), file per
    /// file: senza conflitti il commit con `message` e il branch spostato; con conflitti il
    /// merge **resta in corso** (file uniti in stage, voci `u` con i marcatori, `MERGE_HEAD` e
    /// `MERGE_MSG` con il blocco `# Conflicts:`), il branch com'era. `what` per l'errore se il
    /// merge scriverebbe sopra modifiche locali o non tracciati ([`EngineError::NotAllowed`],
    /// niente cambia). Usato da `merge.start` e da `remote.pull`.
    pub(super) fn merge_three_way(
        &mut self,
        full: &str,
        expected: Oid,
        target: Oid,
        target_name: &str,
        message: &str,
        what: &str,
    ) -> Result<MergeOutcome, EngineError> {
        let base = self
            .merge_base(expected, target)
            .map(|b| self.tree_at(b))
            .unwrap_or_default();
        let (merged, conflicts) =
            merge_partial(&base, &self.tree_at(expected), &self.tree_at(target));
        // Nel worktree vanno anche i file in conflitto presenti solo da loro: non devono essere
        // al posto di un non tracciato.
        let mut written = merged.clone();
        for c in &conflicts {
            if let Some(entry) = c.ours.or(c.theirs) {
                written.insert(c.path.clone(), entry);
            }
        }
        let (_, files) = self.switch_plan(&written);
        if !files.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "{what} sovrascriverebbe modifiche locali o file non tracciati ({}): \
                 committarli, metterli da parte o spostarli e riprovare; niente modificato",
                files
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        let (differing, _) = self.switch_plan(&merged);
        let index = self.switch_index(&differing, &merged);
        let mut outcome = MergeOutcome {
            after: expected,
            backup: String::new(),
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings: Vec::new(),
        };
        if conflicts.is_empty() {
            let (summary, body) = split_message(message);
            let id = self.make_commit(vec![expected, target], &merged, None, summary, body)?;
            self.set_ref(full.to_owned(), id, None);
            self.rebase_entries(&index);
            outcome.after = id;
            return Ok(outcome);
        }
        let mut text = Vec::new();
        let mut merge_msg = format!("{message}\n\n# Conflicts:\n");
        for c in &conflicts {
            self.add_conflict(c, target_name);
            text.push(format!("CONFLICT (content): Merge conflict in {}", c.path));
            merge_msg.push_str(&format!("#\t{}\n", c.path));
            outcome.conflicts.push(c.path.clone());
        }
        self.rebase_entries(&index);
        self.operation = Some(Operation::Merge);
        self.merge_state = Some(MemoryMerge {
            heads: vec![target],
            message: merge_msg,
        });
        text.push("Automatic merge failed; fix conflicts and then commit the result.".to_owned());
        outcome.in_progress = true;
        outcome.message = text.join("\n");
        Ok(outcome)
    }

    /// Una voce `u` per il conflitto `c`, con il contenuto del worktree come lo lascia git: i
    /// marcatori se i due lati hanno il file (e i contenuti sono noti), altrimenti il lato che
    /// c'è.
    fn add_conflict(&mut self, c: &Conflicted, theirs_label: &str) {
        let (conflict, x, y) = match (c.base, c.ours, c.theirs) {
            (None, Some(_), Some(_)) => (Conflict::BothAdded, Change::Added, Change::Added),
            (_, None, Some(_)) => (Conflict::DeletedByUs, Change::Deleted, Change::Unmerged),
            (_, Some(_), None) => (Conflict::DeletedByThem, Change::Unmerged, Change::Deleted),
            _ => (Conflict::BothModified, Change::Unmerged, Change::Unmerged),
        };
        let (mode_base, oid_base) = c.base.unwrap_or((0, ZERO));
        let (mode_ours, oid_ours) = c.ours.unwrap_or((0, ZERO));
        let (mode_theirs, oid_theirs) = c.theirs.unwrap_or((0, ZERO));
        let mode_worktree = if mode_ours != 0 {
            mode_ours
        } else {
            mode_theirs
        };
        let content = match (c.ours, c.theirs) {
            (Some((_, o)), Some((_, t))) => match (self.blobs.get(&o), self.blobs.get(&t)) {
                (Some(o), Some(t)) => {
                    let side = |b: &[u8]| {
                        let mut v = b.to_vec();
                        if !v.is_empty() && !v.ends_with(b"\n") {
                            v.push(b'\n');
                        }
                        v
                    };
                    let mut out = b"<<<<<<< HEAD\n".to_vec();
                    out.extend(side(o));
                    out.extend_from_slice(b"=======\n");
                    out.extend(side(t));
                    out.extend_from_slice(format!(">>>>>>> {theirs_label}\n").as_bytes());
                    Some(out)
                }
                _ => None,
            },
            (Some((_, o)), None) => self.blobs.get(&o).cloned(),
            (None, Some((_, t))) => self.blobs.get(&t).cloned(),
            (None, None) => None,
        };
        match content {
            Some(content) => {
                self.worktree.insert(c.path.clone(), content);
            }
            None => {
                self.worktree.remove(&c.path);
            }
        }
        self.entries.retain(|e| e.path != c.path);
        self.entries.push(StatusEntry {
            path: c.path.clone(),
            index: x,
            worktree: y,
            kind: EntryKind::Unmerged(UnmergedInfo {
                conflict,
                submodule: None,
                mode_base,
                mode_ours,
                mode_theirs,
                mode_worktree,
                oid_base,
                oid_ours,
                oid_theirs,
            }),
        });
    }

    /// [`GitBackend::merge_abort`].
    pub(super) fn abort_merge(&mut self) -> Result<MergeAborted, EngineError> {
        check_merging("merge.abort", &self.status()?)?;
        // Come `git merge --abort` (`reset --merge`): rifiutato se un file in stage ha anche
        // modifiche fuori stage.
        let mixed: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.is_staged() && e.is_unstaged())
            .map(|e| format!("`{}`", e.path))
            .collect();
        if !mixed.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "git non annulla il merge: file in stage con anche modifiche fuori stage ({}). \
                 Metterle in stage (`index.stage`) o scartarle (`worktree.discard`) e \
                 riprovare; niente modificato, il merge è ancora in corso",
                mixed.join(", ")
            )));
        }
        let head = self.head_target().ok_or_else(|| {
            EngineError::NotAllowed("HEAD non ha un commit: niente annullato".to_owned())
        })?;
        // Il backup: l'indice (i conflitti con la versione nostra) e sopra il worktree dei file
        // cambiati o in conflitto.
        let index_tree = self.index_tree();
        let mut tree = index_tree.clone();
        let mut files = Vec::new();
        for entry in self.entries.clone() {
            let path = &entry.path;
            let (changed, mode) = match &entry.kind {
                EntryKind::Unmerged(u) => (true, u.mode_worktree),
                _ => match tracked_info(&entry) {
                    Some(info) => (
                        entry.index.is_change() || entry.worktree.is_change(),
                        info.mode_worktree,
                    ),
                    None => (false, 0),
                },
            };
            if !changed {
                continue;
            }
            files.push(path.clone());
            if entry.worktree == Change::Deleted || mode == 0 {
                tree.remove(path);
            } else if let Some(content) = self.worktree.get(path).cloned() {
                let oid = self.store_blob(content);
                tree.insert(path.clone(), (mode, oid));
            }
        }
        let message = format!(
            "project: backup di indice e worktree prima di merge.abort (merge di {})",
            self.merge_state
                .as_ref()
                .map(|m| m.heads.iter().map(short).collect::<Vec<_>>().join(", "))
                .unwrap_or_default()
        );
        let backup_sig = Signature {
            name: "Project".to_owned(),
            email: "backup@project.invalid".to_owned(),
            time: self.next_time(),
            offset_min: 0,
        };
        // Come uno stash (e come il `CliBackend`): un commit con l'indice sopra HEAD, e il
        // backup con il worktree che ha per genitori HEAD e quel commit.
        let first = self.tree_at(head);
        let index_message = "project: indice prima di merge.abort".to_owned();
        let index_id = self.new_commit_id(&[head], &index_message, &index_tree, backup_sig.time);
        self.insert_commit(Commit {
            id: index_id,
            parents: vec![head],
            author: backup_sig.clone(),
            committer: backup_sig.clone(),
            summary: index_message,
            body: String::new(),
        });
        self.set_changes(index_id, super::commit::tree_changes(&first, &index_tree));
        let id = self.new_commit_id(&[head, index_id], &message, &tree, backup_sig.time);
        let (summary, body) = split_message(&message);
        self.insert_commit(Commit {
            id,
            parents: vec![head, index_id],
            author: backup_sig.clone(),
            committer: backup_sig,
            summary,
            body,
        });
        self.set_changes(id, super::commit::tree_changes(&first, &tree));
        let prefix = format!("{BACKUP_PREFIX}merge-abort-");
        let n = self
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix))
            .count()
            + 1;
        let backup = format!("{prefix}{n}");
        self.commit_backups.push((backup.clone(), id));

        // `reset --merge` a HEAD: restano solo non tracciati e ignorati.
        let entries = std::mem::take(&mut self.entries);
        for entry in &entries {
            if !matches!(entry.kind, EntryKind::Untracked | EntryKind::Ignored) {
                self.worktree.remove(&entry.path);
            }
        }
        let mut kept: Vec<StatusEntry> = entries
            .into_iter()
            .filter(|e| matches!(e.kind, EntryKind::Untracked | EntryKind::Ignored))
            .collect();
        kept.sort_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        self.entries = kept;
        self.operation = None;
        self.merge_state = None;
        Ok(MergeAborted {
            backup,
            files,
            warnings: Vec::new(),
        })
    }

    /// [`GitBackend::operation_state`].
    pub(super) fn operation_info(&self) -> OperationState {
        let merge = self
            .merge_state
            .as_ref()
            .filter(|_| self.operation == Some(Operation::Merge));
        OperationState {
            operation: self.operation,
            incoming: merge.map(|m| m.heads.clone()).unwrap_or_default(),
            message: merge.map(|m| m.message.clone()),
            onto: None,
            rebase_branch: None,
        }
    }
}
