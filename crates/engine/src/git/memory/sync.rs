//! Pull and push on the [`MemoryBackend`] (`remote.pull`, `remote.push`, S26), with the same
//! checks as the `CliBackend`.
//!
//! The in-memory merge works file by file (a file changed differently on the two sides is a
//! conflict, even if git could merge the lines); the rebase replays the local commits without
//! merge, oldest first, skipping those that no longer change anything. A merge conflict is
//! left in progress like `merge.start` ([`MemoryBackend::merge_three_way`]); a rebase
//! conflict changes nothing (like the `CliBackend`, which aborts the rebase). The undo of a pull of the active
//! branch ([`MemoryBackend::reset_keep`]) updates index and worktree like the checkout, with the
//! same refusals. The push copies the commits into the in-memory remote and updates the local
//! remote branch; [`MemoryBackend::fail_pre_push`] simulates a `pre-push` hook that rejects.

use super::commit::{Tree, tree_changes};
use super::{HEADS, HeadRef, MemoryBackend, REMOTES, RefEntry};
use crate::git::{
    BACKUP_PREFIX, LogQuery, PullMode, PullOperation, PullOutcome, PullRequest, PullSettings,
    PushKind, PushOutcome, PushRejection, PushRequest, RefRestore, RestoreOutcome, UpstreamConfig,
    check_pull_state, short,
};
use crate::{EngineError, FetchFailure, GitBackend, JobContext, Progress};
use project_domain::{Commit, Oid, Signature};
use std::collections::BTreeSet;

/// The three-way merge of `ours` and `theirs` from `base`, file by file; the conflicted files if
/// changed differently on the two sides.
fn merge_trees(base: &Tree, ours: &Tree, theirs: &Tree) -> Result<Tree, Vec<String>> {
    let mut paths: Vec<_> = base
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
            conflicts.push(path.to_string());
            continue;
        };
        if let Some(entry) = result {
            out.insert(path.clone(), *entry);
        }
    }
    if conflicts.is_empty() {
        Ok(out)
    } else {
        Err(conflicts)
    }
}

impl MemoryBackend {
    /// The pull configuration (`pull.rebase`, `pull.ff`) for all branches.
    pub fn set_pull_settings(&mut self, settings: PullSettings) {
        self.pull_settings = settings;
    }

    /// The values of the configuration key `key` (e.g. `remote.pushDefault`,
    /// `branch.<name>.pushRemote`, `remote.<name>.mirror`); empty removes it.
    pub fn set_config(&mut self, key: &str, values: &[&str]) {
        if values.is_empty() {
            self.config.remove(key);
        } else {
            self.config.insert(
                key.to_owned(),
                values.iter().map(|v| (*v).to_owned()).collect(),
            );
        }
    }

    /// [`GitBackend::config_values`]: the keys set with [`MemoryBackend::set_config`];
    /// `remote.<name>.url` is the URL of the remote ([`MemoryBackend::add_remote`]) and
    /// `remote.<name>.fetch` of a remote without values has the default refspec
    /// (`+refs/heads/*:refs/remotes/<name>/*`).
    pub(super) fn config_of(&self, key: &str) -> Vec<String> {
        if let Some(values) = self.config.get(key) {
            return values.clone();
        }
        if let Some(remote) = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".url"))
            .and_then(|name| self.remotes.get(name))
        {
            return vec![remote.url.clone()];
        }
        key.strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".fetch"))
            .filter(|name| self.remotes.contains_key(*name))
            .map(|name| vec![format!("+refs/heads/*:{REMOTES}{name}/*")])
            .unwrap_or_default()
    }

    /// A `pre-push` hook that rejects every push with `message` (`None`: no hook).
    pub fn fail_pre_push(&mut self, message: Option<&str>) {
        self.pre_push_failure = message.map(str::to_owned);
    }

    pub(super) fn upstream_config(&self, branch: &str) -> Option<UpstreamConfig> {
        let tracking = self.upstreams.get(&format!("{HEADS}{branch}"))?;
        let rest = tracking.strip_prefix(REMOTES)?;
        // The remote with the longest name that is a prefix (`fork/a` before `fork`).
        let remote = self
            .remotes
            .keys()
            .filter(|r| {
                rest.strip_prefix(r.as_str())
                    .is_some_and(|x| x.starts_with('/'))
            })
            .max_by_key(|r| r.len());
        let (remote, merge) = match remote {
            Some(r) => (r.clone(), &rest[r.len() + 1..]),
            None => {
                let (r, b) = rest.split_once('/')?;
                (r.to_owned(), b)
            }
        };
        Some(UpstreamConfig {
            remote,
            merge: format!("{HEADS}{merge}"),
            tracking: Some(tracking.clone()),
        })
    }

    pub(super) fn commits_missing(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError> {
        let excluded: BTreeSet<Oid> = exclude.iter().flat_map(|e| self.reachable(*e)).collect();
        Ok(self
            .log(&LogQuery::all().from_rev(tip.to_string()))?
            .into_iter()
            .filter(|c| !excluded.contains(&c.id))
            .take(limit)
            .collect())
    }

    /// The best common ancestor of `a` and `b` (the most recent, if there are several).
    pub(super) fn merge_base(&self, a: Oid, b: Oid) -> Option<Oid> {
        let common: BTreeSet<Oid> = self
            .reachable(a)
            .intersection(&self.reachable(b))
            .copied()
            .collect();
        common
            .iter()
            .copied()
            .filter(|c| {
                !common
                    .iter()
                    .any(|d| d != c && self.reachable(*d).contains(c))
            })
            .max_by_key(|c| (self.commits[c].committer.time, *c))
    }

    /// A new commit with the tree `tree` (the changed files are computed relative to the first
    /// parent); author `author`, or the committer.
    pub(super) fn make_commit(
        &mut self,
        parents: Vec<Oid>,
        tree: &Tree,
        author: Option<Signature>,
        summary: String,
        body: String,
    ) -> Result<Oid, EngineError> {
        let (name, email) = self.identity.clone().ok_or_else(|| EngineError::Git {
            command: "merge".to_owned(),
            stderr: "Committer identity unknown: no identity in the in-memory repo \
                     (`MemoryBackend::set_identity`)"
                .to_owned(),
        })?;
        let now = Signature {
            name,
            email,
            time: self.next_time(),
            offset_min: 0,
        };
        let message = if body.is_empty() {
            summary.clone()
        } else {
            format!("{summary}\n\n{body}")
        };
        let id = self.new_commit_id(&parents, &message, tree, now.time);
        let first = parents
            .first()
            .map(|p| self.tree_at(*p))
            .unwrap_or_default();
        self.insert_commit(Commit {
            id,
            parents,
            author: author.unwrap_or_else(|| now.clone()),
            committer: now,
            summary,
            body,
        });
        self.set_changes(id, tree_changes(&first, tree));
        Ok(id)
    }

    fn pull_conflict(
        request: &PullRequest,
        operation: PullOperation,
        files: Vec<String>,
    ) -> EngineError {
        let message = files
            .iter()
            .map(|f| format!("CONFLICT (content): Merge conflict in {f}"))
            .collect::<Vec<_>>()
            .join("\n");
        EngineError::PullConflict {
            branch: request.branch.clone(),
            upstream: request
                .upstream_name
                .strip_prefix(REMOTES)
                .unwrap_or(&request.upstream_name)
                .to_owned(),
            operation,
            files,
            message,
        }
    }

    pub(super) fn pull_upstream(
        &mut self,
        request: &PullRequest,
    ) -> Result<PullOutcome, EngineError> {
        check_pull_state(&self.status()?, &request.mode)?;
        let full = format!("{HEADS}{}", request.branch);
        self.check_value(&full, &request.expected)?;
        let on_head = self.head == HeadRef::Branch(request.branch.clone());
        if on_head != request.current {
            return Err(EngineError::NotAllowed(msg!("engine-pull-head-moved")));
        }
        if !request.current && request.mode != PullMode::FastForward {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pull-not-current-ff",
                branch = request.branch
            )));
        }
        // The new value of the branch, computed on a copy: a conflict changes nothing.
        let mut work = self.clone();
        let target = match request.mode {
            PullMode::FastForward => request.upstream,
            PullMode::Merge { .. } => {
                // Like `merge.start`: a conflict leaves the merge in progress.
                let short_name = request
                    .upstream_name
                    .strip_prefix(REMOTES)
                    .unwrap_or(&request.upstream_name);
                let merged = work.merge_three_way(
                    &full,
                    request.expected,
                    request.upstream,
                    short_name,
                    &request.message,
                    msg!("engine-merge-what-pull"),
                )?;
                let backup = Self::pull_backup(&mut work, request.expected);
                *self = work;
                return Ok(PullOutcome {
                    after: merged.after,
                    backup,
                    in_progress: merged.in_progress,
                    conflicts: merged.conflicts,
                    message: merged.message,
                    warnings: Vec::new(),
                });
            }
            PullMode::Rebase { .. } => {
                let theirs = work.reachable(request.upstream);
                let mut todo: Vec<Commit> = work
                    .log(&LogQuery::all().from_rev(request.expected.to_string()))?
                    .into_iter()
                    .filter(|c| !theirs.contains(&c.id) && c.parents.len() <= 1)
                    .collect();
                todo.reverse();
                // The rebase goes through the trees of the commits to replay: a file that one of
                // them adds (even if a later commit removes it) must not be in place of
                // an untracked or ignored one, which would be written and then deleted.
                let mut hit: Vec<String> = Vec::new();
                for commit in &todo {
                    let (_, files) = work.switch_plan(&work.tree_at(commit.id));
                    for file in files {
                        if !hit.contains(&file) {
                            hit.push(file);
                        }
                    }
                }
                if !hit.is_empty() {
                    return Err(EngineError::NotAllowed(msg!(
                        "engine-pull-rebase-untracked",
                        files = hit
                            .iter()
                            .map(|f| format!("`{f}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
                let mut tip = request.upstream;
                let mut tree = work.tree_at(tip);
                for commit in todo {
                    let base = commit
                        .parents
                        .first()
                        .map(|p| work.tree_at(*p))
                        .unwrap_or_default();
                    let mine = work.tree_at(commit.id);
                    let merged = merge_trees(&base, &tree, &mine).map_err(|files| {
                        Self::pull_conflict(request, PullOperation::Rebase, files)
                    })?;
                    // Already in the upstream: git skips it.
                    if merged == tree {
                        continue;
                    }
                    tip = work.make_commit(
                        vec![tip],
                        &merged,
                        Some(commit.author.clone()),
                        commit.summary.clone(),
                        commit.body.clone(),
                    )?;
                    tree = merged;
                }
                tip
            }
        };
        if request.current {
            let new_tree = work.tree_at(target);
            let (differing, files) = work.switch_plan(&new_tree);
            if !files.is_empty() {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-pull-overwrite",
                    files = files
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            let index = work.switch_index(&differing, &new_tree);
            work.set_ref(full, target, None);
            work.rebase_entries(&index);
        } else {
            work.set_ref(full, target, None);
        }
        let backup = Self::pull_backup(&mut work, request.expected);
        *self = work;
        Ok(PullOutcome {
            after: target,
            backup,
            in_progress: false,
            conflicts: Vec::new(),
            message: String::new(),
            warnings: Vec::new(),
        })
    }

    /// The backup `refs/project/backup/pull-<n>` of the branch as it was.
    fn pull_backup(work: &mut Self, expected: Oid) -> String {
        let prefix = format!("{BACKUP_PREFIX}pull-");
        let n = work
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix))
            .count()
            + 1;
        let backup = format!("{prefix}{n}");
        work.commit_backups.push((backup.clone(), expected));
        backup
    }

    /// [`GitBackend::reset_branch`]: undo and redo of a pull of the active branch.
    pub(super) fn reset_keep(
        &mut self,
        request: &RefRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        request.check(self.operation)?;
        let (moved, target) = match (request.refs.as_slice(), &request.head_expected) {
            ([r], crate::HeadState::Branch(b)) if r.name == *b && r.target.is_some() => {
                (r.name.clone(), r.target.expect("controllato"))
            }
            _ => {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-pull-bad-request",
                    command = request.command
                )));
            }
        };
        let head = self.head_state();
        let conflicts =
            request.conflicts(&head, |name| Ok(self.refs.get(name).map(|r| r.target)))?;
        if !conflicts.is_empty() {
            return Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            });
        }
        let new_tree = self.tree_at(target);
        let (differing, files) = self.switch_plan(&new_tree);
        if !files.is_empty() {
            return Err(EngineError::NotAllowed(msg!(
                "engine-pull-undo-overwrite",
                command = request.command,
                moved = moved,
                target = short(&target),
                files = files.join(", ")
            )));
        }
        let mut work = self.clone();
        let index = work.switch_index(&differing, &new_tree);
        let backups = work.restore(request)?;
        work.rebase_entries(&index);
        *self = work;
        Ok(RestoreOutcome {
            backups,
            warnings: Vec::new(),
        })
    }

    pub(super) fn push_branch(
        &mut self,
        request: &PushRequest,
        job: &JobContext,
    ) -> Result<PushOutcome, EngineError> {
        let remote = self.remotes.get(&request.remote).ok_or_else(|| {
            EngineError::NotAllowed(msg!("engine-remote-missing", remote = request.remote))
        })?;
        match &remote.failure {
            Some(FetchFailure::Auth(message)) => {
                return Err(EngineError::Auth {
                    remote: request.remote.clone(),
                    message: message.clone(),
                });
            }
            Some(FetchFailure::Unreachable(message)) => {
                return Err(EngineError::RemoteUnreachable {
                    remote: request.remote.clone(),
                    message: message.clone(),
                });
            }
            None => {}
        }
        let reject = |reason, message: &str| EngineError::PushRejected {
            remote: request.remote.clone(),
            branch: request.remote_branch.clone(),
            reason,
            message: message.to_owned(),
        };
        if let Some(message) = &self.pre_push_failure {
            return Err(reject(PushRejection::Hook, message));
        }
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "remote.push",
            });
        }
        let local = self
            .refs
            .get(&format!("{HEADS}{}", request.branch))
            .map(|r| r.target)
            .ok_or_else(|| {
                EngineError::NotAllowed(msg!("engine-pull-no-branch", branch = request.branch))
            })?;
        let theirs = remote.branches.get(&request.remote_branch).copied();
        let url = remote.url.clone();
        let mine = self.reachable(local);
        let kind = match (request.lease, theirs) {
            (_, Some(t)) if t == local => PushKind::UpToDate,
            (Some(expected), t) if t != expected => {
                return Err(reject(PushRejection::StaleLease, "[rejected] (stale info)"));
            }
            (Some(_), Some(t)) if !mine.contains(&t) => PushKind::Forced,
            (None, Some(t)) if !mine.contains(&t) => {
                let why = if self.commits.contains_key(&t) {
                    "[rejected] (non-fast-forward)"
                } else {
                    "[rejected] (fetch first)"
                };
                return Err(reject(PushRejection::NonFastForward, why));
            }
            (_, Some(_)) => PushKind::FastForward,
            (_, None) => PushKind::New,
        };
        let backup = request.backup.map(|oid| {
            let prefix = format!("{BACKUP_PREFIX}push-");
            let n = self
                .commit_backups
                .iter()
                .filter(|(b, _)| b.starts_with(&prefix))
                .count()
                + 1;
            let name = format!("{prefix}{n}");
            self.commit_backups.push((name.clone(), oid));
            name
        });
        job.report(&Progress {
            phase: "Writing objects".to_owned(),
            remote: false,
            percent: Some(100),
            current: Some(1),
            total: Some(1),
            done: true,
            text: "Writing objects: 100% (1/1), done.".to_owned(),
        });
        let commits: Vec<Commit> = mine
            .iter()
            .filter_map(|id| self.commits.get(id).cloned())
            .collect();
        let remote = self
            .remotes
            .get_mut(&request.remote)
            .expect("controllato sopra");
        for commit in commits {
            remote.commits.insert(commit.id, commit);
        }
        remote.branches.insert(request.remote_branch.clone(), local);
        let tracking = format!("{REMOTES}{}/{}", request.remote, request.remote_branch);
        if kind != PushKind::UpToDate {
            self.refs.insert(
                tracking.clone(),
                RefEntry {
                    target: local,
                    tag_object: None,
                },
            );
        }
        if request.set_upstream {
            self.upstreams
                .insert(format!("{HEADS}{}", request.branch), tracking);
        }
        let summary = match (kind, theirs) {
            (PushKind::New, _) => "[new branch]".to_owned(),
            (PushKind::UpToDate, _) => "[up to date]".to_owned(),
            (PushKind::Forced, Some(t)) => {
                format!("{}...{} (forced update)", short(&t), short(&local))
            }
            (_, Some(t)) => format!("{}..{}", short(&t), short(&local)),
            (_, None) => String::new(),
        };
        let mut messages = vec![format!("To {url}")];
        if request.set_upstream {
            messages.push(format!(
                "branch '{}' set up to track '{}/{}'.",
                request.branch, request.remote, request.remote_branch
            ));
        }
        Ok(PushOutcome {
            kind,
            summary,
            messages,
            backup,
        })
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
