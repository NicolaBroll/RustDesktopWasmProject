//! Commit: `commit.create`. The checks shared by the backends and the `amend` impact are in
//! `git/commit.rs`; contract in `impl-docs/git/commit.md`.

use super::{Out, to_json};
use crate::{CommitOutcome, CommitRequest, EngineError, JournalEntry, Session};
use project_domain::Status;
use serde::{Deserialize, Serialize};

/// Result of `commit.create`: the fields of [`CommitOutcome`] and the new status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitCreateResult {
    /// The created commit, HEAD before, the moved branch and, with `amend`, the backup ref of
    /// the rewritten commit and the other branches and tags left on it.
    #[serde(flatten)]
    pub outcome: CommitOutcome,
    /// The status after the commit (an error re-reading it is [`EngineError::PostCommit`]).
    pub status: Status,
}

/// Creates a commit with what is staged, or with `amend` rewrites the last one (see
/// [`GitIndex::commit`](crate::GitIndex::commit)). `confirm` is needed only for `amend` of a
/// commit already published. An error after a successful commit is
/// [`EngineError::PostCommit`].
///
/// Result: [`CommitCreateResult`].
pub(super) fn commit_create(s: &mut Session, request: CommitRequest) -> Out {
    request.check_message()?;
    let (repo, journal) = s.repo_and_journal()?;
    let outcome = repo.commit(&request)?;
    // The commit is made: it goes into the journal (for `edit.undo`) even if the state cannot be re-read afterwards.
    if let Some(entry) = JournalEntry::commit(&outcome, request.amend) {
        journal.record(entry);
    }
    // The commit is made: an error from here on must not look like a failed commit.
    let status = repo.status().map_err(|e| EngineError::PostCommit {
        commit: Some(outcome.commit.id),
        backup: outcome.backup.clone(),
        reason: msg!("engine-status-unreadable", error = e),
    })?;
    Ok(to_json(CommitCreateResult { outcome, status }))
}

#[cfg(test)]
mod tests {
    use super::CommitCreateResult;
    use crate::{MemoryOpener, Registry, Session};
    use project_domain::Status;
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn same(out: Value) -> CommitCreateResult {
        let typed: CommitCreateResult = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        typed
    }

    /// The typed result reads back the JSON of the command and gives the same JSON again (a
    /// commit and an `amend` with backup and `also_on`): the UI deserializes into it.
    #[test]
    fn result_round_trips() {
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        r.run(
            "repo.open",
            &mut s,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
        let status = |r: &Registry, s: &mut Session| -> Status {
            serde_json::from_value(r.run("repo.status", s, json!({})).unwrap()).unwrap()
        };

        let one = same(
            r.run("commit.create", &mut s, json!({ "message": "One" }))
                .unwrap(),
        );
        assert_eq!(one.outcome.commit.summary, "One");
        assert_eq!(one.outcome.branch.as_deref(), Some("refs/heads/main"));
        assert!(one.outcome.previous.is_some());
        assert_eq!(one.outcome.backup, None);
        assert!(one.outcome.also_on.is_empty());
        assert_eq!(one.status, status(&r, &mut s));

        r.run(
            "branch.create",
            &mut s,
            json!({ "name": "keep", "start": "HEAD" }),
        )
        .unwrap();
        let amended = same(
            r.run(
                "commit.create",
                &mut s,
                json!({ "message": "One, amended", "amend": true }),
            )
            .unwrap(),
        );
        assert_eq!(amended.outcome.previous, Some(one.outcome.commit.id));
        assert!(amended.outcome.backup.is_some());
        assert_eq!(amended.outcome.also_on, vec!["refs/heads/keep".to_string()]);
        assert_eq!(amended.status, status(&r, &mut s));
    }
}
