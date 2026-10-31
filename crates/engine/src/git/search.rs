//! Search in the history (`repo.search`, S48; content S61): the query shared by the backends
//! and the literal matching rules (`impl-docs/git/reading.md`, `repo.search`).

use super::LogQuery;
use project_domain::{Oid, RepoPath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Where [`SearchQuery::text`] is looked for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum SearchField {
    /// The whole message (subject and body), like `git log --grep`.
    Message,
    /// Author name or email, like `git log --author`.
    Author,
    /// Prefix of the full commit id (only when the text is hexadecimal).
    Id,
    /// What the commit changed, like `git log -S`/`-G` (S61): only alone, never in
    /// [`SearchField::ALL`].
    Content,
}

impl SearchField {
    /// All the fields: the default of `repo.search`.
    pub const ALL: [SearchField; 3] = [Self::Message, Self::Author, Self::Id];
}

/// What [`GitRead::search`](super::GitRead::search) looks for: `text` taken literally
/// (never a regex), case-insensitive for ASCII letters, in any of `fields` (OR), optionally only
/// in the commits that touch `path`, over the same commits as `repo.log` with `all`/`stashes`.
///
/// With `fields == [Content]` (S61) instead: the commits (merges excluded, as `git log`) whose
/// changes, file by file against the first parent and without rename detection, change the
/// number of occurrences of `text` (`git log -S`, case-sensitive unless `ignore_case`) or, with
/// `regex`, have an added or removed line matching `text` as a POSIX extended regex (`git log
/// -G`, binary files skipped); with `path`, only the changes under it.
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
    /// [`SearchField::Content`] only: `text` is a regex (`-G`) instead of a literal (`-S`).
    pub regex: bool,
    /// [`SearchField::Content`] only: ASCII case-insensitive (`--regexp-ignore-case`).
    pub ignore_case: bool,
    /// Stop after this many matches (the first ones in the order of the result); `None`: all.
    /// Used for content, where counting them all would mean diffing the whole history.
    pub max: Option<usize>,
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

    /// True if this is a content search ([`SearchField::Content`], always alone).
    pub fn is_content(&self) -> bool {
        self.has(SearchField::Content)
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
