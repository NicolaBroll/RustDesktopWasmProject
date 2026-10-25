//! Conflict editor (S30): `conflict.show` (reading) and `conflict.resolve` (writing).
//! Types and checks common to the backends in `git/conflict.rs`; contract in
//! `impl-docs/git/conflicts.md`.
//!
//! `conflict.resolve` does not move refs: no journal, like stage. Recovery is the backup of the
//! worktree (`git show <backup>:<path>`) or `git checkout -m <path>` while the merge is in progress.

use super::{Out, check_path, to_json};
use crate::commands::parse_params;
use crate::{ConflictResolution, EngineError, Session, VersionText, worktree_digest};
use project_domain::RepoPath;
use serde::Deserialize;
use serde_json::{Value, json};

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

/// `{"path", "kind", "base", "ours", "theirs", "worktree", "marker_size", "digest"}`: the
/// versions of the conflicted file (see
/// [`GitBackend::conflict_versions`](crate::GitBackend::conflict_versions)), each
/// `{"text", "binary", "size"}` or `null`, and the worktree digest to pass again to
/// `conflict.resolve`.
pub(super) fn conflict_show(s: &mut Session, p: Value) -> Out {
    let ShowParams { path } = parse_params("conflict.show", p)?;
    let path = check_path("conflict.show", path)?;
    let v = s.repo()?.conflict_versions(&path)?;
    let version = |b: &Option<Vec<u8>>| to_json(b.as_deref().map(VersionText::of));
    Ok(json!({
        "path": v.path,
        "kind": v.kind,
        "base": version(&v.base),
        "ours": version(&v.ours),
        "theirs": version(&v.theirs),
        "worktree": version(&v.worktree),
        "marker_size": v.marker_size,
        "digest": worktree_digest(v.worktree.as_deref()),
    }))
}

/// Resolves the conflict of `path` with `content` or `delete` (see
/// [`GitBackend::resolve_conflict`](crate::GitBackend::resolve_conflict)). Result:
/// `{"status", "backup", "index_backup"}`.
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
    Ok(json!({
        "status": repo.status()?,
        "backup": resolved.backup,
        "index_backup": resolved.index_backup,
    }))
}

#[cfg(test)]
#[path = "conflict_tests.rs"]
mod tests;
