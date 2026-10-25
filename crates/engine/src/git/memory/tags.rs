//! Deletion of a local tag and tags on the remote on the [`MemoryBackend`] (`tag.delete`,
//! `remote.push_tag`, `remote.delete_tag`, S47), with the same checks as the `CliBackend`: the local
//! deletion only with the expected value and after the backup; the push never overwrites a tag
//! with another value; the remote deletion with the lease on the value confirmed and, if the object is
//! not here, a "fetch" of it into the backup first.

use super::{MemoryBackend, TAGS};
use crate::git::{
    BACKUP_PREFIX, PushRejection, RemoteBranchDeleted, RemoteTagDelete, TagPush, TagPushed,
};
use crate::{EngineError, FetchFailure, JobContext};
use project_domain::{Commit, Oid};

impl MemoryBackend {
    /// The next backup name `refs/project/backup/<kind>-<n>`, saved on `oid`.
    fn save_backup(&mut self, kind: &str, oid: Oid) -> String {
        let prefix = format!("{BACKUP_PREFIX}{kind}-");
        let n = self
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix))
            .count()
            + 1;
        let name = format!("{prefix}{n}");
        self.commit_backups.push((name.clone(), oid));
        name
    }

    /// The error prepared with `fail_fetch` for `remote`, or the remote missing.
    fn remote_failure(&self, remote: &str) -> Result<(), EngineError> {
        let r = self.remotes.get(remote).ok_or_else(|| {
            EngineError::NotAllowed(msg!("engine-remote-missing", remote = remote))
        })?;
        match &r.failure {
            Some(FetchFailure::Auth(message)) => Err(EngineError::Auth {
                remote: remote.to_owned(),
                message: message.clone(),
            }),
            Some(FetchFailure::Unreachable(message)) => Err(EngineError::RemoteUnreachable {
                remote: remote.to_owned(),
                message: message.clone(),
            }),
            None => Ok(()),
        }
    }

    /// [`GitBackend::delete_tag`](crate::GitBackend::delete_tag).
    pub(super) fn delete_local_tag(
        &mut self,
        name: &str,
        expected: &Oid,
    ) -> Result<String, EngineError> {
        let full = format!("{TAGS}{name}");
        match self.refs.get(&full).map(|r| r.value()) {
            Some(now) if now == *expected => {}
            Some(now) => {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-tag-changed",
                    full = full,
                    expected = expected,
                    now = now
                )));
            }
            None => {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-tag-missing",
                    full = full
                )));
            }
        }
        let backup = self.save_backup("tag-delete", *expected);
        self.refs.remove(&full);
        Ok(backup)
    }

    /// [`GitBackend::remote_tag`](crate::GitBackend::remote_tag).
    pub(super) fn tag_value_on_remote(
        &self,
        remote: &str,
        name: &str,
        job: &JobContext,
    ) -> Result<Option<Oid>, EngineError> {
        self.remote_failure(remote)?;
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "remote.delete_tag",
            });
        }
        Ok(self.tag_on_remote(remote, name))
    }

    /// [`GitBackend::push_tag`](crate::GitBackend::push_tag).
    pub(super) fn push_tag_to_remote(
        &mut self,
        request: &TagPush,
        job: &JobContext,
    ) -> Result<TagPushed, EngineError> {
        const ID: &str = "remote.push_tag";
        self.remote_failure(&request.remote)?;
        let full = format!("{TAGS}{}", request.name);
        let reject = |reason, message: &str| EngineError::PushRejected {
            remote: request.remote.clone(),
            branch: full.clone(),
            reason,
            message: message.to_owned(),
        };
        let entry = self.refs.get(&full).cloned().ok_or_else(|| {
            EngineError::NotAllowed(msg!("engine-tag-missing", full = full.as_str()))
        })?;
        let value = entry.value();
        let remote = &self.remotes[&request.remote];
        let url = remote.url.clone();
        match remote.tags.get(&request.name) {
            Some(theirs) if *theirs == value => {
                return Ok(TagPushed {
                    up_to_date: true,
                    summary: "[up to date]".to_owned(),
                    messages: vec![format!("To {url}")],
                });
            }
            Some(_) => {
                return Err(reject(
                    PushRejection::TagExists,
                    "[rejected] (already exists)",
                ));
            }
            None => {}
        }
        if let Some(message) = &self.pre_push_failure {
            return Err(reject(PushRejection::Hook, message));
        }
        if job.is_cancelled() {
            return Err(EngineError::Cancelled { command: ID });
        }
        let commits: Vec<Commit> = self
            .reachable(entry.target)
            .iter()
            .filter_map(|id| self.commits.get(id).cloned())
            .collect();
        let remote = self
            .remotes
            .get_mut(&request.remote)
            .expect("checked above");
        for commit in commits {
            remote.commits.insert(commit.id, commit);
        }
        if let Some(object) = entry.tag_object {
            remote.tag_objects.insert(object, entry.target);
        }
        remote.tags.insert(request.name.clone(), value);
        Ok(TagPushed {
            up_to_date: false,
            summary: "[new tag]".to_owned(),
            messages: vec![format!("To {url}")],
        })
    }

    /// [`GitBackend::delete_remote_tag`](crate::GitBackend::delete_remote_tag).
    pub(super) fn delete_tag_on_remote(
        &mut self,
        request: &RemoteTagDelete,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError> {
        const ID: &str = "remote.delete_tag";
        self.remote_failure(&request.remote)?;
        let reject = |reason, message: &str| EngineError::PushRejected {
            remote: request.remote.clone(),
            branch: format!("{TAGS}{}", request.name),
            reason,
            message: message.to_owned(),
        };
        if job.is_cancelled() {
            return Err(EngineError::Cancelled { command: ID });
        }
        let expected = request.expected;
        let remote = &self.remotes[&request.remote];
        let theirs = remote.tags.get(&request.name).copied();
        let url = remote.url.clone();
        // The object is not here: "fetched" from the remote with the ref's value now.
        let here = self.commits.contains_key(&expected) || self.tag_objects.contains_key(&expected);
        if !here {
            if theirs != Some(expected) {
                return Err(reject(PushRejection::StaleLease, "[rejected] (stale info)"));
            }
            let target = remote.tag_objects.get(&expected).copied();
            let commits: Vec<Commit> = remote.commits.values().cloned().collect();
            for commit in commits {
                self.commits.entry(commit.id).or_insert(commit);
            }
            if let Some(target) = target {
                self.tag_objects.insert(expected, target);
            }
        }
        if let Some(message) = &self.pre_push_failure {
            return Err(reject(PushRejection::Hook, message));
        }
        if theirs != Some(expected) {
            return Err(reject(PushRejection::StaleLease, "[rejected] (stale info)"));
        }
        let backup = self.save_backup("push-delete", expected);
        self.remotes
            .get_mut(&request.remote)
            .expect("checked above")
            .tags
            .remove(&request.name);
        Ok(RemoteBranchDeleted {
            summary: "[deleted]".to_owned(),
            messages: vec![format!("To {url}")],
            backup,
            incomplete: false,
        })
    }
}
