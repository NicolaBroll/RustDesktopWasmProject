//! Blame and history of one file on the [`CliBackend`] (`file.blame`, `file.log`, S34), per §5:
//!
//! - blame: `git --no-optional-locks -c core.fsmonitor=false -c blame.markIgnoredLines=false
//!   -c blame.markUnblamableLines=false blame --porcelain --no-textconv --no-root
//!   --ignore-revs-file= [<oid>] -- <path>`. `--ignore-revs-file=` (empty) clears the files of
//!   `blame.ignoreRevsFile`, so no commit is skipped whatever the configuration (a configured
//!   file that does not exist still makes git fail, as with a plain `git blame`); `--no-root`
//!   keeps the root commit a boundary whatever `blame.showRoot` is; `--no-textconv` gives the
//!   file's bytes and runs no configured converter. `git blame` does not accept
//!   `--end-of-options` (it reads it as a revision): only the resolved id reaches it, which is
//!   hexadecimal and cannot look like an option. The path goes after `--` and blame takes it
//!   literally (it is not a pathspec);
//! - log: `git --literal-pathspecs -c log.diffMerges=separate log <FILE_LOG_ARGS> (--follow
//!   --find-renames | --no-follow --no-renames) [--skip=<n>] --max-count=<n> --end-of-options <oid> -- <path>`: the explicit
//!   options make the result independent of `log.follow` and `diff.renames` (`copies`).
//!   With `--follow` git applies `--skip` before seeing the renames (the commits before a
//!   rename then vanish): the first `skip` commits are asked for and dropped here.
//!
//! Both run with [`Git::run_cancellable`](crate::command::Git): the job's token stops git.

use super::CliBackend;
use crate::command::{failure, os_from_bytes};
use project_domain::{FILE_LOG_ARGS, FileLogEntry, Oid, RepoPath, parse_blame, parse_file_log};
use project_engine::{
    BlameQuery, EngineError, FileBlame, FileLogQuery, JobContext, blame_binary, blame_is_binary,
    blame_missing_at, blame_missing_worktree, blame_no_commits,
};
use std::ffi::OsStr;
use std::process::Output;

impl CliBackend {
    /// `git <args> -- <path>`, cancellable with `job`; stdout, or git's error (with stderr).
    fn run_read(
        &self,
        command: &'static str,
        args: &[&str],
        path: &RepoPath,
        job: &JobContext,
    ) -> Result<Result<Vec<u8>, (Output, EngineError)>, EngineError> {
        if job.is_cancelled() {
            return Err(EngineError::Cancelled { command });
        }
        let path = os_from_bytes(path.as_bytes());
        let paths: [&OsStr; 1] = [&path];
        let (streamed, stderr) = self
            .git
            .run_cancellable(&self.workdir, args, &paths, &|| job.is_cancelled())?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled { command });
        }
        if streamed.status.success() {
            return Ok(Ok(streamed.stdout));
        }
        let output = Output {
            status: streamed.status,
            stdout: Vec::new(),
            stderr: stderr.into_bytes(),
        };
        let error = failure(args, &output);
        Ok(Err((output, error)))
    }

    /// [`GitRead::blame`](project_engine::GitRead::blame).
    pub(super) fn blame_file(
        &self,
        query: &BlameQuery,
        job: &JobContext,
    ) -> Result<FileBlame, EngineError> {
        let path = &query.path;
        let rev = match &query.rev {
            Some(rev) => Some(
                self.resolve(rev)?
                    .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?,
            ),
            None => {
                if self.resolve("HEAD")?.is_none() {
                    return Err(blame_no_commits());
                }
                None
            }
        };
        let hex = rev.map(|id| id.to_string());
        let mut args = vec![
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "blame.markIgnoredLines=false",
            "-c",
            "blame.markUnblamableLines=false",
            "blame",
            "--porcelain",
            "--no-textconv",
            "--no-root",
            "--ignore-revs-file=",
        ];
        args.extend(hex.as_deref());
        args.push("--");
        let out = match self.run_read("file.blame", &args, path, job)? {
            Ok(out) => out,
            Err((output, error)) => {
                return Err(blame_error(&output, path, rev.as_ref()).unwrap_or(error));
            }
        };
        let blame = parse_blame(&out)?;
        if blame_is_binary(&blame) {
            return Err(blame_binary(path));
        }
        Ok(FileBlame {
            blame,
            path: path.clone(),
            rev,
        })
    }

    /// [`GitRead::file_log`](project_engine::GitRead::file_log).
    pub(super) fn log_file(
        &self,
        query: &FileLogQuery,
        job: &JobContext,
    ) -> Result<Vec<FileLogEntry>, EngineError> {
        let start = match &query.rev {
            Some(rev) => self
                .resolve(rev)?
                .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?,
            None => match self.resolve("HEAD")? {
                Some(id) => id,
                // A branch without commits: no history.
                None => return Ok(Vec::new()),
            },
        };
        if query.limit == Some(0) {
            return Ok(Vec::new());
        }
        // With `--follow` the skip is done here (see the module documentation).
        let (git_skip, our_skip) = if query.follow {
            (0, query.skip)
        } else {
            (query.skip, 0)
        };
        let skip = format!("--skip={git_skip}");
        let max_count = query
            .limit
            .map(|n| format!("--max-count={}", n.saturating_add(our_skip)));
        let hex = start.to_string();
        // `-m` of `FILE_LOG_ARGS` with one record per parent whatever `log.diffMerges` is.
        let mut args = vec![
            "--literal-pathspecs",
            "-c",
            "log.diffMerges=separate",
            "log",
        ];
        args.extend_from_slice(FILE_LOG_ARGS);
        if query.follow {
            args.extend(["--follow", "--find-renames"]);
        } else {
            args.extend(["--no-follow", "--no-renames", &skip]);
        }
        args.extend(max_count.as_deref());
        args.extend(["--end-of-options", &hex, "--"]);
        let out = self
            .run_read("file.log", &args, &query.path, job)?
            .map_err(|(_, error)| error)?;
        let mut entries = parse_file_log(&out)?;
        entries.drain(..our_skip.min(entries.len()));
        Ok(entries)
    }
}

/// The engine error for a blame git refused because the path is not a file there:
/// `no such path '<path>' in HEAD` (worktree: missing from HEAD and from the index),
/// `no such path <path> in <oid>`, `Cannot lstat '<path>'` (missing from the worktree).
fn blame_error(output: &Output, path: &RepoPath, rev: Option<&Oid>) -> Option<EngineError> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let missing = stderr.lines().any(|line| {
        line.starts_with("fatal: no such path ") || line.starts_with("fatal: Cannot lstat ")
    });
    if !missing {
        return None;
    }
    Some(match rev {
        Some(rev) => blame_missing_at(path, rev),
        None => blame_missing_worktree(path),
    })
}
