//! Conflict editor (S30): `conflict.show` (reading) and `conflict.resolve` (writing).
//! Types and checks common to the backends in `git/conflict.rs`; contract in
//! `impl-docs/git/conflicts.md`.
//!
//! `conflict.resolve` does not move refs: no journal, like stage. Recovery is the backup of the
//! worktree (`git show <backup>:<path>`) or `git checkout -m <path>` while the merge is in progress.

use super::{Out, check_path, to_json};
use crate::commands::parse_params;
use crate::{ConflictResolution, EngineError, Session, VersionText, worktree_digest};
use project_domain::{Conflict, RepoPath, Status};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result of `conflict.show`: the versions of the conflicted file (see
/// [`GitBackend::conflict_versions`](crate::GitBackend::conflict_versions)).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConflictShowResult {
    /// The conflicted file.
    pub path: RepoPath,
    /// The kind of conflict.
    pub kind: Conflict,
    /// The common ancestor's version (stage 1); `null` if missing.
    pub base: Option<VersionText>,
    /// Our version (stage 2); `null` if missing.
    pub ours: Option<VersionText>,
    /// Their version (stage 3); `null` if missing.
    pub theirs: Option<VersionText>,
    /// The worktree file; `null` if missing.
    pub worktree: Option<VersionText>,
    /// The length of the conflict markers (`conflict-marker-size` attribute).
    pub marker_size: usize,
    /// The digest of the worktree file ([`worktree_digest`]), to pass again to
    /// `conflict.resolve`.
    pub digest: String,
}

/// Result of `conflict.resolve`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConflictResolveResult {
    /// The status after the resolution (a status that cannot be re-read is an error of the
    /// command).
    pub status: Status,
    /// The backup ref of the worktree bytes (`refs/project/backup/conflict-...`); `null` if the
    /// file was not there.
    pub backup: Option<String>,
    /// The backup ref of the index from `index.stage`, if there was one.
    pub index_backup: Option<String>,
}

/// Parameters of `conflict.show`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShowParams {
    path: RepoPath,
}

/// Parameters of `conflict.resolve`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveParams {
    path: RepoPath,
    digest: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    delete: bool,
}

/// The versions of the conflicted file `path`, each `{"text", "binary", "size"}` or `null`, and
/// the worktree digest to pass again to `conflict.resolve`. Result: [`ConflictShowResult`].
pub(super) fn conflict_show(s: &mut Session, p: Value) -> Out {
    let ShowParams { path } = parse_params("conflict.show", p)?;
    let path = check_path("conflict.show", path)?;
    let v = s.repo()?.conflict_versions(&path)?;
    let version = |b: &Option<Vec<u8>>| b.as_deref().map(VersionText::of);
    Ok(to_json(ConflictShowResult {
        base: version(&v.base),
        ours: version(&v.ours),
        theirs: version(&v.theirs),
        worktree: version(&v.worktree),
        digest: worktree_digest(v.worktree.as_deref()),
        path: v.path,
        kind: v.kind,
        marker_size: v.marker_size,
    }))
}

/// Resolves the conflict of `path` with `content` or `delete` (see
/// [`GitBackend::resolve_conflict`](crate::GitBackend::resolve_conflict)). Result:
/// [`ConflictResolveResult`].
pub(super) fn conflict_resolve(s: &mut Session, p: Value) -> Out {
    const ID: &str = "conflict.resolve";
    let ResolveParams {
        path,
        digest,
        content,
        delete,
    } = parse_params(ID, p)?;
    let path = check_path(ID, path)?;
    let content = match (content, delete) {
        (Some(c), false) => Some(c.into_bytes()),
        (None, true) => None,
        _ => {
            return Err(EngineError::InvalidParams {
                id: ID,
                reason: msg!("engine-conflict-content-or-delete"),
            });
        }
    };
    let repo = s.repo_mut()?;
    let resolved = repo.resolve_conflict(&ConflictResolution {
        path,
        digest,
        content,
    })?;
    Ok(to_json(ConflictResolveResult {
        status: repo.status()?,
        backup: resolved.backup,
        index_backup: resolved.index_backup,
    }))
}

#[cfg(test)]
#[path = "conflict_tests.rs"]
mod tests;
