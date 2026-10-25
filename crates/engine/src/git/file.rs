//! Blame and history of one file (`file.blame`, `file.log`, S34): queries and errors shared
//! by the backends (`impl-docs/git/reading.md`, "Blame and file history").

use crate::EngineError;
use project_domain::{Blame, Oid, RepoPath};
use serde::{Deserialize, Serialize};

/// What to blame: the file `path` at commit `rev`, or in the worktree if `rev` is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameQuery {
    pub path: RepoPath,
    /// A ref, `HEAD` or an id (also abbreviated); `None` = the file in the worktree, with the
    /// lines not yet committed.
    pub rev: Option<String>,
}

/// The result of [`GitBackend::blame`](super::GitBackend::blame): the [`Blame`] with the
/// requested path and the commit `rev` resolved to (`None` for the worktree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBlame {
    #[serde(flatten)]
    pub blame: Blame,
    pub path: RepoPath,
    pub rev: Option<Oid>,
}

/// A page of the history of one file ([`GitBackend::file_log`](super::GitBackend::file_log)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileLogQuery {
    pub path: RepoPath,
    /// Where to start; `None` = HEAD (a HEAD without commits gives an empty history).
    pub rev: Option<String>,
    /// Commits to skip from the start of the history.
    pub skip: usize,
    /// Maximum number of commits; `None` = all.
    pub limit: Option<usize>,
    /// Across renames (`git log --follow`).
    pub follow: bool,
}

/// `file.blame` of a path that is not a file at `rev` (missing, a folder or a submodule).
pub fn blame_missing_at(path: &RepoPath, rev: &Oid) -> EngineError {
    EngineError::InvalidParams {
        id: "file.blame",
        reason: msg!("engine-blame-missing-rev", path = path, rev = rev),
    }
}

/// `file.blame` without `rev` of a path that is not a tracked file of the worktree (missing,
/// a folder, untracked).
pub fn blame_missing_worktree(path: &RepoPath) -> EngineError {
    EngineError::InvalidParams {
        id: "file.blame",
        reason: msg!("engine-blame-missing-worktree", path = path),
    }
}

/// `file.blame` without `rev` on a branch without commits.
pub fn blame_no_commits() -> EngineError {
    EngineError::InvalidParams {
        id: "file.blame",
        reason: msg!("engine-blame-no-commits"),
    }
}

/// `file.blame` of a binary file.
pub fn blame_binary(path: &RepoPath) -> EngineError {
    EngineError::NotAllowed(msg!("engine-blame-binary", path = path))
}

/// True if `content` is binary for git: a NUL in the first 8000 bytes.
#[cfg(any(test, feature = "test-support"))]
pub fn is_binary_content(content: &[u8]) -> bool {
    content.iter().take(8000).any(|&b| b == 0)
}

/// True if the file the blame describes is binary (`is_binary_content` on its lines).
pub fn blame_is_binary(blame: &Blame) -> bool {
    let mut seen = 0usize;
    for line in &blame.lines {
        if seen >= 8000 {
            break;
        }
        let text = line.text.as_bytes();
        if text.iter().take(8000 - seen).any(|&b| b == 0) {
            return true;
        }
        seen += text.len() + 1;
    }
    false
}
