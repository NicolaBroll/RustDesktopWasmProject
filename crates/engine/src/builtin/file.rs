//! Blame and history of one file (S34): `file.blame`, `file.log`. Reads, slow on long
//! histories (§6): cancellation arrives from [`Session::job`].

use super::{DEFAULT_LOG_LIMIT, Out, check_path, to_json};
use crate::{BlameQuery, FileLogQuery, Session};
use project_domain::{FileLogEntry, RepoPath};
use serde::{Deserialize, Serialize};

/// Result of `file.log`: a page of the history of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLogResult {
    /// The commits that change the file, newest first.
    pub commits: Vec<FileLogEntry>,
    /// After the last returned commit there are others (as in `repo.log`).
    pub has_more: bool,
}

/// Parameters of `file.blame`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileBlameParams {
    /// The file.
    pub path: RepoPath,
    /// The commit (rev); without: the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

/// Who last changed each line of `path`: in the worktree (lines not yet committed have
/// `commit: null`) or at commit `rev`.
///
/// Result: the [`Blame`](project_domain::Blame) (`{"lines", "commits"}`) plus `{"path", "rev":
/// oid | null}`.
pub(super) fn file_blame(s: &mut Session, FileBlameParams { path, rev }: FileBlameParams) -> Out {
    const ID: &str = "file.blame";
    let path = check_path(ID, path)?;
    let job = s.job().clone();
    let query = BlameQuery { path, rev };
    Ok(to_json(s.repo()?.blame(&query, &job)?))
}

fn default_follow() -> bool {
    true
}

/// Parameters of `file.log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileLogParams {
    /// The file.
    pub path: RepoPath,
    /// Where to start (rev); without: HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Commits to skip from the start.
    #[serde(default)]
    pub skip: usize,
    /// Maximum number of commits; without: 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Follow the file across renames (default `true`).
    #[serde(default = "default_follow")]
    pub follow: bool,
}

/// The commits that change `path`, newest first (paginated like `repo.log`, `limit` 200).
///
/// Result: [`FileLogResult`].
pub(super) fn file_log(
    s: &mut Session,
    FileLogParams {
        path,
        rev,
        skip,
        limit,
        follow,
    }: FileLogParams,
) -> Out {
    const ID: &str = "file.log";
    let path = check_path(ID, path)?;
    let limit = limit.unwrap_or(DEFAULT_LOG_LIMIT);
    let job = s.job().clone();
    // One more commit to know whether the page is the last.
    let query = FileLogQuery {
        path,
        rev,
        skip,
        limit: Some(limit.saturating_add(1)),
        follow,
    };
    let mut commits = s.repo()?.file_log(&query, &job)?;
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    Ok(to_json(FileLogResult { commits, has_more }))
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
