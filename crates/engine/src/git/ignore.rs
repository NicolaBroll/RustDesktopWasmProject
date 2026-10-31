//! Ignoring files: `worktree.ignore` appends patterns to the root `.gitignore` or to
//! `.git/info/exclude` (`impl-docs/git/ignore.md`).
//!
//! Choices, shared by the backends:
//!
//! - **Only the two root files.** The patterns are relative to the root, as
//!   [`ignore_pattern`](project_domain::ignore_pattern) builds them; a `.gitignore` in a
//!   subfolder would need patterns relative to it.
//! - **Append only**, with [`append_ignore_patterns`](project_domain::append_ignore_patterns): a
//!   pattern already present as a line is not added again, the file's line endings are kept.
//!   Nothing is lost, so no `confirm`; but the file is rewritten whole (atomically), so the
//!   journal entry ([`IgnoreRecord`]) has the blobs of the exact bytes before and after: undo
//!   writes the old bytes back (or removes the file it created) only if the file has exactly
//!   the written bytes, redo the other way round.
//! - **The effect is reported, not prevented**: the untracked files that became ignored, and the
//!   tracked files the new patterns match (git keeps tracking them: a warning).

use crate::{EngineError, Message, MessageArg};
use project_domain::{IgnorePattern, Oid, PatternProblem, RepoPath, check_ignore_pattern};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The file `worktree.ignore` writes to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IgnoreFile {
    /// `.gitignore` at the root of the worktree: shared with whoever gets the repo (once
    /// committed).
    #[default]
    Gitignore,
    /// `.git/info/exclude` (`git rev-parse --git-path info/exclude`): only this clone, never
    /// committed.
    Exclude,
}

impl IgnoreFile {
    /// The name shown to people: `.gitignore` or `.git/info/exclude`.
    pub fn label(self) -> &'static str {
        match self {
            IgnoreFile::Gitignore => ".gitignore",
            IgnoreFile::Exclude => ".git/info/exclude",
        }
    }

    /// The path of the file in the backup commits: `.gitignore` or `info/exclude`.
    pub fn backup_path(self) -> RepoPath {
        match self {
            IgnoreFile::Gitignore => RepoPath::from(".gitignore"),
            IgnoreFile::Exclude => RepoPath::from("info/exclude"),
        }
    }
}

/// What `worktree.ignore` asks of the backend ([`GitIndex::ignore`](crate::GitIndex::ignore)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreRequest {
    pub file: IgnoreFile,
    /// Checked by [`check_ignore_patterns`]: not empty, no repetitions.
    pub patterns: Vec<IgnorePattern>,
}

/// The file of a `worktree.ignore` in a journal entry: undo writes back the bytes of `before`
/// (removes the file if `None`: it was created), redo those of `after`, each only if the file
/// has exactly the other ones.
///
/// JSON: `{"file": "gitignore" | "exclude", "before": oid | null, "after": oid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IgnoreRecord {
    pub file: IgnoreFile,
    /// Blob with the exact bytes before (no filters); `None` if the file did not exist.
    pub before: Option<Oid>,
    /// Blob with the exact bytes written (no filters).
    pub after: Oid,
}

/// Outcome of [`GitIndex::ignore`](crate::GitIndex::ignore).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreOutcome {
    pub file: IgnoreFile,
    /// The patterns appended, in the given order.
    pub added: Vec<IgnorePattern>,
    /// The patterns already present as a line: not appended.
    pub already: Vec<IgnorePattern>,
    /// The untracked files that the write made ignored, in path order.
    pub ignored: Vec<RepoPath>,
    /// The tracked files the appended patterns match, in path order: git keeps tracking them.
    pub tracked: Vec<RepoPath>,
    /// `true` if the file did not exist and was created.
    pub created: bool,
    /// The bytes before and after, for the journal; `None` if nothing was written (every
    /// pattern already present).
    pub record: Option<IgnoreRecord>,
    /// The backup ref (`refs/project/backup/ignore-...`) that keeps both blobs of `record`
    /// reachable; `None` if nothing was written.
    pub backup: Option<String>,
    /// Something not as expected after the write (the file reread is not what was written).
    pub warnings: Vec<Message>,
}

/// What `edit.undo`/`edit.redo` of a `worktree.ignore` ask of the backend
/// ([`GitIndex::restore_ignore`](crate::GitIndex::restore_ignore)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreRestore {
    /// `edit.undo` or `edit.redo`, for errors.
    pub command: &'static str,
    /// `false` (undo): the file must have the bytes of `after` and gets those of `before` (or is
    /// removed); `true` (redo): the other way round.
    pub redo: bool,
    pub record: IgnoreRecord,
}

impl IgnoreRestore {
    /// The blobs (expected now, to write); `None` = no file.
    pub fn blobs(&self) -> (Option<Oid>, Option<Oid>) {
        let (before, after) = (self.record.before, Some(self.record.after));
        if self.redo {
            (before, after)
        } else {
            (after, before)
        }
    }

    /// [`EngineError::UndoConflict`]: the file is not as the operation (or the undo) left it.
    pub fn conflict(&self) -> EngineError {
        EngineError::UndoConflict {
            command: self.command,
            refs: vec![msg!(
                "engine-ignore-file-changed-undo",
                file = self.record.file.label()
            )],
        }
    }
}

/// The patterns of `worktree.ignore` (`id`), checked: at least one, each one meaningful line
/// ([`check_ignore_pattern`]); repetitions dropped (the first kept). Otherwise
/// [`EngineError::InvalidParams`].
pub fn check_ignore_patterns(
    id: &'static str,
    patterns: Vec<IgnorePattern>,
) -> Result<Vec<IgnorePattern>, EngineError> {
    if patterns.is_empty() {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-ignore-no-patterns"),
        });
    }
    let mut out: Vec<IgnorePattern> = Vec::with_capacity(patterns.len());
    for pattern in patterns {
        if let Err(problem) = check_ignore_pattern(pattern.as_bytes()) {
            let pattern = pattern.to_string();
            let reason = match problem {
                PatternProblem::Empty => msg!("engine-ignore-pattern-empty"),
                PatternProblem::LineBreak => {
                    msg!("engine-ignore-pattern-line-break", pattern = pattern)
                }
                PatternProblem::Blank => msg!("engine-ignore-pattern-blank"),
                PatternProblem::Comment => msg!("engine-ignore-pattern-comment", pattern = pattern),
            };
            return Err(EngineError::InvalidParams { id, reason });
        }
        if !out.contains(&pattern) {
            out.push(pattern);
        }
    }
    Ok(out)
}

/// The warning for tracked files matched by the new patterns: git keeps tracking them.
pub fn ignore_tracked_warning(tracked: &[RepoPath]) -> Message {
    msg!(
        "engine-ignore-tracked",
        n = tracked.len(),
        files = super::clean::list(tracked)
    )
}

/// The description for the journal: `Ignore “*.log” in .gitignore` or `Ignore 3 patterns
/// (“a”, “b”, “c”) in .git/info/exclude`.
pub fn ignore_description(file: IgnoreFile, added: &[IgnorePattern]) -> Message {
    match added {
        [one] => msg!(
            "engine-journal-ignore-one",
            pattern = one.to_string(),
            file = file.label()
        ),
        _ => {
            let mut names: Vec<Message> = added
                .iter()
                .take(3)
                .map(|p| msg!("engine-journal-quoted", name = p.to_string()))
                .collect();
            if added.len() > 3 {
                names.push(Message::plain("..."));
            }
            msg!(
                "engine-journal-ignore-many",
                n = added.len(),
                names = MessageArg::Joined(names, ", ".into()),
                file = file.label()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryBackend;

    fn p(s: &str) -> IgnorePattern {
        IgnorePattern::from(s)
    }

    #[test]
    fn patterns_are_checked() {
        let id = "worktree.ignore";
        assert_eq!(
            check_ignore_patterns(id, vec![p("a"), p("b"), p("a")]).unwrap(),
            [p("a"), p("b")]
        );
        for bad in [
            vec![],
            vec![p("")],
            vec![p("  ")],
            vec![p("#x")],
            vec![p("a"), p("b\nc")],
            vec![IgnorePattern::from_bytes(b"a\0".to_vec())],
        ] {
            assert!(
                matches!(
                    check_ignore_patterns(id, bad.clone()),
                    Err(EngineError::InvalidParams { .. })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn restore_blobs_descriptions_and_json() {
        let record = IgnoreRecord {
            file: IgnoreFile::Exclude,
            before: None,
            after: MemoryBackend::fake_oid(2),
        };
        let undo = IgnoreRestore {
            command: "edit.undo",
            redo: false,
            record: record.clone(),
        };
        assert_eq!(undo.blobs(), (Some(record.after), None));
        let redo = IgnoreRestore {
            redo: true,
            command: "edit.redo",
            ..undo
        };
        assert_eq!(redo.blobs(), (None, Some(record.after)));
        assert!(matches!(
            redo.conflict(),
            EngineError::UndoConflict {
                command: "edit.redo",
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(&record).unwrap(),
            serde_json::json!({
                "file": "exclude",
                "before": null,
                "after": record.after.to_string(),
            })
        );
        assert_eq!(
            ignore_description(IgnoreFile::Gitignore, &[p("*.log")]).to_string(),
            "Ignore “*.log” in .gitignore"
        );
        assert_eq!(
            ignore_description(IgnoreFile::Exclude, &[p("a"), p("b"), p("c"), p("d")]).to_string(),
            "Ignore 4 patterns (“a”, “b”, “c”, ...) in .git/info/exclude"
        );
    }
}
