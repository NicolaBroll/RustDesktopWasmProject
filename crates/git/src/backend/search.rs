//! `repo.search` (S48): which commits of the log match a literal text.
//!
//! git ANDs `--author` with `--grep` (and splits nothing else), so each field is a run of its
//! own and the ids are merged here, in the order of one unfiltered run:
//!
//! - order: `git log --date-order --format=tformat:%H <range> --`, the same range as
//!   `repo.log` ([`CliBackend::log_range`]); the id prefix is checked here on these ids;
//! - message: `git log --fixed-strings --regexp-ignore-case --encoding=UTF-8 --grep=<text>
//!   <range> --` (the text only as the value of the option: it cannot become an option);
//! - author: the same with `--author=<text>` and `--no-mailmap` (name and email as written in
//!   the commit, as `repo.log` shows them);
//! - path: `git --literal-pathspecs log --full-history --no-follow <range> -- <path>`
//!   (`--full-history`: also the commits of side branches and the merges that bring a change in;
//!   `--no-follow`: independent of `log.follow`, no rename following).
//!
//! Only `%H` comes out, so non-UTF-8 messages and names cannot break the parsing. Every run
//! is cancellable with the job's token.

use super::CliBackend;
use crate::command::{failure, os_from_bytes};
use project_domain::Oid;
use project_engine::{EngineError, JobContext, SearchField, SearchQuery};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::process::Output;

/// Options of every run: only the ids, nothing else from the user's configuration.
const IDS_ARGS: &[&str] = &["--format=tformat:%H", "--no-show-signature", "--no-color"];

impl CliBackend {
    pub(super) fn search_log(
        &self,
        query: &SearchQuery,
        job: &JobContext,
    ) -> Result<Vec<Oid>, EngineError> {
        let Some(range) = self.log_range(&query.range())? else {
            return Ok(Vec::new());
        };
        let order = self.log_ids(&["--date-order"], &range, None, job)?;
        let mut matched: HashSet<Oid> = order
            .iter()
            .filter(|id| query.id_matches(id))
            .copied()
            .collect();
        let literal = [
            "--fixed-strings",
            "--regexp-ignore-case",
            "--encoding=UTF-8",
        ];
        if query.has(SearchField::Message) {
            let grep = format!("--grep={}", query.text);
            let args = [&literal[..], &[grep.as_str()]].concat();
            matched.extend(self.log_ids(&args, &range, None, job)?);
        }
        if query.has(SearchField::Author) {
            let author = format!("--author={}", query.text);
            let args = [&literal[..], &["--no-mailmap", author.as_str()]].concat();
            matched.extend(self.log_ids(&args, &range, None, job)?);
        }
        if let Some(path) = &query.path {
            let path = os_from_bytes(path.as_bytes());
            let touching: HashSet<Oid> = self
                .log_ids(&["--full-history", "--no-follow"], &range, Some(&path), job)?
                .into_iter()
                .collect();
            matched.retain(|id| touching.contains(id));
        }
        Ok(order
            .into_iter()
            .filter(|id| matched.contains(id))
            .collect())
    }

    /// The ids `git log <options> <range> -- [<path>]` prints, one per line.
    fn log_ids(
        &self,
        options: &[&str],
        range: &[String],
        path: Option<&OsStr>,
        job: &JobContext,
    ) -> Result<Vec<Oid>, EngineError> {
        const COMMAND: &str = "repo.search";
        if job.is_cancelled() {
            return Err(EngineError::Cancelled { command: COMMAND });
        }
        let mut args = vec!["--literal-pathspecs", "log"];
        args.extend_from_slice(IDS_ARGS);
        args.extend_from_slice(options);
        args.extend(range.iter().map(String::as_str));
        args.push("--");
        let paths: Vec<&OsStr> = path.into_iter().collect();
        let (streamed, stderr) = self
            .git
            .run_cancellable(&self.workdir, &args, &paths, &|| job.is_cancelled())?;
        if streamed.cancelled {
            return Err(EngineError::Cancelled { command: COMMAND });
        }
        if !streamed.status.success() {
            let output = Output {
                status: streamed.status,
                stdout: Vec::new(),
                stderr: stderr.into_bytes(),
            };
            return Err(failure(&args, &output));
        }
        let ids = streamed
            .stdout
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .map(Oid::from_hex)
            .collect::<Result<_, _>>()?;
        Ok(ids)
    }
}
