//! Ignoring files: `worktree.ignore`. The types and the checks shared by the backends are in
//! `git/ignore.rs`; contract in `impl-docs/git/ignore.md`.

use super::{JournalState, Out, reread, to_json};
use crate::{
    HeadState, IgnoreFile, IgnoreRequest, JournalEntry, Message, Session, check_ignore_patterns,
    ignore_tracked_warning,
};
use project_domain::{IgnorePattern, RepoPath, Status};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters of `worktree.ignore`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IgnoreParams {
    /// The patterns, one line each (for one path, folder or extension build them with
    /// `project_domain::ignore_pattern`). Not empty; no NUL, CR or LF; not only spaces; not
    /// starting with `#` (escape it as `\#`).
    pub patterns: Vec<IgnorePattern>,
    /// `gitignore` (default: the root `.gitignore`, shared once committed) or `exclude`
    /// (`.git/info/exclude`: only this clone).
    #[serde(default)]
    pub file: IgnoreFile,
}

/// Result of `worktree.ignore`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IgnoreResult {
    /// `gitignore` or `exclude`.
    pub file: IgnoreFile,
    /// The file for people: `.gitignore` or `.git/info/exclude`.
    pub label: String,
    /// The patterns appended, in the given order.
    pub added: Vec<IgnorePattern>,
    /// The patterns already present as a line: not appended. If all are, nothing was written
    /// and nothing goes into the journal.
    pub already: Vec<IgnorePattern>,
    /// The untracked files that became ignored (they leave the status), in path order.
    pub ignored: Vec<RepoPath>,
    /// The tracked files the appended patterns match: git keeps tracking them (a warning says
    /// so; `git rm --cached` would stop it).
    pub tracked: Vec<RepoPath>,
    /// `true` if the file did not exist and was created.
    pub created: bool,
    /// The backup ref `refs/project/backup/ignore-...` with the bytes before and after (`null`
    /// if nothing was written).
    pub backup: Option<String>,
    /// The tracked files matched, and anything not as expected after the write.
    pub warnings: Vec<Message>,
    /// The status re-read after the write; `null` if it could not be re-read (see
    /// `status_error`): the write is done anyway.
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Appends `patterns` to the root `.gitignore` or to `.git/info/exclude` (see
/// [`GitIndex::ignore`](crate::GitIndex::ignore)). No `confirm`: nothing is lost, and the
/// operation goes into the journal (`edit.undo` puts the exact old bytes back, or removes the
/// file it created, only if the file still has the bytes written).
///
/// Result: [`IgnoreResult`].
pub(super) fn worktree_ignore(
    s: &mut Session,
    IgnoreParams { patterns, file }: IgnoreParams,
) -> Out {
    const ID: &str = "worktree.ignore";
    let patterns = check_ignore_patterns(ID, patterns)?;
    let (repo, journal) = s.repo_and_journal()?;
    let head = HeadState::from_head(&repo.head()?);
    let outcome = repo.ignore(&IgnoreRequest { file, patterns })?;
    // The file is written: the entry goes into the journal even if the state cannot be re-read.
    if outcome.record.is_some() {
        journal.record(JournalEntry::ignore(&outcome, head));
    }
    let mut warnings = Vec::new();
    if !outcome.tracked.is_empty() {
        warnings.push(ignore_tracked_warning(&outcome.tracked));
    }
    warnings.extend(outcome.warnings);
    let (status, status_error) = reread(repo.status());
    Ok(to_json(IgnoreResult {
        file,
        label: file.label().to_owned(),
        added: outcome.added,
        already: outcome.already,
        ignored: outcome.ignored,
        tracked: outcome.tracked,
        created: outcome.created,
        backup: outcome.backup,
        warnings,
        status,
        status_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
mod tests {
    use super::IgnoreResult;
    use crate::{EngineError, MemoryBackend, MemoryOpener, Registry, Session};
    use project_domain::{EntryKind, Status};
    use serde_json::json;
    use std::sync::Arc;

    fn open() -> (Registry, Session) {
        let mut repo = MemoryBackend::sample();
        repo.add_untracked("build/out.log", "log\n");
        repo.add_untracked("debug.log", "x");
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
    fn appends_and_undo_redo() {
        let (r, mut s) = open();
        assert_eq!(
            untracked(&r, &mut s),
            ["build/out.log", "debug.log", "note.txt"]
        );
        let out = r
            .run(
                "worktree.ignore",
                &mut s,
                json!({ "patterns": ["*.log", "/README.md", "*.log"] }),
            )
            .unwrap();
        let typed: IgnoreResult = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        assert_eq!(out["file"], "gitignore");
        assert_eq!(out["label"], ".gitignore");
        assert_eq!(out["added"], json!(["*.log", "/README.md"]));
        assert_eq!(out["already"], json!([]));
        assert_eq!(out["ignored"], json!(["build/out.log", "debug.log"]));
        assert_eq!(out["tracked"], json!(["README.md"]));
        assert_eq!(out["created"], false);
        assert_eq!(out["backup"], "refs/project/backup/ignore-1");
        assert_eq!(out["warnings"][0]["key"], "engine-ignore-tracked");
        assert_eq!(out["can_undo"], true);
        assert_eq!(untracked(&r, &mut s), ["note.txt"]);

        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        let entry = &history["done"][0];
        assert_eq!(entry["command"], "worktree.ignore");
        assert_eq!(entry["ignore"]["file"], "gitignore");

        // Again: everything already there, nothing written, nothing in the journal.
        let again = r
            .run("worktree.ignore", &mut s, json!({ "patterns": ["*.log"] }))
            .unwrap();
        assert_eq!(again["added"], json!([]));
        assert_eq!(again["already"], json!(["*.log"]));
        assert_eq!(again["backup"], json!(null));
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["done"].as_array().unwrap().len(), 1);

        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(undo["undone"]["command"], "worktree.ignore");
        assert_eq!(untracked(&r, &mut s).len(), 3);
        r.run("edit.redo", &mut s, json!({})).unwrap();
        assert_eq!(untracked(&r, &mut s), ["note.txt"]);
    }

    #[test]
    fn exclude_file_created_and_removed_by_undo() {
        let (r, mut s) = open();
        let out = r
            .run(
                "worktree.ignore",
                &mut s,
                json!({ "patterns": ["/note.txt"], "file": "exclude" }),
            )
            .unwrap();
        assert_eq!(out["label"], ".git/info/exclude");
        assert_eq!(out["created"], true);
        assert_eq!(out["ignored"], json!(["note.txt"]));
        // `.gitignore` untouched: `note.txt` is the only one gone.
        assert_eq!(untracked(&r, &mut s), ["build/out.log", "debug.log"]);
        r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(untracked(&r, &mut s).len(), 3);
    }

    #[test]
    fn invalid_params_change_nothing() {
        let (r, mut s) = open();
        for bad in [
            json!({}),
            json!({ "patterns": [] }),
            json!({ "patterns": [""] }),
            json!({ "patterns": ["   "] }),
            json!({ "patterns": ["#comment"] }),
            json!({ "patterns": ["a\nb"] }),
            json!({ "patterns": ["a\r"] }),
            json!({ "patterns": ["a"], "file": "global" }),
            json!({ "patterns": ["a"], "confirm": true }),
        ] {
            let err = r.run("worktree.ignore", &mut s, bad.clone()).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{bad}: {err}"
            );
        }
        assert_eq!(untracked(&r, &mut s).len(), 3);
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert!(history["done"].as_array().unwrap().is_empty());
    }
}
