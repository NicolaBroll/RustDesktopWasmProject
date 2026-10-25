//! `repo.clone` and `repo.init` on the [`MemoryOpener`](super::MemoryOpener): the new repo built
//! from the registered remote (`impl-docs/git/clone.md`).

use super::{HEADS, HeadRef, MemoryBackend, MemoryRemote, REMOTES, RefEntry, TAGS};
use crate::EngineError;
use crate::git::clone_no_such_branch;

impl MemoryBackend {
    /// The repo `git clone <url> [--branch <branch>]` of this one gives: its commits and tags,
    /// `origin/<b>` for each of its branches, the remote `origin` (with the same branches and
    /// commits) and the local branch `branch` (or the one of HEAD) tracking `origin/<branch>`.
    /// A clean worktree and no stashes, backups or operation in progress.
    pub(super) fn cloned(&self, url: &str, branch: Option<&str>) -> Result<Self, EngineError> {
        let mut clone = MemoryBackend::new();
        clone.commits = self.commits.clone();
        clone.changes = self.changes.clone();
        clone.blobs = self.blobs.clone();
        clone.tag_objects = self.tag_objects.clone();
        let mut remote = MemoryRemote {
            url: url.to_owned(),
            ..MemoryRemote::default()
        };
        remote.commits = self.commits.clone();
        for (name, entry) in &self.refs {
            if let Some(short) = name.strip_prefix(HEADS) {
                remote.branches.insert(short.to_owned(), entry.target);
                clone.refs.insert(
                    format!("{REMOTES}origin/{short}"),
                    RefEntry {
                        target: entry.target,
                        tag_object: None,
                    },
                );
            } else if name.starts_with(TAGS) {
                clone.refs.insert(name.clone(), entry.clone());
            }
        }
        let head_branch = match &self.head {
            HeadRef::Branch(name) => Some(name.as_str()),
            HeadRef::Detached(_) => None,
        };
        let checkout = match branch {
            Some(branch) if !remote.branches.contains_key(branch) => {
                return Err(clone_no_such_branch(
                    url,
                    branch,
                    &format!("Remote branch {branch} not found in upstream origin"),
                ));
            }
            Some(branch) => Some(branch),
            None => head_branch,
        };
        match checkout {
            Some(name) => {
                if let Some(target) = remote.branches.get(name) {
                    clone.refs.insert(
                        format!("{HEADS}{name}"),
                        RefEntry {
                            target: *target,
                            tag_object: None,
                        },
                    );
                    clone
                        .upstreams
                        .insert(format!("{HEADS}{name}"), format!("{REMOTES}origin/{name}"));
                }
                clone.head = HeadRef::Branch(name.to_owned());
            }
            // A remote with a detached HEAD: the clone is detached on the same commit.
            None => {
                if let HeadRef::Detached(id) = self.head {
                    clone.head = HeadRef::Detached(id);
                }
            }
        }
        clone.remotes.insert("origin".to_owned(), remote);
        Ok(clone)
    }

    /// The repo of `git init [--initial-branch <branch>]`: empty, HEAD on `branch` (`main`
    /// without it, the default of [`MemoryBackend::new`]).
    pub(super) fn initialized(branch: Option<&str>) -> Self {
        let mut repo = MemoryBackend::new();
        if let Some(branch) = branch {
            repo.head = HeadRef::Branch(branch.to_owned());
        }
        repo
    }
}
