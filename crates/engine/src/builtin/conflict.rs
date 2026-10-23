//! Editor dei conflitti (S30): `conflict.show` (lettura) e `conflict.resolve` (scrittura).
//! Tipi e controlli comuni ai backend in `git/conflict.rs`; contratto in
//! `impl-docs/git/conflitti.md`.
//!
//! `conflict.resolve` non sposta ref: niente diario, come lo stage. Il recupero è il backup del
//! worktree (`git show <backup>:<path>`) o `git checkout -m <path>` finché il merge è in corso.

use super::{Out, check_path, to_json};
use crate::commands::parse_params;
use crate::{ConflictResolution, EngineError, Session, VersionText, worktree_digest};
use project_domain::RepoPath;
use serde::Deserialize;
use serde_json::{Value, json};

/// Parametri di `conflict.show`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShowParams {
    path: RepoPath,
}

/// Parametri di `conflict.resolve`.
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

/// `{"path", "kind", "base", "ours", "theirs", "worktree", "marker_size", "digest"}`: le
/// versioni del file in conflitto (vedi
/// [`GitBackend::conflict_versions`](crate::GitBackend::conflict_versions)), ciascuna
/// `{"text", "binary", "size"}` o `null`, e il digest del worktree da ripassare a
/// `conflict.resolve`.
pub(super) fn conflict_show(s: &mut Session, p: Value) -> Out {
    let ShowParams { path } = parse_params("conflict.show", p)?;
    let path = check_path("conflict.show", path)?;
    let v = s.repo()?.conflict_versions(&path)?;
    let version = |b: &Option<Vec<u8>>| to_json(b.as_deref().map(VersionText::of));
    Ok(json!({
        "path": to_json(&v.path),
        "kind": to_json(v.kind),
        "base": version(&v.base),
        "ours": version(&v.ours),
        "theirs": version(&v.theirs),
        "worktree": version(&v.worktree),
        "marker_size": v.marker_size,
        "digest": worktree_digest(v.worktree.as_deref()),
    }))
}

/// Risolve il conflitto di `path` con `content` o `delete` (vedi
/// [`GitBackend::resolve_conflict`](crate::GitBackend::resolve_conflict)). Risultato:
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
                reason: "indicare esattamente uno fra `content` (il testo risolto) e \
                         `\"delete\": true` (il file va cancellato)"
                    .to_owned(),
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
        "status": to_json(repo.status()?),
        "backup": resolved.backup,
        "index_backup": resolved.index_backup,
    }))
}

#[cfg(test)]
#[path = "conflict_tests.rs"]
mod tests;
