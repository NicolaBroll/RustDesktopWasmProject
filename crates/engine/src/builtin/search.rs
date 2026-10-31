//! `repo.search` (S48): the ids of the commits that match a literal text, in the order of
//! `repo.log`; with `"in": ["content"]` (S61) the commits whose changes contain it
//! (`impl-docs/git/reading.md`, `repo.search`).

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
    /// The text to look for (literal, case-insensitive; for content: case-sensitive unless
    /// `ignore_case`, a regex with `regex`).
    pub query: String,
    /// Where to look (`in`); without: message, author and id. `"content"` only alone.
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
    /// Content only: `query` is a POSIX extended regex matched against the added and removed
    /// lines (`git log -G`) instead of a literal whose count changes (`git log -S`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub regex: bool,
    /// Content only: ignore the case of ASCII letters.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignore_case: bool,
}

/// Result of `repo.search`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoSearchResult {
    /// The page `skip..skip + limit` of the matches, in the order of `repo.log` with the same
    /// `all`/`stashes`.
    pub ids: Vec<Oid>,
    /// All the matches; `None` (`null`) for content, which is not counted (it would mean
    /// diffing the whole history).
    pub total: Option<usize>,
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
        regex,
        ignore_case,
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
    let content = fields.contains(&SearchField::Content);
    if content && fields.len() > 1 {
        return Err(invalid(msg!("engine-search-content-alone")));
    }
    if !content && (regex || ignore_case) {
        return Err(invalid(msg!("engine-search-content-options")));
    }
    if stashes && !all {
        return Err(invalid(msg!("engine-log-stashes-without-all")));
    }
    let path = path.map(|p| check_path(ID, p)).transpose()?;
    let limit = limit.unwrap_or(DEFAULT_LOG_LIMIT);
    let end = skip.saturating_add(limit);
    let query = SearchQuery {
        text: query,
        fields,
        path,
        all,
        stashes,
        regex,
        ignore_case,
        // Content: only up to one past the page, enough for `has_more`.
        max: content.then(|| end.saturating_add(1)),
    };
    let job = s.job().clone();
    let ids = s.repo()?.search(&query, &job)?;
    let found = ids.len();
    let total = (!content).then_some(found);
    let page: Vec<Oid> = ids.into_iter().skip(skip).take(limit).collect();
    let has_more = end < found;
    Ok(to_json(RepoSearchResult {
        ids: page,
        total,
        has_more,
    }))
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
