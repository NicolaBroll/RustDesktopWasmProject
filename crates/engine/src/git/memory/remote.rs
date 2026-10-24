//! Remotes on the [`MemoryBackend`] (`remote.fetch`, S25).
//!
//! An in-memory remote has a name, a URL (descriptive only), its branches and the commits it has
//! beyond the repo. The fetch copies into the repo the commits reachable from the downloaded branches
//! and updates `refs/remotes/<remote>/<branch>`; like `git fetch` without `--prune`, a branch
//! that disappeared from the remote stays. An error prepared with [`MemoryBackend::fail_fetch`] simulates
//! rejected credentials or an unreachable remote. The progress is fake but with the same
//! shape as git's (a `Receiving objects` phase from 0 to 100%).

use super::{MemoryBackend, REMOTES, RefEntry};
use crate::git::{FetchOutcome, FetchRequest, mask_url, short};
use crate::{EngineError, JobContext, Progress};
use project_domain::{Commit, Oid};
use std::collections::BTreeMap;

/// Error that the next fetch from an in-memory remote returns
/// ([`MemoryBackend::fail_fetch`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchFailure {
    /// [`EngineError::Auth`] with this message.
    Auth(String),
    /// [`EngineError::RemoteUnreachable`] with this message.
    Unreachable(String),
}

/// A remote of the [`MemoryBackend`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MemoryRemote {
    pub(super) url: String,
    /// Short name of the branch on the remote → commit.
    pub(super) branches: BTreeMap<String, Oid>,
    /// The commits of the remote (also those the repo already has).
    pub(super) commits: BTreeMap<Oid, Commit>,
    pub(super) failure: Option<FetchFailure>,
}

impl MemoryBackend {
    /// Adds the remote `name` with `url` (or changes its URL). Without branches until
    /// [`MemoryBackend::set_branch_on_remote`] is called.
    pub fn add_remote(&mut self, name: &str, url: &str) {
        self.remotes.entry(name.to_owned()).or_default().url = url.to_owned();
    }

    /// The URL of the remote `name`, if any.
    pub fn remote_url(&self, name: &str) -> Option<&str> {
        self.remotes.get(name).map(|r| r.url.as_str())
    }

    /// On the remote `remote` (which must exist) the branch `branch` (short name) points to
    /// `target`. The commit must be given with [`MemoryBackend::insert_remote_commit`], if the repo does not
    /// have it already.
    pub fn set_branch_on_remote(&mut self, remote: &str, branch: &str, target: Oid) {
        self.remote_mut(remote)
            .branches
            .insert(branch.to_owned(), target);
    }

    /// The commit of the branch `branch` on the remote `remote` (as the remote sees it, not the local
    /// remote branch), if any.
    pub fn branch_on_remote(&self, remote: &str, branch: &str) -> Option<Oid> {
        self.remotes.get(remote)?.branches.get(branch).copied()
    }

    /// Removes the branch `branch` from the remote `remote`.
    pub fn remove_branch_on_remote(&mut self, remote: &str, branch: &str) {
        self.remote_mut(remote).branches.remove(branch);
    }

    /// A commit that the remote `remote` has (and the repo maybe not): it enters the repo with the fetch
    /// of a branch that reaches it.
    pub fn insert_remote_commit(&mut self, remote: &str, commit: Commit) {
        self.remote_mut(remote).commits.insert(commit.id, commit);
    }

    /// Fetches and pushes from `remote` fail with `failure` (`None`: they succeed again).
    pub fn fail_fetch(&mut self, remote: &str, failure: Option<FetchFailure>) {
        self.remote_mut(remote).failure = failure;
    }

    fn remote_mut(&mut self, name: &str) -> &mut MemoryRemote {
        self.remotes
            .get_mut(name)
            .unwrap_or_else(|| panic!("remote `{name}` missing: add it with `add_remote`"))
    }

    pub(super) fn remote_names(&self) -> Vec<String> {
        self.remotes.keys().cloned().collect()
    }

    pub(super) fn fetch_remote(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
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
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "remote.fetch",
            });
        }
        let branches: Vec<(String, Oid)> = match &request.branch {
            Some(branch) => match remote.branches.get(branch) {
                Some(target) => vec![(branch.clone(), *target)],
                None => {
                    return Err(EngineError::NotAllowed(msg!(
                        "engine-remote-no-branch",
                        remote = request.remote,
                        branch = branch
                    )));
                }
            },
            None => remote
                .branches
                .iter()
                .map(|(b, t)| (b.clone(), *t))
                .collect(),
        };

        // Commits reachable from the downloaded branches that the repo does not have yet.
        let mut new = Vec::new();
        let mut stack: Vec<Oid> = branches.iter().map(|(_, t)| *t).collect();
        while let Some(id) = stack.pop() {
            if self.commits.contains_key(&id) || new.iter().any(|c: &Commit| c.id == id) {
                continue;
            }
            if let Some(commit) = remote.commits.get(&id) {
                stack.extend(commit.parents.iter().copied());
                new.push(commit.clone());
            }
        }
        let total = new.len() as u64;
        for current in 0..=total {
            let percent = (current * 100).checked_div(total).unwrap_or(100);
            let done = current == total;
            let mut text = format!("Receiving objects: {percent:>3}% ({current}/{total})");
            if done {
                text.push_str(", done.");
            }
            job.report(&Progress {
                phase: "Receiving objects".to_owned(),
                remote: false,
                percent: Some(u8::try_from(percent).unwrap_or(100)),
                current: Some(current),
                total: Some(total),
                done,
                text,
            });
        }

        let url = remote.url.clone();
        let remote_name = request.remote.clone();
        let mut messages = vec![format!("From {}", mask_url(&url).0)];
        for commit in new {
            self.commits.insert(commit.id, commit);
        }
        for (branch, target) in branches {
            let full = format!("{REMOTES}{remote_name}/{branch}");
            let line = match self.refs.get(&full) {
                None => Some(format!(
                    " * [new branch]      {branch} -> {remote_name}/{branch}"
                )),
                Some(entry) if entry.target != target => Some(format!(
                    "   {}..{}  {branch} -> {remote_name}/{branch}",
                    short(&entry.target),
                    short(&target)
                )),
                Some(_) => None,
            };
            messages.extend(line);
            self.refs.insert(
                full,
                RefEntry {
                    target,
                    tag_object: None,
                },
            );
        }
        if messages.len() == 1 {
            // Nothing new: git does not even write `From`.
            messages.clear();
        }
        Ok(FetchOutcome { messages })
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
