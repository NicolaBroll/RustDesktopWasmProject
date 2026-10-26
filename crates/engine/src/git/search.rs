//! Search in the history (`repo.search`, S48): the query shared by the backends and the
//! literal matching rules (`impl-docs/git/reading.md`, `repo.search`).

use super::LogQuery;
use project_domain::{Oid, RepoPath};
use serde::{Deserialize, Serialize};

/// Where [`SearchQuery::text`] is looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchField {
    /// The whole message (subject and body), like `git log --grep`.
    Message,
    /// Author name or email, like `git log --author`.
    Author,
    /// Prefix of the full commit id (only when the text is hexadecimal).
    Id,
}

impl SearchField {
    /// All the fields: the default of `repo.search`.
    pub const ALL: [SearchField; 3] = [Self::Message, Self::Author, Self::Id];
}

/// What [`GitBackend::search`](super::GitBackend::search) looks for: `text` taken literally
/// (never a regex), case-insensitive for ASCII letters, in any of `fields` (OR), optionally only
/// in the commits that touch `path`, over the same commits as `repo.log` with `all`/`stashes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    /// Not empty.
    pub text: String,
    /// Not empty, without duplicates.
    pub fields: Vec<SearchField>,
    /// Only the commits that change this path (a file, or anything under a folder), changed
    /// with respect to at least one parent (`git log --full-history`): no rename following.
    pub path: Option<RepoPath>,
    /// As [`LogQuery::all`].
    pub all: bool,
    /// As [`LogQuery::stashes`] (only with `all`).
    pub stashes: bool,
}

impl SearchQuery {
    /// The whole log the search runs over, in the order of the result.
    pub fn range(&self) -> LogQuery {
        LogQuery {
            all: self.all,
            stashes: self.stashes,
            ..LogQuery::all()
        }
    }

    /// True if `field` is searched.
    pub fn has(&self, field: SearchField) -> bool {
        self.fields.contains(&field)
    }

    /// True if [`SearchField::Id`] is searched and `id` starts with the text (hexadecimal,
    /// any case).
    pub fn id_matches(&self, id: &Oid) -> bool {
        self.has(SearchField::Id) && id_prefix_matches(&self.text, id)
    }
}

/// True if `text` is a non-empty hexadecimal prefix (any case) of the full id.
pub fn id_prefix_matches(text: &str, id: &Oid) -> bool {
    !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_hexdigit())
        && id.to_string().starts_with(&text.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_prefix() {
        let id: Oid = "abcdef0123456789abcdef0123456789abcdef01".parse().unwrap();
        assert!(id_prefix_matches("ABCdef", &id));
        assert!(id_prefix_matches(&id.to_string(), &id));
        assert!(!id_prefix_matches("bcd", &id));
        assert!(!id_prefix_matches("abcg", &id));
        assert!(!id_prefix_matches("", &id));
    }
}
