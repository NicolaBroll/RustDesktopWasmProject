//! Discard of the unstaged changes: `worktree.discard` and `worktree.discard_lines`. The plan,
//! the backup and the restore shared by the backends are in `git/discard.rs` and
//! `git/discard_lines.rs`; contract in `impl-docs/git/discard.md`.

use super::{JournalState, Out, check_path, check_path_list, reread, to_json};
use crate::{
    DiscardLines, DiscardedFile, EngineError, HeadState, JournalEntry, LinesBackup, Message,
    Session, discard_line_patch, discard_lines_confirm, discard_lines_target, discard_plan,
};
use project_domain::{LineRef, Oid, RepoPath, Status};
use schemars::JsonSchema;
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
/// [`GitIndex::discard`](crate::GitIndex::discard) and [`discard_plan`]: untracked files are
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

/// Result of `worktree.discard_lines` (with `"confirm": true`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscardLinesResult {
    /// The file, its mode (kept) and the blobs with its exact bytes before and after.
    #[serde(flatten)]
    pub file: LinesBackup,
    /// The backup ref `refs/project/backup/discard-...`: a commit with only the file, with its
    /// bytes and mode before the discard.
    pub backup: String,
    /// Something not as expected after the write; usually empty.
    pub warnings: Vec<Message>,
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

/// Parameters of `worktree.discard_lines`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiscardLinesParams {
    /// The file.
    pub path: RepoPath,
    /// The chosen changed lines of its `diff.worktree` diff, by their number on the old or the
    /// new side.
    pub lines: Vec<LineRef>,
    /// The digest of the file in the `diff.worktree` diff seen.
    pub digest: String,
    /// Always required to discard.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
}

/// Discards the chosen `lines` of the unstaged changes of `path`: they go back to the index
/// version, the other changes stay (see [`GitIndex::discard_lines`](crate::GitIndex::discard_lines)).
/// The refusals come first (those of [`discard_lines_target`] and [`discard_line_patch`]: stale
/// digest, lines that are not changed lines...); then without `"confirm": true`
/// [`EngineError::ConfirmRequired`] with how many lines would be lost, and nothing changes. The
/// backend saves the file's bytes in `refs/project/backup/discard-...` before writing; the
/// operation goes into the journal and `edit.undo` puts the bytes back.
///
/// Result: [`DiscardLinesResult`].
pub(super) fn worktree_discard_lines(
    s: &mut Session,
    DiscardLinesParams {
        path,
        lines,
        digest,
        confirm,
    }: DiscardLinesParams,
) -> Out {
    const ID: &str = "worktree.discard_lines";
    let path = check_path(ID, path)?;
    if lines.is_empty() {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-lines-empty"),
        });
    }
    let selection = DiscardLines {
        path,
        lines,
        digest,
    };
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    discard_lines_target(&status, &selection.path)?;
    let prepared = discard_line_patch(&repo.diff(&selection.diff_spec())?, &selection)?;
    if !confirm {
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: discard_lines_confirm(&selection.path, &prepared),
            expected: None,
        });
    }
    // The backend re-reads status and diff and refuses if they are no longer these.
    let outcome = repo.discard_lines(&selection)?;
    journal.record(JournalEntry::discard_lines(
        &outcome,
        HeadState::from_head(&status.head),
    ));
    let (status, status_error) = reread(repo.status());
    Ok(to_json(DiscardLinesResult {
        file: outcome.file,
        backup: outcome.backup,
        warnings: outcome.warnings,
        status,
        status_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
mod tests {
    use super::{DiscardLinesResult, DiscardResult};
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

    /// `worktree.discard_lines`: confirmation, typed result, journal, undo and redo.
    #[test]
    fn discard_lines_round_trip_and_undo() {
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        r.run(
            "repo.open",
            &mut s,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
        let diff = r
            .run("diff.worktree", &mut s, json!({ "path": "src/lib.rs" }))
            .unwrap();
        let file: project_domain::FileDiff =
            serde_json::from_value(diff["files"][0].clone()).unwrap();
        let digest = diff["files"][0]["digest"].clone();
        let first = file.line_refs()[0];
        let params = json!({ "path": "src/lib.rs", "lines": [first], "digest": digest });
        let status = |r: &Registry, s: &mut Session| r.run("repo.status", s, json!({})).unwrap();
        let before = status(&r, &mut s);

        // Without confirm: nothing changes.
        let err = r
            .run("worktree.discard_lines", &mut s, params.clone())
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::ConfirmRequired {
                    command: "worktree.discard_lines",
                    ..
                }
            ),
            "{err}"
        );
        assert_eq!(status(&r, &mut s), before);
        // Empty lines, unknown fields, missing digest: invalid parameters.
        for bad in [
            json!({ "path": "src/lib.rs", "lines": [], "digest": digest, "confirm": true }),
            json!({ "path": "src/lib.rs", "lines": [first], "confirm": true }),
            json!({ "path": "src/lib.rs", "lines": [first], "digest": digest, "x": 1 }),
            json!({ "path": "", "lines": [first], "digest": digest, "confirm": true }),
        ] {
            let err = r.run("worktree.discard_lines", &mut s, bad).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
        }

        let mut confirmed = params;
        confirmed["confirm"] = json!(true);
        let out = r
            .run("worktree.discard_lines", &mut s, confirmed.clone())
            .unwrap();
        let typed: DiscardLinesResult = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        assert_eq!(typed.backup, "refs/project/backup/discard-1");
        assert_eq!(typed.file.path.to_string(), "src/lib.rs");
        assert!(typed.journal.can_undo);
        assert_eq!(
            Some(serde_json::from_value(status(&r, &mut s)).unwrap()),
            typed.status
        );
        // The same digest again: the diff changed.
        let err = r
            .run("worktree.discard_lines", &mut s, confirmed)
            .unwrap_err();
        assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");

        assert_eq!(out["next_undo"]["text"], "Discard lines of “src/lib.rs”");
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        let entry = &history["done"][0];
        assert_eq!(entry["command"], "worktree.discard_lines");
        assert_eq!(entry["lines"]["path"], "src/lib.rs");
        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(undo["undone"]["command"], "worktree.discard_lines");
        let now = |r: &Registry, s: &mut Session| {
            r.run("diff.worktree", s, json!({ "path": "src/lib.rs" }))
                .unwrap()
        };
        assert_eq!(now(&r, &mut s), diff);
        r.run("edit.redo", &mut s, json!({})).unwrap();
        assert_ne!(now(&r, &mut s), diff);
    }
}
