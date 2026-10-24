//! Blame and history of one file (S34): `file.blame`, `file.log`. Reads, slow on long
//! histories (§6): cancellation arrives from [`Session::job`].

use super::{DEFAULT_LOG_LIMIT, Out, check_path, to_json};
use crate::commands::parse_params;
use crate::{BlameQuery, FileLogQuery, Session};
use project_domain::RepoPath;
use serde::Deserialize;
use serde_json::{Value, json};

/// Parameters of `file.blame`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlameParams {
    path: RepoPath,
    #[serde(default)]
    rev: Option<String>,
}

/// Who last changed each line of `path`: in the worktree (lines not yet committed have
/// `commit: null`) or at commit `rev`.
///
/// Result: the [`Blame`](project_domain::Blame) (`{"lines", "commits"}`) plus `{"path", "rev":
/// oid | null}`.
pub(super) fn file_blame(s: &mut Session, p: Value) -> Out {
    const ID: &str = "file.blame";
    let BlameParams { path, rev } = parse_params(ID, p)?;
    let path = check_path(ID, path)?;
    let job = s.job().clone();
    let query = BlameQuery { path, rev };
    Ok(to_json(s.repo()?.blame(&query, &job)?))
}

fn default_follow() -> bool {
    true
}

/// Parameters of `file.log`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileLogParams {
    path: RepoPath,
    #[serde(default)]
    rev: Option<String>,
    #[serde(default)]
    skip: usize,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default = "default_follow")]
    follow: bool,
}

/// The commits that change `path`, newest first (paginated like `repo.log`, `limit` 200).
///
/// Result: `{"commits": [{"commit", "path", "old_path", "status"}], "has_more"}`.
pub(super) fn file_log(s: &mut Session, p: Value) -> Out {
    const ID: &str = "file.log";
    let FileLogParams {
        path,
        rev,
        skip,
        limit,
        follow,
    } = parse_params(ID, p)?;
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
    Ok(json!({ "commits": to_json(commits), "has_more": has_more }))
}

#[cfg(test)]
#[path = "file_tests.rs"]
mod tests;
