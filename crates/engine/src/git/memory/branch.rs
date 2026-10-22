//! Branch sul [`MemoryBackend`] (`branch.create`, `branch.checkout`, `branch.delete`,
//! `branch.rename` e i loro undo/redo), con gli stessi rifiuti del `CliBackend`.
//!
//! Il checkout è più severo di git: rifiuta ([`EngineError::CheckoutConflict`]) se un file
//! diverso fra HEAD e la destinazione ha una modifica qualsiasi (in stage o no), o se la
//! destinazione contiene un file che nel worktree è non tracciato o ignorato; git accetta
//! anche alcuni casi in cui la modifica coincide con la destinazione. I backup delle
//! eliminazioni vanno in [`MemoryBackend::commit_backups`] (`refs/project/backup/delete-<n>`).
//! Il repo in memoria non ha worktree aggiuntivi né configurazione: l'upstream segue la
//! rinomina e sparisce con l'eliminazione, come la sezione `branch.<nome>` in git.

use super::write::TreeEntry;
use super::{HEADS, HeadRef, MemoryBackend, REMOTES};
use crate::EngineError;
use crate::git::{
    BACKUP_PREFIX, BranchDeleted, ConfigEntry, HeadState, RestoreOutcome, check_new_branch_name,
    check_no_operation, short,
};
use project_domain::{EntryKind, Oid, RepoPath};
use std::collections::{BTreeMap, BTreeSet};

impl MemoryBackend {
    /// Errore se `full` non si può creare: esiste già, o esiste un ref che lo contiene o che
    /// esso contiene (`a` per `a/b`, `a/b` per `a`), come per git.
    fn check_free(&self, full: &str) -> Result<(), EngineError> {
        let clash = self.refs.keys().find(|name| {
            *name == full
                || name.starts_with(&format!("{full}/"))
                || full.starts_with(&format!("{name}/"))
        });
        match clash {
            Some(name) => Err(EngineError::NotAllowed(format!(
                "il nome `{full}` è già preso da `{name}`, niente modificato (un ref non si \
                 sovrascrive)"
            ))),
            None => Ok(()),
        }
    }

    /// Il valore del branch `full`, che deve essere `expected`.
    pub(super) fn check_value(&self, full: &str, expected: &Oid) -> Result<(), EngineError> {
        match self.refs.get(full) {
            Some(entry) if entry.target == *expected => Ok(()),
            Some(entry) => Err(EngineError::NotAllowed(format!(
                "`{full}` è cambiato: doveva essere {}, ora è {}; niente modificato",
                short(expected),
                short(&entry.target)
            ))),
            None => Err(EngineError::NotAllowed(format!(
                "non esiste un branch `{full}`, niente modificato"
            ))),
        }
    }

    pub(super) fn unmerged(&self, tip: &Oid, into: &[Oid]) -> usize {
        let merged: BTreeSet<Oid> = into.iter().flat_map(|id| self.reachable(*id)).collect();
        self.reachable(*tip).difference(&merged).count()
    }

    pub(super) fn create_local_branch(
        &mut self,
        name: &str,
        target: &Oid,
    ) -> Result<(), EngineError> {
        check_new_branch_name("branch.create", name)?;
        let full = format!("{HEADS}{name}");
        self.check_free(&full)?;
        if !self.commits.contains_key(target) {
            return Err(EngineError::UnknownRev(target.to_string()));
        }
        self.set_ref(full, *target, None);
        Ok(())
    }

    pub(super) fn checkout_head(
        &mut self,
        target: &HeadState,
    ) -> Result<RestoreOutcome, EngineError> {
        check_no_operation("branch.checkout", "cambierebbe branch", self.operation)?;
        let unmerged: Vec<String> = self
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.to_string())
            .collect();
        if !unmerged.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "ci sono file in conflitto ({}): risolverli prima di cambiare branch, niente \
                 modificato",
                unmerged.join(", ")
            )));
        }
        let (head, oid, label) = match target {
            HeadState::Branch(full) => {
                let name = full.strip_prefix(HEADS).ok_or_else(|| {
                    EngineError::NotAllowed(format!("`{full}` non è un branch locale"))
                })?;
                let entry = self.refs.get(full).ok_or_else(|| {
                    EngineError::NotAllowed(format!(
                        "non esiste un branch locale `{name}`, niente modificato"
                    ))
                })?;
                (
                    HeadRef::Branch(name.to_owned()),
                    entry.target,
                    name.to_owned(),
                )
            }
            HeadState::Detached(oid) => {
                if !self.commits.contains_key(oid) {
                    return Err(EngineError::UnknownRev(oid.to_string()));
                }
                (HeadRef::Detached(*oid), *oid, oid.to_string())
            }
        };
        let new_tree = self.tree_at(oid);
        let (differing, files) = self.switch_plan(&new_tree);
        if !files.is_empty() {
            return Err(EngineError::CheckoutConflict {
                target: label,
                files,
                stderr: String::new(),
            });
        }
        let index = self.switch_index(&differing, &new_tree);
        self.head = head;
        self.rebase_entries(&index);
        Ok(RestoreOutcome::default())
    }

    /// Per portare HEAD (indice e worktree) all'albero `new_tree`: i file diversi fra HEAD e
    /// `new_tree`, e i file con modifiche locali che lo impedirebbero (una modifica qualsiasi,
    /// in stage o no, a un file diverso; un non tracciato o ignorato che è un file, una
    /// cartella di un file o un file al posto di una cartella di `new_tree`).
    pub(super) fn switch_plan(
        &self,
        new_tree: &BTreeMap<RepoPath, TreeEntry>,
    ) -> (BTreeSet<RepoPath>, Vec<String>) {
        let old_tree = self.head_tree();
        let differing: BTreeSet<RepoPath> = old_tree
            .keys()
            .chain(new_tree.keys())
            .filter(|p| old_tree.get(*p) != new_tree.get(*p))
            .cloned()
            .collect();
        let mut files = Vec::new();
        for entry in &self.entries {
            let hit = match &entry.kind {
                // Il file stesso, una sua cartella che diventa un file (`d/x` non tracciato e
                // `d` nella destinazione), o un file al posto di una cartella della destinazione.
                EntryKind::Untracked | EntryKind::Ignored => {
                    let path = entry.path.as_bytes();
                    let inside = |dir: &[u8]| {
                        path.len() > dir.len() && path.starts_with(dir) && path[dir.len()] == b'/'
                    };
                    new_tree.keys().any(|p| {
                        let p = p.as_bytes();
                        p == path
                            || inside(p)
                            || (p.len() > path.len()
                                && p.starts_with(path)
                                && p[path.len()] == b'/')
                    })
                }
                EntryKind::Renamed { original, .. } | EntryKind::Copied { original, .. } => {
                    differing.contains(&entry.path) || differing.contains(original)
                }
                _ => differing.contains(&entry.path),
            };
            if hit {
                files.push(entry.path.to_string());
            }
        }
        (differing, files)
    }

    /// L'indice dopo il passaggio a `new_tree` (i file `differing` presi da lì, gli altri
    /// com'erano), e il worktree di quei file torna pulito. Da chiamare dopo [`Self::switch_plan`]
    /// senza file in conflitto, poi spostare HEAD e chiamare `rebase_entries` con l'indice.
    pub(super) fn switch_index(
        &mut self,
        differing: &BTreeSet<RepoPath>,
        new_tree: &BTreeMap<RepoPath, TreeEntry>,
    ) -> BTreeMap<RepoPath, TreeEntry> {
        let mut index = self.index_tree();
        for path in differing {
            match new_tree.get(path) {
                Some(entry) => {
                    index.insert(path.clone(), *entry);
                }
                None => {
                    index.remove(path);
                }
            }
            self.worktree.remove(path);
        }
        index
    }

    pub(super) fn delete_local_branch(
        &mut self,
        name: &str,
        expected: &Oid,
    ) -> Result<BranchDeleted, EngineError> {
        if self.head == HeadRef::Branch(name.to_owned()) {
            return Err(EngineError::NotAllowed(format!(
                "`{name}` è il branch attivo (HEAD), niente eliminato"
            )));
        }
        let full = format!("{HEADS}{name}");
        self.check_value(&full, expected)?;
        let prefix = format!("{BACKUP_PREFIX}delete-");
        let n = self
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix))
            .count()
            + 1;
        let backup = format!("{prefix}{n}");
        self.commit_backups.push((backup.clone(), *expected));
        self.refs.remove(&full);
        let config = self.branch_config(name);
        self.upstreams.remove(&full);
        Ok(BranchDeleted {
            backup,
            config,
            warnings: Vec::new(),
        })
    }

    /// La "configurazione" `branch.<name>.*` del repo in memoria: l'upstream, come lo
    /// scriverebbe git (`remote` e `merge`).
    fn branch_config(&self, name: &str) -> Vec<ConfigEntry> {
        let Some(upstream) = self.upstreams.get(&format!("{HEADS}{name}")) else {
            return Vec::new();
        };
        let rest = upstream.strip_prefix(REMOTES).unwrap_or(upstream);
        let (remote, branch) = rest.split_once('/').unwrap_or((rest, ""));
        vec![
            ConfigEntry {
                key: format!("branch.{name}.remote"),
                value: Some(remote.to_owned()),
            },
            ConfigEntry {
                key: format!("branch.{name}.merge"),
                value: Some(format!("{HEADS}{branch}")),
            },
        ]
    }

    pub(super) fn set_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<String> {
        let current = self.branch_config(name);
        let full = format!("{HEADS}{name}");
        if remove {
            if current == entries {
                self.upstreams.remove(&full);
                Vec::new()
            } else {
                vec![format!(
                    "la configurazione `branch.{name}.*` è cambiata dopo l'undo: lasciata com'è"
                )]
            }
        } else if entries.is_empty() {
            Vec::new()
        } else if !current.is_empty() {
            vec![format!(
                "esiste già una configurazione `branch.{name}.*`: quella salvata non è stata \
                 riscritta"
            )]
        } else {
            let value = |suffix: &str| {
                entries
                    .iter()
                    .find(|e| e.key == format!("branch.{name}.{suffix}"))
                    .and_then(|e| e.value.clone())
            };
            match (value("remote"), value("merge")) {
                (Some(remote), Some(merge)) => {
                    let branch = merge.strip_prefix(HEADS).unwrap_or(&merge).to_owned();
                    self.upstreams
                        .insert(full, format!("{REMOTES}{remote}/{branch}"));
                    Vec::new()
                }
                _ => vec![format!(
                    "configurazione `branch.{name}.*` senza upstream: non riscritta nel repo in \
                     memoria"
                )],
            }
        }
    }

    pub(super) fn rename_local_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError> {
        check_new_branch_name("branch.rename", to)?;
        let (old, new) = (format!("{HEADS}{from}"), format!("{HEADS}{to}"));
        self.check_value(&old, expected)?;
        let entry = self.refs.remove(&old).expect("controllato sopra");
        if let Err(e) = self.check_free(&new) {
            self.refs.insert(old, entry);
            return Err(e);
        }
        self.refs.insert(new.clone(), entry);
        if let Some(up) = self.upstreams.remove(&old) {
            self.upstreams.insert(new, up);
        }
        if self.head == HeadRef::Branch(from.to_owned()) {
            self.head = HeadRef::Branch(to.to_owned());
        }
        Ok(RestoreOutcome::default())
    }
}

#[cfg(test)]
#[path = "branch_tests.rs"]
mod tests;
