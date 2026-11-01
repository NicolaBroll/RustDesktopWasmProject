//! `repo.search` on the [`MemoryBackend`]: the same rules as the `CliBackend`, in memory.
//! Content (S61): a scan of each commit against its first parent, file by file: `-S` compares
//! the number of occurrences, `-G` looks at the added and removed lines of the memory line diff
//! (the `regex` crate without Unicode, close to git's POSIX extended regex for simple patterns).

use super::{MemoryBackend, line_diff};
use crate::git::{GitRead, SearchField, SearchQuery, is_binary_content, path_matches};
use crate::{EngineError, JobContext};
use project_domain::{Commit, LineKind, Oid, RepoPath};
use regex::bytes::{Regex, RegexBuilder};
use std::collections::{BTreeMap, BTreeSet};

const COMMAND: &str = "repo.search";

impl MemoryBackend {
    pub(super) fn memory_search(
        &self,
        query: &SearchQuery,
        job: &JobContext,
    ) -> Result<Vec<Oid>, EngineError> {
        if job.is_cancelled() {
            return Err(EngineError::Cancelled { command: COMMAND });
        }
        if query.is_content() {
            return self.content_search(query, job);
        }
        let text = query.text.as_bytes();
        let matches = |c: &Commit| {
            let message = format!("{}\n\n{}", c.summary, c.body);
            (query.has(SearchField::Message) && contains_literal(message.as_bytes(), text))
                || (query.has(SearchField::Author)
                    && (contains_literal(c.author.name.as_bytes(), text)
                        || contains_literal(c.author.email.as_bytes(), text)))
                || query.id_matches(&c.id)
        };
        Ok(self
            .log(&query.range())?
            .into_iter()
            .filter(|c| matches(c))
            .filter(|c| query.path.as_ref().is_none_or(|p| self.touches(c, p)))
            .map(|c| c.id)
            .collect())
    }

    /// The non-merge commits whose diff against the first parent (under `query.path`) changes
    /// the count of the text (`-S`) or has an added/removed line matching it (`-G`).
    fn content_search(
        &self,
        query: &SearchQuery,
        job: &JobContext,
    ) -> Result<Vec<Oid>, EngineError> {
        let regex = query
            .regex
            .then(|| {
                RegexBuilder::new(&query.text)
                    .unicode(false)
                    .case_insensitive(query.ignore_case)
                    .build()
                    .map_err(|e| EngineError::InvalidParams {
                        id: COMMAND,
                        reason: msg!("engine-search-bad-regex", stderr = e.to_string()),
                    })
            })
            .transpose()?;
        let max = query.max.unwrap_or(usize::MAX);
        let mut out = Vec::new();
        for commit in self.log(&query.range())? {
            if out.len() >= max {
                break;
            }
            if job.is_cancelled() {
                return Err(EngineError::Cancelled { command: COMMAND });
            }
            if commit.parents.len() > 1 {
                continue;
            }
            let old = commit
                .parents
                .first()
                .map(|p| self.tree_at(*p))
                .unwrap_or_default();
            let new = self.tree_at(commit.id);
            let paths: BTreeSet<&RepoPath> = old
                .keys()
                .chain(new.keys())
                .filter(|p| query.path.as_ref().is_none_or(|q| path_matches(p, q)))
                .collect();
            let content = |tree: &BTreeMap<RepoPath, (u32, Oid)>, p: &RepoPath| {
                tree.get(p)
                    .map(|(_, id)| *id)
                    .and_then(|id| self.blob(id))
                    .unwrap_or_default()
            };
            let changed = paths.into_iter().any(|p| {
                let (a, b) = (content(&old, p), content(&new, p));
                match &regex {
                    None => {
                        let text = query.text.as_bytes();
                        occurrences(a, text, query.ignore_case)
                            != occurrences(b, text, query.ignore_case)
                    }
                    Some(re) => diff_line_matches(a, b, re),
                }
            });
            if changed {
                out.push(commit.id);
            }
        }
        Ok(out)
    }

    /// True if the commit changes something at `path` (or under it) with respect to at least
    /// one parent (a root commit: if it has it), like `git log --full-history -- <path>`: a merge
    /// that brings in a change from the other side counts.
    fn touches(&self, commit: &Commit, path: &RepoPath) -> bool {
        let under = |id: Oid| -> BTreeMap<_, _> {
            self.tree_at(id)
                .into_iter()
                .filter(|(p, _)| path_matches(p, path))
                .collect()
        };
        let here = under(commit.id);
        if commit.parents.is_empty() {
            return !here.is_empty();
        }
        commit.parents.iter().any(|p| under(*p) != here)
    }
}

/// True if `needle` appears in `haystack` as is, ignoring the case of ASCII letters (what
/// `git log --fixed-strings --regexp-ignore-case` does with `LC_ALL=C`).
fn contains_literal(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty()
        || haystack
            .windows(needle.len())
            .any(|w| w.eq_ignore_ascii_case(needle))
}

/// Non-overlapping occurrences of `needle` in `haystack`, like git's `-S` count.
fn occurrences(haystack: &[u8], needle: &[u8], ignore_case: bool) -> usize {
    let (mut i, mut count) = (0, 0);
    while i + needle.len() <= haystack.len() {
        let w = &haystack[i..i + needle.len()];
        if (ignore_case && w.eq_ignore_ascii_case(needle)) || w == needle {
            count += 1;
            i += needle.len();
        } else {
            i += 1;
        }
    }
    count
}

/// True if an added or removed line of the diff from `old` to `new` matches `re`, like git's
/// `-G` (binary files never match).
fn diff_line_matches(old: &[u8], new: &[u8], re: &Regex) -> bool {
    if old == new || is_binary_content(old) || is_binary_content(new) {
        return false;
    }
    line_diff::hunks(old, new).iter().any(|h| {
        h.lines
            .iter()
            .any(|l| l.kind != LineKind::Context && re.is_match(l.content.as_bytes()))
    })
}

#[cfg(test)]
mod tests {
    use super::{contains_literal, occurrences};

    #[test]
    fn literal_matching() {
        assert!(contains_literal(b"Fix a.b*[ here", b"A.B*["));
        assert!(!contains_literal(b"Fix aXb here", b"a.b"));
        assert!(contains_literal(b"caf\xe9 needle", b"NEEDLE"));
        assert!(!contains_literal(b"short", b"longer than it"));
    }

    #[test]
    fn occurrence_count() {
        assert_eq!(occurrences(b"aaaa", b"aa", false), 2);
        assert_eq!(occurrences(b"Ab ab AB", b"ab", false), 1);
        assert_eq!(occurrences(b"Ab ab AB", b"ab", true), 3);
        assert_eq!(occurrences(b"", b"ab", true), 0);
    }
}
