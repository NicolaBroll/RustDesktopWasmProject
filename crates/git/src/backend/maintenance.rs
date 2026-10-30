//! The commit-graph on the [`CliBackend`] (`repo.open`, `repo.optimize`, S36), per §5:
//!
//! - state: `git rev-parse --git-path objects` (the object directory, also in a linked
//!   worktree; `GIT_OBJECT_DIRECTORY` is cleared from git's environment), then the files
//! `info/commit-graph` and
//!   `info/commit-graphs/commit-graph-chain` there and in the alternates
//!   (`info/alternates`, followed up to [`MAX_ALTERNATE_DEPTH`] levels like git; relative
//!   entries are relative to the object directory that lists them; `#` comments and quoted
//!   entries are skipped). Git reads the first graph it finds in those directories. Then
//!   `git config --type=bool --get core.commitGraph` (unset = enabled) and
//!   `git rev-parse --is-shallow-repository` (git ignores the graph in a shallow repo).
//!   Replace refs (`refs/replace/`, unless `core.useReplaceRefs=false`) and a grafts file
//!   (`info/grafts` in the common git directory) also make git ignore the graph.
//!   `GIT_ALTERNATE_OBJECT_DIRECTORIES` is not followed. With `core.commitGraph=false` git
//!   neither reads nor writes the graph (`repo.optimize` refuses first);
//! - write: `git commit-graph write --reachable --progress`, streamed (progress to the job,
//!   cancelled with SIGTERM). Git writes `objects/info/commit-graph.lock` and renames it at the
//!   end: on SIGTERM it removes the lock, the old graph (if any) stays as it was. Only a SIGKILL
//!   (after `TERM_GRACE`, or from outside) can leave a stale `commit-graph.lock`: the next write
//!   then fails with git's text (`Unable to create '...commit-graph.lock': File exists`) and
//!   the repo keeps working with the previous graph or none. The lock is never removed here:
//!   it may belong to a git that is still running.

use super::CliBackend;
use crate::command::{Segment, failure, path_from_output};
use crate::progress::parse_progress;
use project_engine::{CommitGraphState, EngineError, JobContext};
use std::path::{Path, PathBuf};
use std::process::Output;

/// How many levels of alternates git follows (`read_info_alternates` in git stops at 5).
const MAX_ALTERNATE_DEPTH: usize = 5;

impl CliBackend {
    /// [`GitMaintenance::commit_graph`](project_engine::GitMaintenance::commit_graph).
    pub(super) fn read_commit_graph_state(&self) -> Result<CommitGraphState, EngineError> {
        let out = self.run(&["rev-parse", "--git-path", "objects"])?;
        let objects = self.workdir.join(path_from_output(&out));
        let present = graph_in(&objects, 0, &mut Vec::new());
        let args = ["config", "--type=bool", "--get", "core.commitGraph"];
        let out = self.git.output(Some(&self.workdir), &args)?;
        let enabled = if out.status.code() == Some(1) && out.stderr.is_empty() {
            true
        } else if out.status.success() {
            out.stdout.strip_suffix(b"\n").unwrap_or(&out.stdout) != b"false"
        } else {
            return Err(failure(&args, &out));
        };
        let out = self.run(&["rev-parse", "--is-shallow-repository"])?;
        let shallow = out.trim_ascii_end() == b"true";
        let replaced = self.replace_refs_used()? || self.grafts_file()?;
        Ok(CommitGraphState {
            present,
            enabled,
            shallow,
            replaced,
        })
    }

    /// There are replace refs and git uses them (`core.useReplaceRefs`, default on; git
    /// inherits `GIT_NO_REPLACE_OBJECTS`, which turns them off).
    fn replace_refs_used(&self) -> Result<bool, EngineError> {
        if std::env::var_os("GIT_NO_REPLACE_OBJECTS").is_some() {
            return Ok(false);
        }
        let args = ["config", "--type=bool", "--get", "core.useReplaceRefs"];
        let out = self.git.output(Some(&self.workdir), &args)?;
        if out.status.success() && out.stdout.trim_ascii_end() == b"false" {
            return Ok(false);
        }
        if !out.status.success() && !(out.status.code() == Some(1) && out.stderr.is_empty()) {
            return Err(failure(&args, &out));
        }
        let refs = self.run(&["for-each-ref", "--count=1", "--format=x", "refs/replace/"])?;
        Ok(!refs.is_empty())
    }

    /// A grafts file (`git rev-parse --git-path info/grafts`).
    fn grafts_file(&self) -> Result<bool, EngineError> {
        let out = self.run(&["rev-parse", "--git-path", "info/grafts"])?;
        Ok(self.workdir.join(path_from_output(&out)).is_file())
    }

    /// [`GitMaintenance::write_commit_graph`](project_engine::GitMaintenance::write_commit_graph).
    pub(super) fn write_reachable_commit_graph(&self, job: &JobContext) -> Result<(), EngineError> {
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "repo.optimize",
            });
        }
        let args = ["commit-graph", "write", "--reachable", "--progress"];
        let mut lines = Vec::new();
        let streamed = self.git.run_streaming(
            &self.workdir,
            &args,
            false,
            |Segment { text, newline }| match parse_progress(&text) {
                Some(progress) => job.report(&progress),
                None if newline && !text.trim().is_empty() => {
                    lines.push(text.trim_end().to_owned())
                }
                None => {}
            },
            &|| job.is_cancelled(),
        )?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled {
                command: "repo.optimize",
            });
        }
        if !streamed.status.success() {
            return Err(failure(
                &args,
                &Output {
                    status: streamed.status,
                    stdout: Vec::new(),
                    stderr: lines.join("\n").into_bytes(),
                },
            ));
        }
        Ok(())
    }
}

/// A commit-graph in the object directory `objects` or in one of its alternates (`depth`
/// levels already followed; `seen` avoids loops).
fn graph_in(objects: &Path, depth: usize, seen: &mut Vec<PathBuf>) -> bool {
    let key = objects
        .canonicalize()
        .unwrap_or_else(|_| objects.to_owned());
    if seen.contains(&key) {
        return false;
    }
    seen.push(key);
    let info = objects.join("info");
    if info.join("commit-graph").is_file()
        || info
            .join("commit-graphs")
            .join("commit-graph-chain")
            .is_file()
    {
        return true;
    }
    if depth >= MAX_ALTERNATE_DEPTH {
        return false;
    }
    let Ok(list) = std::fs::read(info.join("alternates")) else {
        return false;
    };
    list.split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
        .filter(|line| !line.is_empty() && !line.starts_with(b"#") && !line.starts_with(b"\""))
        .any(|line| graph_in(&objects.join(path_from_output(line)), depth + 1, seen))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn graph_found_in_objects_chain_and_alternates() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a/objects");
        let b = tmp.path().join("b/objects");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        assert!(!graph_in(&a, 0, &mut Vec::new()));

        // Relative alternate (from `a/objects`), comments, blank lines and a loop back to `a`.
        write(
            &a.join("info/alternates"),
            b"# shared\n\n../../b/objects\r\n../../a/objects\n",
        );
        write(&b.join("info/alternates"), b"../../a/objects\n");
        assert!(!graph_in(&a, 0, &mut Vec::new()));

        write(&b.join("info/commit-graphs/commit-graph-chain"), b"");
        assert!(graph_in(&a, 0, &mut Vec::new()));
        std::fs::remove_file(b.join("info/commit-graphs/commit-graph-chain")).unwrap();

        write(&b.join("info/commit-graph"), b"");
        assert!(graph_in(&a, 0, &mut Vec::new()));
        // Too deep: not followed, like git.
        assert!(!graph_in(&a, MAX_ALTERNATE_DEPTH, &mut Vec::new()));
    }
}
