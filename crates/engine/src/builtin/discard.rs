//! Discard of the unstaged changes: `worktree.discard`. The plan, the backup and the restore
//! shared by the backends are in `git/discard.rs`; contract in `impl-docs/git/discard.md`.

use super::{JournalState, Out, check_path_list, reread, to_json};
use crate::{DiscardedFile, EngineError, HeadState, JournalEntry, Session, discard_plan};
use project_domain::{Oid, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// Result of `worktree.discard` (with `"confirm": true`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscardResult {
    /// The files restored to the staged version, in path order.
    pub files: Vec<DiscardedFile>,
    /// The backup ref `refs/project/backup/discard-...` of the worktree.
    pub backup: String,
    /// The commit of `git stash create` with the worktree before the discard.
    pub stash: Oid,
    /// The status re-read after the discard; `null` if it could not be re-read (see
    /// `status_error`): the discard is done anyway.
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Parameters of `worktree.discard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardParams {
    /// The files or folders.
    pub paths: Vec<RepoPath>,
    /// Always required to discard.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
}

/// Discards the unstaged changes of the files `paths` (files or folders): the worktree goes back
/// to the index, the index is not touched (see
/// [`GitBackend::discard`](crate::GitBackend::discard) and [`discard_plan`]: untracked files are
/// never touched). Needs `"confirm": true`: without it, [`EngineError::ConfirmRequired`] with the files
/// that would be lost and nothing changes (the refusals of [`discard_plan`] come first). Before
/// writing, the worktree is saved with `git stash create` in `refs/project/backup/discard-...`;
/// the operation goes into the journal and `edit.undo` puts the files back as they were.
///
/// Result: [`DiscardResult`].
pub(super) fn worktree_discard(
    s: &mut Session,
    DiscardParams { paths, confirm }: DiscardParams,
) -> Out {
    const ID: &str = "worktree.discard";
    let paths = check_path_list(ID, paths)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let files = discard_plan(&status, &paths)?;
    if !confirm {
        let names: Vec<String> = files.iter().map(|f| format!("`{}`", f.path)).collect();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-discard-confirm",
                n = files.len(),
                files = names.join(", ")
            ),
            expected: None,
        });
    }
    // The backend re-reads the state and refuses if the files are no longer these.
    let outcome = repo.discard(&paths, &files)?;
    // The files are discarded: the entry goes into the journal even if the state cannot be re-read afterwards.
    journal.record(JournalEntry::discard(
        &outcome,
        HeadState::from_head(&status.head),
    ));
    let (status, status_error) = reread(repo.status());
    Ok(to_json(DiscardResult {
        files: outcome.files,
        backup: outcome.backup,
        stash: outcome.stash,
        status,
        status_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
mod tests {
    use super::DiscardResult;
    use crate::builtin::{JournalState, reread};
    use crate::{EngineError, MemoryOpener, Registry, Session};
    use project_domain::{Change, Status};
    use serde_json::{Value, json};
    use std::sync::Arc;

    /// The typed result reads back the JSON of the command and gives the same JSON again: the
    /// UI deserializes into it.
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
        let out = r
            .run(
                "worktree.discard",
                &mut s,
                json!({ "paths": ["src/lib.rs"], "confirm": true }),
            )
            .unwrap();
        let typed: DiscardResult = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        assert_eq!(typed.files.len(), 1);
        assert_eq!(typed.files[0].path.to_string(), "src/lib.rs");
        assert_eq!(typed.files[0].change, Change::Modified);
        assert_eq!(typed.backup, "refs/project/backup/discard-1");
        assert_eq!(out["stash"], typed.stash.to_string());
        let status: Status =
            serde_json::from_value(r.run("repo.status", &mut s, json!({})).unwrap()).unwrap();
        assert_eq!(typed.status, Some(status));
        assert!(typed.status_error.is_none());
        assert!(typed.journal.can_undo && !typed.journal.can_redo);
    }

    /// A status that cannot be re-read gives `null` and `status_error`.
    #[test]
    fn reread_error_fields() {
        let (status, status_error) = reread::<Status>(Err(EngineError::NoRepo));
        let result = DiscardResult {
            files: vec![],
            backup: "refs/project/backup/discard-1".into(),
            stash: crate::MemoryBackend::fake_oid(1),
            status,
            status_error,
            journal: JournalState::default(),
        };
        let out = serde_json::to_value(&result).unwrap();
        assert_eq!(out["status"], Value::Null);
        assert_eq!(out["status_error"], EngineError::NoRepo.to_string());
        let back: DiscardResult = serde_json::from_value(out).unwrap();
        assert_eq!(back, result);
    }
}
