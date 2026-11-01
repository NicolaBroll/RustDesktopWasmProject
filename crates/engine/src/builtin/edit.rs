//! The operation journal (§7 of the git architecture): `edit.undo`, `edit.redo`,
//! `edit.history`. Undo and redo of the refs are in `journal.rs` and `git/restore.rs`; contract
//! in `impl-docs/git/commit.md`.

use super::{JournalState, NoParams, Out, reread, to_json};
use crate::{Journal, JournalEntry, Message, RestoreOutcome, Session};
use project_domain::Status;
use serde::{Deserialize, Serialize};

/// Result of `edit.undo` (with `undone`) and `edit.redo` (with `redone`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UndoResult {
    /// `edit.undo`: the entry undone; absent for `edit.redo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone: Option<JournalEntry>,
    /// `edit.redo`: the entry redone; absent for `edit.undo`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redone: Option<JournalEntry>,
    /// The backup refs created for the commits that would no longer be reachable.
    pub backups: Vec<String>,
    /// Something unexpected found after the successful move; usually empty.
    pub warnings: Vec<Message>,
    /// The status re-read afterwards; `null` if it could not be re-read (see `status_error`):
    /// the operation is done anyway and must not be repeated (a second undo would undo the
    /// previous entry).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `edit.history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryResult {
    /// The entries done, oldest first: the last is the one `edit.undo` undoes.
    pub done: Vec<JournalEntry>,
    /// The entries undone: the last is the one `edit.redo` redoes.
    pub undone: Vec<JournalEntry>,
    /// The journal state.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Which of `edit.undo`/`edit.redo` produced an [`UndoResult`].
#[derive(Clone, Copy)]
enum Op {
    Undo,
    Redo,
}

/// The [`UndoResult`] of `op`, with the journal and the status after the move.
fn undo_result(s: &Session, op: Op, entry: JournalEntry, outcome: RestoreOutcome) -> Out {
    let journal = JournalState::of(s.journal()?);
    let (status, status_error) = reread(s.repo()?.status());
    let (undone, redone) = match op {
        Op::Undo => (Some(entry), None),
        Op::Redo => (None, Some(entry)),
    };
    Ok(to_json(UndoResult {
        undone,
        redone,
        backups: outcome.backups,
        warnings: outcome.warnings,
        status,
        status_error,
        journal,
    }))
}

/// Undoes the last operation of the journal (see [`Journal::undo`]): the refs go back to their previous
/// values, only if nobody moved them in the meantime ([`EngineError::UndoConflict`](crate::EngineError::UndoConflict)
/// otherwise, and nothing changes). Index and worktree are not touched.
///
/// Result: [`UndoResult`], with the entry in `undone`.
pub(super) fn edit_undo(s: &mut Session, NoParams {}: NoParams) -> Out {
    let (repo, journal) = s.repo_and_journal()?;
    let (entry, outcome) = journal.undo(repo)?;
    undo_result(s, Op::Undo, entry, outcome)
}

/// Redoes the last undone operation (see [`Journal::redo`]), with the same check.
///
/// Result: [`UndoResult`], with the entry in `redone`.
pub(super) fn edit_redo(s: &mut Session, NoParams {}: NoParams) -> Out {
    let (repo, journal) = s.repo_and_journal()?;
    let (entry, outcome) = journal.redo(repo)?;
    undo_result(s, Op::Redo, entry, outcome)
}

/// The journal of the open repo.
///
/// Result: [`HistoryResult`].
pub(super) fn edit_history(s: &mut Session, NoParams {}: NoParams) -> Out {
    let journal: &Journal = s.journal()?;
    Ok(to_json(HistoryResult {
        done: journal.done().to_vec(),
        undone: journal.undone().to_vec(),
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
mod tests {
    use super::{HistoryResult, UndoResult};
    use crate::builtin::snapshot_tests::{
        clean_sample, conflict_repo, id, opened_with, pull_rebase_conflict, run, sample,
    };
    use crate::builtin::{JournalState, reread};
    use crate::{EngineError, MemoryOpener, Registry, Session};
    use project_domain::Status;
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: &Value) -> T {
        let typed: T = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), *out);
        typed
    }

    /// The typed results read back the JSON of the three commands and give the same JSON
    /// again: the UI deserializes into them.
    #[test]
    fn results_round_trip() {
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

        let empty: HistoryResult = same(&r.run("edit.history", &mut s, json!({})).unwrap());
        assert!(empty.done.is_empty() && empty.undone.is_empty());
        assert_eq!(empty.journal, JournalState::default());

        r.run("commit.create", &mut s, json!({ "message": "One" }))
            .unwrap();
        let history: HistoryResult = same(&r.run("edit.history", &mut s, json!({})).unwrap());
        assert_eq!(history.done.len(), 1);
        assert_eq!(history.done[0].command, "commit.create");
        assert!(history.journal.can_undo && !history.journal.can_redo);

        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert!(undo.get("redone").is_none() && undo.get("status_error").is_none());
        let undo: UndoResult = same(&undo);
        // The undone commit is no longer reachable: undo saved it in a backup ref.
        let undone = undo.undone.as_ref().unwrap();
        assert_eq!(undone.id, history.done[0].id);
        assert_eq!(undone.undo_backups, undo.backups);
        assert_eq!(undo.backups.len(), 1);
        assert_eq!(undo.status, Some(status(&r, &mut s)));
        assert!(!undo.journal.can_undo && undo.journal.can_redo);

        let history: HistoryResult = same(&r.run("edit.history", &mut s, json!({})).unwrap());
        assert!(history.done.is_empty() && history.undone.len() == 1);

        let redo = r.run("edit.redo", &mut s, json!({})).unwrap();
        assert!(redo.get("undone").is_none());
        let redo: UndoResult = same(&redo);
        assert_eq!(redo.redone.map(|e| e.id), Some(history.undone[0].id));
        assert_eq!(redo.status, Some(status(&r, &mut s)));
        assert!(redo.journal.can_undo && !redo.journal.can_redo);
    }

    /// A status that cannot be re-read gives `null` and `status_error`.
    #[test]
    fn reread_error_fields() {
        let (status, status_error) = reread::<Status>(Err(EngineError::NoRepo));
        let result = UndoResult {
            undone: None,
            redone: None,
            backups: vec!["refs/project/backup/undo-1".into()],
            warnings: vec![],
            status,
            status_error,
            journal: JournalState::default(),
        };
        let out = serde_json::to_value(&result).unwrap();
        assert_eq!(out["status"], Value::Null);
        assert_eq!(out["status_error"], EngineError::NoRepo.to_string());
        let back: UndoResult = serde_json::from_value(out).unwrap();
        assert_eq!(back, result);
    }

    /// Like [`same`] for results that hold journal entries, except for the `remote` of the
    /// entries of `remote.add`/`remote.remove`: its JSON has the URLs masked and on purpose it
    /// is not read back (`skip_deserializing` on `JournalEntry::remote`), so it comes back `null`.
    fn same_entries<T: serde::de::DeserializeOwned + serde::Serialize>(out: &Value) -> T {
        let typed: T = serde_json::from_value(out.clone()).unwrap();
        let mut expected = out.clone();
        let fields = expected.as_object_mut().unwrap();
        let entries = fields
            .iter_mut()
            .filter(|(key, _)| ["done", "undone", "redone"].contains(&key.as_str()))
            .flat_map(|(_, value)| match value {
                Value::Array(list) => list.iter_mut().collect::<Vec<_>>(),
                entry => vec![entry],
            });
        for entry in entries {
            if !entry["remote"].is_null() {
                assert!(
                    matches!(
                        entry["command"].as_str(),
                        Some("remote.add" | "remote.remove")
                    ),
                    "{entry}"
                );
                entry["remote"] = Value::Null;
            }
        }
        assert_eq!(serde_json::to_value(&typed).unwrap(), expected);
        typed
    }

    /// `edit.history` of `s` reads back into [`HistoryResult`] and gives the same JSON again;
    /// the ids of the commands of its entries.
    fn history_commands(r: &Registry, s: &mut Session) -> Vec<String> {
        let history: HistoryResult = same_entries(&run(r, s, "edit.history", json!({})));
        history
            .done
            .iter()
            .chain(&history.undone)
            .map(|e| e.command.clone())
            .collect()
    }

    /// `edit.undo` then `edit.redo` of the last entry: both results read back into
    /// [`UndoResult`] and give the same JSON again.
    fn undo_redo(r: &Registry, s: &mut Session, command: &str) {
        let undo: UndoResult = same_entries(&run(r, s, "edit.undo", json!({})));
        assert_eq!(undo.undone.unwrap().command, command);
        history_commands(r, s);
        let redo: UndoResult = same_entries(&run(r, s, "edit.redo", json!({})));
        assert_eq!(redo.redone.unwrap().command, command);
    }

    /// Sample repo: branches, tags, discard, stash, remotes, reset, pull, commit.
    fn sample_entries() -> Vec<String> {
        let mut covered = Vec::new();
        let (r, mut s) = sample();
        let s = &mut s;
        run(&r, s, "branch.create", json!({ "name": "topic" }));
        undo_redo(&r, s, "branch.create");
        run(
            &r,
            s,
            "branch.rename",
            json!({ "name": "topic", "new_name": "topic2" }),
        );
        undo_redo(&r, s, "branch.rename");
        run(&r, s, "branch.checkout", json!({ "name": "topic2" }));
        undo_redo(&r, s, "branch.checkout");
        run(&r, s, "branch.checkout", json!({ "name": "main" }));
        run(&r, s, "branch.delete", json!({ "name": "topic2" }));
        undo_redo(&r, s, "branch.delete");
        run(
            &r,
            s,
            "tag.create",
            json!({ "name": "v0.2.0", "message": "Release" }),
        );
        undo_redo(&r, s, "tag.create");
        run(
            &r,
            s,
            "tag.delete",
            json!({ "name": "v0.2.0", "confirm": true }),
        );
        undo_redo(&r, s, "tag.delete");
        run(
            &r,
            s,
            "worktree.discard",
            json!({ "paths": ["src/lib.rs"], "confirm": true }),
        );
        undo_redo(&r, s, "worktree.discard");
        run(&r, s, "stash.push", json!({}));
        undo_redo(&r, s, "stash.push");
        let list = run(&r, s, "stash.list", json!({}));
        let expected = list["stashes"][0]["id"].clone();
        run(
            &r,
            s,
            "stash.drop",
            json!({ "index": 0, "expected": expected, "confirm": true }),
        );
        undo_redo(&r, s, "stash.drop");
        run(
            &r,
            s,
            "remote.add",
            json!({ "name": "fork", "url": "https://example.com/fork.git" }),
        );
        undo_redo(&r, s, "remote.add");
        run(
            &r,
            s,
            "remote.remove",
            json!({ "name": "fork", "confirm": true }),
        );
        undo_redo(&r, s, "remote.remove");
        run(
            &r,
            s,
            "branch.reset",
            json!({ "target": id(11), "mode": "hard", "confirm": true, "expected": id(12) }),
        );
        undo_redo(&r, s, "branch.reset");
        run(&r, s, "remote.pull", json!({ "branch": "feature/graph" }));
        undo_redo(&r, s, "remote.pull");
        run(&r, s, "index.stage", json!({ "paths": ["note.txt"] }));
        run(&r, s, "commit.create", json!({ "message": "One" }));
        undo_redo(&r, s, "commit.create");
        covered.extend(history_commands(&r, s));
        covered
    }

    /// Merges (fast-forward, merge commit, conflict concluded, aborted).
    fn merge_entries() -> Vec<String> {
        let mut covered = Vec::new();
        for (rev, finish) in [
            ("ahead", None),
            ("feat", None),
            ("other", Some("merge.continue")),
            ("other", Some("merge.abort")),
        ] {
            let (r, mut s) = opened_with(conflict_repo());
            let s = &mut s;
            run(&r, s, "merge.start", json!({ "rev": rev }));
            match finish {
                Some("merge.continue") => {
                    run(&r, s, "index.stage", json!({ "paths": ["a.txt"] }));
                    run(&r, s, "merge.continue", json!({}));
                    undo_redo(&r, s, "merge.continue");
                }
                Some(id) => {
                    run(&r, s, id, json!({ "confirm": true }));
                }
                None => undo_redo(&r, s, "merge.start"),
            }
            covered.extend(history_commands(&r, s));
        }
        covered
    }

    /// Cherry-pick and revert, clean and after a conflict.
    fn pick_entries() -> Vec<String> {
        let mut covered = Vec::new();
        let (r, mut s) = opened_with(conflict_repo());
        let s = &mut s;
        run(&r, s, "commit.cherry_pick", json!({ "rev": "feat" }));
        undo_redo(&r, s, "commit.cherry_pick");
        run(&r, s, "commit.revert", json!({ "rev": "main" }));
        undo_redo(&r, s, "commit.revert");
        covered.extend(history_commands(&r, s));
        for (start, cont) in [
            ("commit.cherry_pick", "cherry_pick.continue"),
            ("commit.revert", "revert.continue"),
        ] {
            let (r, mut s) = opened_with(conflict_repo());
            let s = &mut s;
            run(&r, s, start, json!({ "rev": "other" }));
            run(&r, s, "index.stage", json!({ "paths": ["a.txt"] }));
            run(&r, s, cont, json!({}));
            covered.extend(history_commands(&r, s));
        }
        covered
    }

    /// Rebase, and a pull that rebases with a conflict, continued and aborted.
    fn rebase_entries() -> Vec<String> {
        let mut covered = Vec::new();
        let (r, mut s) = opened_with(clean_sample());
        let s = &mut s;
        run(
            &r,
            s,
            "rebase.start",
            json!({
                "base": id(11),
                "steps": [{ "action": "reword", "commit": id(12), "message": "Commit panel, v2" }],
            }),
        );
        undo_redo(&r, s, "rebase.start");
        covered.extend(history_commands(&r, s));
        let (r, mut s) = pull_rebase_conflict();
        let s = &mut s;
        run(&r, s, "remote.pull", json!({}));
        run(&r, s, "index.stage", json!({ "paths": ["src/bin/app.rs"] }));
        run(&r, s, "rebase.continue", json!({}));
        covered.extend(history_commands(&r, s));
        let (r, mut s) = pull_rebase_conflict();
        let s = &mut s;
        run(&r, s, "remote.pull", json!({}));
        run(&r, s, "rebase.abort", json!({ "confirm": true }));
        covered.extend(history_commands(&r, s));
        covered
    }

    /// Every kind of journal entry the `MemoryBackend` produces reads back from its own JSON
    /// (in `edit.history`, `edit.undo` and `edit.redo`): the UI decodes the entries into
    /// [`JournalEntry`](crate::JournalEntry).
    #[test]
    fn every_entry_kind_round_trips() {
        let mut covered = sample_entries();
        covered.extend(merge_entries());
        covered.extend(pick_entries());
        covered.extend(rebase_entries());
        covered.sort();
        covered.dedup();
        for id in [
            "branch.create",
            "branch.rename",
            "branch.checkout",
            "branch.delete",
            "tag.create",
            "tag.delete",
            "worktree.discard",
            "stash.push",
            "stash.drop",
            "remote.add",
            "remote.remove",
            "branch.reset",
            "commit.create",
            "merge.start",
            "merge.continue",
            "commit.cherry_pick",
            "commit.revert",
            "rebase.start",
            "remote.pull",
            "cherry_pick.continue",
            "revert.continue",
        ] {
            assert!(covered.iter().any(|c| c == id), "{id} not in {covered:?}");
        }
    }
}
