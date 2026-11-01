//! Branches on the [`MemoryBackend`] (`branch.create`, `branch.checkout`, `branch.delete`,
//! `branch.rename` and their undo/redo), with the same refusals as the `CliBackend`.
//!
//! The checkout is stricter than git: it refuses ([`EngineError::CheckoutConflict`]) if a file
//! that differs between HEAD and the destination has any change (staged or not), or if the
//! destination contains a file that is untracked or ignored in the worktree; git also accepts
//! some cases where the change matches the destination. The deletion backups
//! go in [`MemoryBackend::commit_backups`] (`refs/project/backup/delete-<n>`).
//! The in-memory repo has no configuration: the upstream follows the
//! rename and disappears with the deletion, like the `branch.<name>` section in git.

use super::write::TreeEntry;
use super::{HEADS, HeadRef, MemoryBackend, REMOTES, TAGS};
use crate::git::{
    BACKUP_PREFIX, BranchDeleted, ConfigEntry, HeadState, RestoreOutcome, check_new_branch_name,
    check_new_tag_name, check_no_operation, tag_ref,
};
use crate::{EngineError, Message};
use project_domain::{EntryKind, Oid, RepoPath};
use std::collections::{BTreeMap, BTreeSet};

impl MemoryBackend {
    /// Error if `full` cannot be created: it already exists, or a ref exists that contains it or that
    /// it contains (`a` for `a/b`, `a/b` for `a`), as for git.
    fn check_free(&self, full: &str) -> Result<(), EngineError> {
        let clash = self.refs.keys().find(|name| {
            *name == full
                || name.starts_with(&format!("{full}/"))
                || full.starts_with(&format!("{name}/"))
        });
        match clash {
            Some(name) => Err(EngineError::NotAllowed(msg!(
                "engine-branch-name-taken",
                full = full,
                name = name
            ))),
            None => Ok(()),
        }
    }

    /// The value of the branch `full`, which must be `expected`.
    pub(super) fn check_value(&self, full: &str, expected: &Oid) -> Result<(), EngineError> {
        match self.refs.get(full) {
            Some(entry) if entry.target == *expected => Ok(()),
            Some(entry) => Err(EngineError::NotAllowed(msg!(
                "engine-branch-changed",
                full = full,
                expected = expected.short(),
                now = entry.target.short()
            ))),
            None => Err(EngineError::NotAllowed(msg!(
                "engine-branch-missing",
                full = full
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

    pub(super) fn create_tag_ref(
        &mut self,
        name: &str,
        target: &Oid,
        message: Option<&str>,
    ) -> Result<Oid, EngineError> {
        check_new_tag_name("tag.create", name)?;
        let full = tag_ref(name);
        self.check_free(&full)?;
        if !self.commits.contains_key(target) {
            return Err(EngineError::UnknownRev(target.to_string()));
        }
        let tag_object = message.map(|_| {
            let n = u64::try_from(self.tag_objects.len()).unwrap_or(u64::MAX);
            let id = Self::fake_oid(0x5441_4700_0000_0000 + n);
            self.tag_objects.insert(id, *target);
            id
        });
        self.set_ref(full, *target, tag_object);
        Ok(tag_object.unwrap_or(*target))
    }

    pub(super) fn checkout_head(
        &mut self,
        target: &HeadState,
    ) -> Result<RestoreOutcome, EngineError> {
        check_no_operation(
            "branch.checkout",
            msg!("engine-busy-checkout"),
            self.operation,
        )?;
        let unmerged: Vec<String> = self
            .entries
            .iter()
            .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
            .map(|e| e.path.to_string())
            .collect();
        if !unmerged.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-checkout-conflicts",
                files = unmerged.join(", ")
            )));
        }
        let (head, oid, label) = match target {
            HeadState::Branch(full) => {
                let name = full.strip_prefix(HEADS).ok_or_else(|| {
                    EngineError::NotAllowed(msg!("engine-branch-not-local", full = full))
                })?;
                let entry = self.refs.get(full).ok_or_else(|| {
                    EngineError::NotAllowed(msg!("engine-branch-no-local", name = name))
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
        // A detached HEAD on commits that no branch, tag or remote branch keeps: saved first.
        let mut outcome = RestoreOutcome::default();
        if let HeadRef::Detached(current) = self.head
            && *target != HeadState::Detached(current)
        {
            let kept: BTreeSet<Oid> = self.reachable_from(
                self.refs
                    .iter()
                    .filter(|(name, _)| [HEADS, TAGS, REMOTES].iter().any(|p| name.starts_with(p)))
                    .map(|(_, entry)| entry.target),
            );
            let count = self.reachable(current).difference(&kept).count();
            if count > 0 {
                let prefix = format!("{BACKUP_PREFIX}detached-");
                let n = self
                    .commit_backups
                    .iter()
                    .filter(|(b, _)| b.starts_with(&prefix))
                    .count()
                    + 1;
                let backup = format!("{prefix}{n}");
                self.commit_backups.push((backup.clone(), current));
                outcome.warnings.push(msg!(
                    "engine-branch-detached-saved",
                    commit = current.short(),
                    count = count,
                    backup = backup
                ));
                outcome.backups.push(backup);
            }
        }
        let index = self.switch_index(&differing, &new_tree);
        self.head = head;
        self.rebase_entries(&index);
        Ok(outcome)
    }

    /// To bring HEAD (index and worktree) to the tree `new_tree`: the files that differ between HEAD and
    /// `new_tree`, and the files with local changes that would prevent it (any change,
    /// staged or not, to a differing file; an untracked or ignored one that is a file, a
    /// folder of a file or a file in place of a folder of `new_tree`).
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
                // The file itself, a folder of it that becomes a file (`d/x` untracked and
                // `d` in the destination), or a file in place of a folder of the destination.
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

    /// The index after switching to `new_tree` (the `differing` files taken from there, the others
    /// as they were), and the worktree of those files becomes clean. To call after [`Self::switch_plan`]
    /// without conflicted files, then move HEAD and call `rebase_entries` with the index.
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
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-delete-current",
                name = name
            )));
        }
        let full = format!("{HEADS}{name}");
        if let Some(w) = self.worktree_of_branch(&full) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-branch-delete-other-worktree",
                name = name,
                worktree = w.path
            )));
        }
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

    /// The `branch.<name>.*` "configuration" of the in-memory repo: the upstream, as git
    /// would write it (`remote` and `merge`).
    fn branch_config(&self, name: &str) -> Vec<ConfigEntry> {
        self.upstream_entries(name)
    }

    /// [`GitRead::local_upstream`](crate::GitRead::local_upstream): the upstream as git would
    /// write it (`remote` and `merge`; a remote with `/` in the name is recognized).
    pub(super) fn upstream_entries(&self, name: &str) -> Vec<ConfigEntry> {
        let Some(upstream) = self.upstream_config(name) else {
            return Vec::new();
        };
        vec![
            ConfigEntry {
                key: format!("branch.{name}.remote"),
                value: Some(upstream.remote),
            },
            ConfigEntry {
                key: format!("branch.{name}.merge"),
                value: Some(upstream.merge),
            },
        ]
    }

    /// Every write of the upstream configuration fails with `message` (`None`: they succeed
    /// again), as `git config` does with a locked or read-only `.git/config`.
    pub fn fail_config_writes(&mut self, message: Option<&str>) {
        self.config_write_failure = message.map(str::to_owned);
    }

    /// [`GitRefs::write_upstream`](crate::GitRefs::write_upstream): the in-memory repo stores
    /// only a complete upstream (`remote` + a branch in `merge`) or none.
    pub(super) fn replace_upstream(
        &mut self,
        name: &str,
        expected: &[ConfigEntry],
        entries: &[ConfigEntry],
    ) -> Result<Vec<Message>, EngineError> {
        let current = self.upstream_entries(name);
        if current != expected {
            return Err(EngineError::NotAllowed(msg!(
                "engine-upstream-changed",
                name = name.escape_debug(),
                expected = crate::upstream_label(expected).unwrap_or_default(),
                now = crate::upstream_label(&current).unwrap_or_default()
            )));
        }
        if let Some(message) = &self.config_write_failure {
            return Err(EngineError::Git {
                command: "config".to_owned(),
                stderr: message.clone(),
            });
        }
        let full = format!("{HEADS}{name}");
        if entries.is_empty() {
            self.upstreams.remove(&full);
            return Ok(Vec::new());
        }
        let value = |suffix: &str| {
            entries
                .iter()
                .find(|e| e.key == format!("branch.{name}.{suffix}"))
                .and_then(|e| e.value.clone())
        };
        match (value("remote"), value("merge")) {
            (Some(remote), Some(merge)) if merge.starts_with(HEADS) => {
                let branch = &merge[HEADS.len()..];
                self.upstreams
                    .insert(full, format!("{REMOTES}{remote}/{branch}"));
                Ok(Vec::new())
            }
            _ => Err(EngineError::NotAllowed(msg!(
                "engine-branch-memory-no-upstream",
                name = name
            ))),
        }
    }

    pub(super) fn set_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<Message> {
        let current = self.branch_config(name);
        let full = format!("{HEADS}{name}");
        if remove {
            if current == entries {
                self.upstreams.remove(&full);
                Vec::new()
            } else {
                vec![msg!("engine-branch-config-changed", name = name)]
            }
        } else if entries.is_empty() {
            Vec::new()
        } else if !current.is_empty() {
            vec![msg!("engine-branch-config-exists", name = name)]
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
                _ => vec![msg!("engine-branch-memory-no-upstream", name = name)],
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
        let entry = self.refs.remove(&old).expect("checked above");
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
