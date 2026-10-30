//! Index writes: `index.stage`, `index.unstage`, `index.stage_lines`, `index.unstage_lines`.
//! The backends save the index backup before writing ([`GitBackend::stage`],
//! [`GitBackend::apply_lines`]); contract in `impl-docs/git/stage.md`.

use super::{Out, check_path, check_path_list, to_json};
use crate::{EngineError, GitBackend, LineSelection, Session};
use project_domain::{LineRef, PatchDirection, RepoPath, Status};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Result of `index.stage`, `index.unstage`, `index.stage_lines` and `index.unstage_lines`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexWriteResult {
    /// The new status (the caller updates the view without a second command).
    pub status: Status,
    /// The index backup ref, if before writing staged versions that existed only in the index
    /// were saved; `null` otherwise.
    pub backup: Option<String>,
}

/// Parameters of `index.stage` and `index.unstage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IndexPathsParams {
    /// The files or folders.
    pub paths: Vec<RepoPath>,
}

/// Stages `paths` (see [`GitBackend::stage`]).
///
/// Result: [`IndexWriteResult`].
pub(super) fn index_stage(s: &mut Session, p: IndexPathsParams) -> Out {
    index_paths("index.stage", s, p, |repo, paths| repo.stage(paths))
}

/// Unstages `paths` (see [`GitBackend::unstage`]), without touching the worktree.
///
/// Result: [`IndexWriteResult`].
pub(super) fn index_unstage(s: &mut Session, p: IndexPathsParams) -> Out {
    index_paths("index.unstage", s, p, |repo, paths| repo.unstage(paths))
}

/// The write of `index.stage`/`index.unstage`: the index backup ref, if one was saved.
type IndexWrite = fn(&mut dyn GitBackend, &[RepoPath]) -> Result<Option<String>, EngineError>;

/// `index.stage`/`index.unstage`: `write` on the validated paths, then the new status.
fn index_paths(
    id: &'static str,
    s: &mut Session,
    IndexPathsParams { paths }: IndexPathsParams,
    write: IndexWrite,
) -> Out {
    let paths = check_path_list(id, paths)?;
    let repo = s.repo_mut()?;
    let backup = write(repo, &paths)?;
    Ok(to_json(IndexWriteResult {
        status: repo.status()?,
        backup,
    }))
}

/// Parameters of `index.stage_lines` and `index.unstage_lines`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct IndexLinesParams {
    /// The file.
    pub path: RepoPath,
    /// The chosen lines, by their number on the old or the new side.
    pub lines: Vec<LineRef>,
    /// The digest of the file in the diff seen (`diff.worktree` to stage, `diff.index` to
    /// unstage).
    pub digest: String,
}

/// The `lines` of file `path` staged or unstaged, with the diff seen identified by
/// `digest` (see [`GitBackend::apply_lines`]). An empty
/// selection is an error, not a no-op: it is almost always a mistake by the
/// caller. Result: [`IndexWriteResult`].
fn index_lines(
    id: &'static str,
    direction: PatchDirection,
    s: &mut Session,
    IndexLinesParams {
        path,
        lines,
        digest,
    }: IndexLinesParams,
) -> Out {
    let path = check_path(id, path)?;
    if lines.is_empty() {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-lines-empty"),
        });
    }
    let repo = s.repo_mut()?;
    let backup = repo.apply_lines(&LineSelection {
        path,
        lines,
        digest,
        direction,
    })?;
    Ok(to_json(IndexWriteResult {
        status: repo.status()?,
        backup,
    }))
}

/// Stages the chosen lines of the worktree diff.
pub(super) fn index_stage_lines(s: &mut Session, p: IndexLinesParams) -> Out {
    index_lines("index.stage_lines", PatchDirection::Stage, s, p)
}

/// Unstages the chosen lines of the index diff.
pub(super) fn index_unstage_lines(s: &mut Session, p: IndexLinesParams) -> Out {
    index_lines("index.unstage_lines", PatchDirection::Unstage, s, p)
}

#[cfg(test)]
mod tests {
    use super::IndexWriteResult;
    use crate::{MemoryOpener, Registry, Session};
    use project_domain::FileDiff;
    use serde_json::{Value, json};
    use std::sync::Arc;

    fn same(out: Value) -> IndexWriteResult {
        let typed: IndexWriteResult = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        typed
    }

    /// The digest and the line refs of the diff of `path` (`diff.worktree` or `diff.index`).
    fn lines(r: &Registry, s: &mut Session, id: &str, path: &str) -> (Value, Value) {
        let diff = r.run(id, s, json!({ "path": path })).unwrap();
        let file: FileDiff = serde_json::from_value(diff["files"][0].clone()).unwrap();
        (diff["files"][0]["digest"].clone(), json!(file.line_refs()))
    }

    /// The typed result reads back the JSON of the four commands and gives the same JSON
    /// again (with and without a backup): the UI deserializes into it.
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
        let status = |r: &Registry, s: &mut Session| {
            serde_json::from_value(r.run("repo.status", s, json!({})).unwrap()).unwrap()
        };

        let out = same(
            r.run("index.stage", &mut s, json!({ "paths": ["note.txt"] }))
                .unwrap(),
        );
        assert_eq!(out.backup, None);
        assert_eq!(out.status, status(&r, &mut s));

        let (digest, refs) = lines(&r, &mut s, "diff.worktree", "src/lib.rs");
        let out = same(
            r.run(
                "index.stage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": [refs[0]], "digest": digest }),
            )
            .unwrap(),
        );
        assert_eq!(out.backup, None);
        assert_eq!(out.status, status(&r, &mut s));

        // `src/lib.rs` is now `MM`: unstaging all its staged lines saves a backup first.
        let (digest, refs) = lines(&r, &mut s, "diff.index", "src/lib.rs");
        let out = same(
            r.run(
                "index.unstage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": refs, "digest": digest }),
            )
            .unwrap(),
        );
        assert_eq!(out.backup.as_deref(), Some("refs/project/backup/index-1"));
        assert_eq!(out.status, status(&r, &mut s));

        // README.md is `MM`: the same for the whole file.
        let out = same(
            r.run("index.unstage", &mut s, json!({ "paths": ["README.md"] }))
                .unwrap(),
        );
        assert_eq!(out.backup.as_deref(), Some("refs/project/backup/index-2"));
        assert_eq!(out.status, status(&r, &mut s));
    }
}
