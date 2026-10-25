//! `repo.search` on the [`MemoryBackend`]: the same rules as the `CliBackend`, in memory.

use super::MemoryBackend;
use crate::git::{GitBackend, SearchField, SearchQuery, contains_literal, path_matches};
use crate::{EngineError, JobContext};
use project_domain::{Commit, Oid, RepoPath};
use std::collections::BTreeMap;

impl MemoryBackend {
    pub(super) fn memory_search(
        &self,
        query: &SearchQuery,
        job: &JobContext,
    ) -> Result<Vec<Oid>, EngineError> {
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "repo.search",
            });
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
