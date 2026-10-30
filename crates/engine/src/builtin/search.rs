//! `repo.search` (S48): the ids of the commits that match a literal text, in the order of
//! `repo.log` (`impl-docs/git/reading.md`, `repo.search`).

use super::{DEFAULT_LOG_LIMIT, Out, check_path, to_json};
use crate::{EngineError, SearchField, SearchQuery, Session};
use project_domain::{Oid, RepoPath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const ID: &str = "repo.search";

/// Parameters of `repo.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepoSearchParams {
    /// The text to look for (literal, case-insensitive).
    pub query: String,
    /// Where to look (`in`); without: message, author and id.
    #[serde(default, rename = "in", skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<SearchField>>,
    /// Only commits touching this path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<RepoPath>,
    /// As in `repo.log`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    /// As in `repo.log` (only with `all`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stashes: bool,
    /// Matches to skip from the start.
    #[serde(default)]
    pub skip: usize,
    /// Maximum number of ids; without: 200.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Result of `repo.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSearchResult {
    /// The page `skip..skip + limit` of the matches, in the order of `repo.log` with the same
    /// `all`/`stashes`.
    pub ids: Vec<Oid>,
    /// All the matches.
    pub total: usize,
    /// There are other matches after the page.
    pub has_more: bool,
}

/// The commits that match a text: [`RepoSearchResult`].
pub(super) fn repo_search(
    s: &mut Session,
    RepoSearchParams {
        query,
        fields,
        path,
        all,
        stashes,
        skip,
        limit,
    }: RepoSearchParams,
) -> Out {
    let invalid = |reason| EngineError::InvalidParams { id: ID, reason };
    if query.is_empty() {
        return Err(invalid(msg!("engine-search-empty")));
    }
    // git would split the text at the newline and look for each line (OR).
    if query.contains(['\n', '\0']) {
        return Err(invalid(msg!("engine-search-line")));
    }
    let mut fields = fields.unwrap_or_else(|| SearchField::ALL.to_vec());
    fields.sort();
    fields.dedup();
    if fields.is_empty() {
        return Err(invalid(msg!("engine-search-no-fields")));
    }
    if stashes && !all {
        return Err(invalid(msg!("engine-log-stashes-without-all")));
    }
    let path = path.map(|p| check_path(ID, p)).transpose()?;
    let query = SearchQuery {
        text: query,
        fields,
        path,
        all,
        stashes,
    };
    let job = s.job().clone();
    let ids = s.repo()?.search(&query, &job)?;
    let total = ids.len();
    let limit = limit.unwrap_or(DEFAULT_LOG_LIMIT);
    let page: Vec<Oid> = ids.into_iter().skip(skip).take(limit).collect();
    let has_more = skip.saturating_add(limit) < total;
    Ok(to_json(RepoSearchResult {
        ids: page,
        total,
        has_more,
    }))
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
