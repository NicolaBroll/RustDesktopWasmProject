//! Push (`remote.push`, S26): types shared by the backends (`impl-docs/git/sync.md`).
//!
//! The command decides remote, destination branch and upstream; the backend publishes **only**
//! `refs/heads/<branch>:refs/heads/<remote_branch>` (explicit refspec: no tags, no other
//! branches, whatever `push.default`, `remote.<name>.push` or `push.followTags` say). A force is
//! always and only `--force-with-lease=refs/heads/<remote_branch>:<expected oid>`, with the oid read
//! from the local remote branch before the push.

use crate::Message;
use project_domain::Oid;
use serde::{Deserialize, Serialize};

/// What to publish with [`GitRemotes::push`](super::GitRemotes::push). Names already checked by the
/// command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushRequest {
    /// The configured remote.
    pub remote: String,
    /// The local branch (short name).
    pub branch: String,
    /// The branch on the remote (short name).
    pub remote_branch: String,
    /// Sets the branch's upstream (`--set-upstream`) if the push succeeds: only on the first push,
    /// when the branch has none.
    pub set_upstream: bool,
    /// `Some(expected)`: force with `--force-with-lease=refs/heads/<remote_branch>:<expected>`, that is
    /// the remote is overwritten only if its branch still equals `expected` (`None`: only if it does not
    /// exist). `None`: normal push, fast-forward only.
    pub lease: Option<Option<Oid>>,
    /// The commit to save in a backup ref before a force (`refs/project/backup/push-...`):
    /// the value of the remote branch that the force would overwrite, if it has commits the local
    /// branch does not have.
    pub backup: Option<Oid>,
}

/// How the branch on the remote was updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushKind {
    /// The branch did not exist: created.
    New,
    FastForward,
    /// Overwritten (force-with-lease).
    Forced,
    /// The remote already had that commit: nothing published.
    UpToDate,
}

/// Outcome of [`GitRemotes::push`](super::GitRemotes::push).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushOutcome {
    pub kind: PushKind,
    /// The git summary (`a1b2c3d..e4f5a6b`, `[new branch]`, ...).
    pub summary: String,
    /// The git lines that are not progress (`To <url>`, warnings, hook output).
    pub messages: Vec<String>,
    /// The backup ref created before a force (see [`PushRequest::backup`]).
    pub backup: Option<String>,
}

/// The deletion of a branch on the remote ([`GitRemotes::delete_remote_branch`](super::GitRemotes::delete_remote_branch),
/// `remote.delete_branch`). Names already checked by the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteBranchDelete {
    /// The configured remote.
    pub remote: String,
    /// The branch on the remote (short name): only `refs/heads/<branch>` is deleted.
    pub branch: String,
    /// The value of the local remote-tracking branch confirmed by the user: the deletion is
    /// `--force-with-lease=refs/heads/<branch>:<expected>`, so if the branch moved on the remote
    /// it is rejected ([`PushRejection::StaleLease`]); also saved in the backup ref.
    pub expected: Oid,
}

/// Outcome of [`GitRemotes::delete_remote_branch`](super::GitRemotes::delete_remote_branch).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteBranchDeleted {
    /// The git summary (`[deleted]`).
    pub summary: String,
    /// The git lines that are not progress (`To <url>`, warnings, hook output).
    pub messages: Vec<String>,
    /// The backup ref `refs/project/backup/push-delete-...` on the deleted commit.
    pub backup: String,
    /// Deleted only on some of the push URLs (`remote.<name>.pushurl`), or git failed after
    /// the deletion: the branch may still be on the remote; `messages` has git's lines.
    #[serde(default)]
    pub incomplete: bool,
}

/// The publication of a tag ([`GitRemotes::push_tag`](super::GitRemotes::push_tag),
/// `remote.push_tag`). Names already checked by the command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagPush {
    /// The configured remote.
    pub remote: String,
    /// The tag (short name): only `refs/tags/<name>:refs/tags/<name>` is published, never forced.
    pub name: String,
}

/// Outcome of [`GitRemotes::push_tag`](super::GitRemotes::push_tag).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagPushed {
    /// The remote already had the tag with the same value: nothing published.
    pub up_to_date: bool,
    /// The git summary (`[new tag]`, `[up to date]`).
    pub summary: String,
    /// The git lines that are not progress (`To <url>`, warnings, hook output).
    pub messages: Vec<String>,
}

/// The deletion of a tag on the remote
/// ([`GitRemotes::delete_remote_tag`](super::GitRemotes::delete_remote_tag), `remote.delete_tag`).
/// Names already checked by the command. The outcome is a [`RemoteBranchDeleted`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTagDelete {
    /// The configured remote.
    pub remote: String,
    /// The tag on the remote (short name): only `refs/tags/<name>` is deleted.
    pub name: String,
    /// The value of the tag on the remote (the tag object of an annotated tag) confirmed by the
    /// user: the deletion is `--force-with-lease=refs/tags/<name>:<expected>`; also saved in the
    /// backup ref (fetched first if the object is not here).
    pub expected: Oid,
}

/// Why the push was rejected ([`EngineError::PushRejected`](crate::EngineError)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushRejection {
    /// The remote has commits the local branch does not have (`non-fast-forward`, `fetch first`).
    NonFastForward,
    /// Force-with-lease: the branch on the remote no longer equals the expected one (`stale info`).
    StaleLease,
    /// The remote rejected the update (`pre-receive`/`update` hook, protected branch).
    RemoteRejected,
    /// A local `pre-push` hook (or another local check, such as unpublished
    /// submodules) stopped the push before it started.
    Hook,
    /// A tag already on the remote with another value (`[rejected] (already exists)`): a tag is
    /// never overwritten.
    TagExists,
    /// Other git rejection.
    Other,
}

impl PushRejection {
    /// What to do, for the error message.
    pub fn advice(&self) -> Message {
        match self {
            PushRejection::NonFastForward => msg!("engine-push-non-fast-forward"),
            PushRejection::StaleLease => msg!("engine-push-stale-lease"),
            PushRejection::RemoteRejected => msg!("engine-push-remote-rejected"),
            PushRejection::Hook => msg!("engine-push-hook"),
            PushRejection::TagExists => msg!("engine-push-tag-exists"),
            PushRejection::Other => msg!("engine-push-other"),
        }
    }
}

/// The name of the local remote branch that mirrors `remote_ref` (e.g. `refs/heads/main`)
/// according to the remote's fetch refspecs (`remote.<name>.fetch`, e.g.
/// `+refs/heads/*:refs/remotes/origin/*`), as git does to update it after a push: the first
/// refspec that matches (with at most one `*`), `None` if none, if the refspec has no
/// destination or if a negative refspec (`^...`) excludes it.
pub fn tracking_ref(refspecs: &[String], remote_ref: &str) -> Option<String> {
    fn matches<'a>(pattern: &str, name: &'a str) -> Option<&'a str> {
        match pattern.split_once('*') {
            Some((prefix, suffix)) => name
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(suffix))
                .filter(|middle| !middle.is_empty()),
            None => (pattern == name).then_some(""),
        }
    }
    let excluded = refspecs
        .iter()
        .filter_map(|r| r.strip_prefix('^'))
        .any(|pattern| matches(pattern, remote_ref).is_some());
    if excluded {
        return None;
    }
    refspecs
        .iter()
        .filter(|r| !r.starts_with('^'))
        .find_map(|spec| {
            let spec = spec.strip_prefix('+').unwrap_or(spec);
            let (src, dst) = spec.split_once(':')?;
            if dst.is_empty() {
                return None;
            }
            let middle = matches(src, remote_ref)?;
            Some(if src.contains('*') {
                dst.replacen('*', middle, 1)
            } else {
                dst.to_owned()
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracking_ref_from_refspecs() {
        let specs = |s: &[&str]| s.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        let default = specs(&["+refs/heads/*:refs/remotes/origin/*"]);
        assert_eq!(
            tracking_ref(&default, "refs/heads/feature/x").as_deref(),
            Some("refs/remotes/origin/feature/x")
        );
        assert_eq!(tracking_ref(&default, "refs/tags/v1"), None);
        assert_eq!(tracking_ref(&[], "refs/heads/main"), None);
        let custom = specs(&[
            "^refs/heads/tmp/*",
            "refs/heads/main:refs/remotes/up/principale",
            "+refs/heads/*:refs/remotes/up/b/*",
        ]);
        assert_eq!(
            tracking_ref(&custom, "refs/heads/main").as_deref(),
            Some("refs/remotes/up/principale")
        );
        assert_eq!(
            tracking_ref(&custom, "refs/heads/x").as_deref(),
            Some("refs/remotes/up/b/x")
        );
        assert_eq!(tracking_ref(&custom, "refs/heads/tmp/a"), None);
        assert_eq!(
            tracking_ref(&specs(&["refs/heads/main"]), "refs/heads/main"),
            None
        );
    }
}
