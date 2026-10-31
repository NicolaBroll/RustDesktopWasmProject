//! Deletion of untracked files: `worktree.clean`. The plan and the checks shared by the backends
//! are in `git/clean.rs`; contract in `impl-docs/git/discard.md` ("Deleting untracked files").

use super::{JournalState, Out, check_path_list, reread, to_json};
use crate::{CleanedFile, EngineError, HeadState, JournalEntry, Message, Session};
use project_domain::{Oid, RepoPath, Status};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Result of `worktree.clean` (with `"confirm": true`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CleanResult {
    /// The paths deleted, in path order.
    pub deleted: Vec<RepoPath>,
    /// The same files with the mode they were saved with and their size.
    pub files: Vec<CleanedFile>,
    /// The folders removed because the deletion left them empty, deepest first.
    pub dirs: Vec<RepoPath>,
    /// The backup ref `refs/project/backup/clean-...` (`git show <ref>:<path>`).
    pub backup: String,
    /// The backup commit (no parent; its tree has exactly the deleted files).
    pub commit: Oid,
    /// The total size of the deleted files, in bytes.
    pub bytes: u64,
    /// Files that could not be deleted after the backup (they stay), usually empty.
    pub warnings: Vec<Message>,
    /// The status re-read after the deletion; `null` if it could not be re-read (see
    /// `status_error`): the deletion is done anyway.
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Parameters of `worktree.clean`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CleanParams {
    /// The untracked files, or folders (= the untracked files under them).
    pub paths: Vec<RepoPath>,
    /// Always required to delete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
}

/// Deletes the untracked files `paths` (files or folders: the untracked files under them, as
/// `repo.status` lists them; see [`clean_plan`](crate::clean_plan) for the refusals: tracked,
/// ignored, conflicted or missing paths, nested repositories). Needs `"confirm": true`: without
/// it, [`EngineError::ConfirmRequired`] with the files and their total size, and nothing
/// changes (the refusals come first). Before deleting, the exact bytes and mode of every file
/// are saved in `refs/project/backup/clean-...`; the operation goes into the journal and
/// `edit.undo` recreates the files where the path is still absent.
///
/// Result: [`CleanResult`].
pub(super) fn worktree_clean(s: &mut Session, CleanParams { paths, confirm }: CleanParams) -> Out {
    const ID: &str = "worktree.clean";
    let paths = check_path_list(ID, paths)?;
    let (repo, journal) = s.repo_and_journal()?;
    let head = HeadState::from_head(&repo.head()?);
    let files = repo.clean_plan(&paths)?;
    if !confirm {
        let names: Vec<String> = files.iter().map(|f| format!("`{}`", f.path)).collect();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-clean-confirm",
                n = files.len(),
                files = names.join(", "),
                bytes = total(&files)
            ),
            expected: None,
        });
    }
    // The backend re-reads the state and refuses if the files are no longer these.
    let outcome = repo.clean(&paths, &files)?;
    // The files are deleted: the entry goes into the journal even if the state cannot be re-read.
    if !outcome.files.is_empty() {
        journal.record(JournalEntry::clean(&outcome, head));
    }
    let (status, status_error) = reread(repo.status());
    Ok(to_json(CleanResult {
        deleted: outcome.files.iter().map(|f| f.path.clone()).collect(),
        bytes: total(&outcome.files),
        files: outcome.files,
        dirs: outcome.dirs,
        backup: outcome.backup,
        commit: outcome.commit,
        warnings: outcome.warnings,
        status,
        status_error,
        journal: JournalState::of(journal),
    }))
}

fn total(files: &[CleanedFile]) -> u64 {
    files.iter().map(|f| f.size).sum()
}

#[cfg(test)]
mod tests {
    use super::CleanResult;
    use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
    use project_domain::{EntryKind, Status};
    use serde_json::json;
    use std::sync::Arc;

    fn open() -> (Registry, Session) {
        let mut repo = MemoryBackend::sample();
        repo.add_untracked("build/out.log", "log\n");
        repo.add_untracked("build/sub/a.o", "abc");
        let mut opener = MemoryOpener::new();
        opener.insert(MemoryOpener::SAMPLE_PATH, repo);
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(opener));
        r.run(
            "repo.open",
            &mut s,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
        (r, s)
    }

    fn untracked(r: &Registry, s: &mut Session) -> Vec<String> {
        let status: Status =
            serde_json::from_value(r.run("repo.status", s, json!({})).unwrap()).unwrap();
        status
            .entries
            .into_iter()
            .filter(|e| matches!(e.kind, EntryKind::Untracked))
            .map(|e| e.path.to_string())
            .collect()
    }

    #[test]
    fn needs_confirm_then_deletes_and_undo_restores() {
        let (r, mut s) = open();
        let params = json!({ "paths": ["build", "note.txt"] });
        let err = r.run("worktree.clean", &mut s, params).unwrap_err();
        match &err {
            EngineError::ConfirmRequired {
                command, reason, ..
            } => {
                assert_eq!(*command, "worktree.clean");
                let text = reason.to_string();
                assert!(text.contains("3 untracked files"), "{text}");
                assert!(text.contains("`build/sub/a.o`"), "{text}");
                assert!(text.contains("26 bytes"), "{text}");
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(untracked(&r, &mut s).len(), 3);

        let out = r
            .run(
                "worktree.clean",
                &mut s,
                json!({ "paths": ["build", "note.txt"], "confirm": true }),
            )
            .unwrap();
        let typed: CleanResult = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        assert_eq!(
            out["deleted"],
            json!(["build/out.log", "build/sub/a.o", "note.txt"])
        );
        assert_eq!(out["bytes"], 26);
        assert_eq!(out["backup"], "refs/project/backup/clean-1");
        assert_eq!(out["can_undo"], true);
        assert!(untracked(&r, &mut s).is_empty());

        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        let entry = &history["done"][0];
        assert_eq!(entry["command"], "worktree.clean");
        assert_eq!(entry["clean"]["files"].as_array().unwrap().len(), 3);
        assert_eq!(entry["backup"], "refs/project/backup/clean-1");

        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(undo["undone"]["command"], "worktree.clean");
        assert_eq!(untracked(&r, &mut s).len(), 3);
        r.run("edit.redo", &mut s, json!({})).unwrap();
        assert!(untracked(&r, &mut s).is_empty());
    }

    #[test]
    fn refusals_change_nothing() {
        let (r, mut s) = open();
        for (paths, needle) in [
            (json!(["README.md"]), "`README.md`"),
            (json!(["missing.txt"]), "`missing.txt`"),
            (json!(["note.txt", "src/lib.rs"]), "`src/lib.rs`"),
        ] {
            let err = r
                .run(
                    "worktree.clean",
                    &mut s,
                    json!({ "paths": paths, "confirm": true }),
                )
                .unwrap_err();
            assert!(
                matches!(&err, EngineError::NotAllowed(m) if m.contains(needle)),
                "{err}"
            );
        }
        assert_eq!(untracked(&r, &mut s).len(), 3);
        for bad in [
            json!({}),
            json!({ "paths": [], "confirm": true }),
            json!({ "paths": ["note.txt"], "confirm": "yes" }),
            json!({ "paths": ["note.txt"], "confirm": true, "force": true }),
            json!({ "paths": ["../x"], "confirm": true }),
        ] {
            let err = r.run("worktree.clean", &mut s, bad.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{bad}: {err}"
            );
        }
    }
}
