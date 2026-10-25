//! `repo.search` (S48): the ids of the commits that match a literal text, in the order of
//! `repo.log` (`impl-docs/git/reading.md`, `repo.search`).

use super::{DEFAULT_LOG_LIMIT, Out, check_path};
use crate::commands::parse_params;
use crate::{EngineError, SearchField, SearchQuery, Session};
use project_domain::RepoPath;
use serde::Deserialize;
use serde_json::{Value, json};

const ID: &str = "repo.search";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchParams {
    query: String,
    #[serde(default, rename = "in")]
    fields: Option<Vec<SearchField>>,
    #[serde(default)]
    path: Option<RepoPath>,
    #[serde(default)]
    all: bool,
    #[serde(default)]
    stashes: bool,
    #[serde(default)]
    skip: usize,
    #[serde(default)]
    limit: Option<usize>,
}

/// `{"ids": [oid], "total": number, "has_more": bool}`: the page `skip..skip + limit` of the
/// matches, in the order of `repo.log` with the same `all`/`stashes`; `total` counts all the
/// matches, `has_more` says whether there are others after the page.
pub(super) fn repo_search(s: &mut Session, p: Value) -> Out {
    let SearchParams {
        query,
        fields,
        path,
        all,
        stashes,
        skip,
        limit,
    } = parse_params(ID, p)?;
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
    let page: Vec<String> = ids
        .iter()
        .skip(skip)
        .take(limit)
        .map(ToString::to_string)
        .collect();
    let has_more = skip.saturating_add(limit) < total;
    Ok(json!({ "ids": page, "total": total, "has_more": has_more }))
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
