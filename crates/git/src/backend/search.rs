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
//! Content (S61) is one run of its own, in the order git gives:
//! `git log --date-order --no-renames --no-textconv (-S<text> | -G<text>)
//! [--regexp-ignore-case] [--full-history --no-follow] [--max-count=<max>] <range> -- [<path>]`
//! (the text glued to the option, so it cannot become one; `--max-count` counts the matches,
//! while `--skip` would count the walked commits, so the page is cut here). An invalid regex
//! (`fatal: invalid regex`) → `InvalidParams` with git's text.
//!
//! Only `%H` comes out, so non-UTF-8 messages and names cannot break the parsing. Every run
//! is cancellable with the job's token: the git process (group) is killed.

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
        if query.is_content() {
            return self.search_content(query, &range, job);
        }
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

    /// The commits whose changes contain the text (`-S`/`-G`), at most `query.max`.
    fn search_content(
        &self,
        query: &SearchQuery,
        range: &[String],
        job: &JobContext,
    ) -> Result<Vec<Oid>, EngineError> {
        let pickaxe = if query.regex {
            format!("-G{}", query.text)
        } else {
            format!("-S{}", query.text)
        };
        let max = query.max.map(|n| format!("--max-count={n}"));
        let mut options = vec![
            "--date-order",
            "--no-renames",
            "--no-textconv",
            pickaxe.as_str(),
        ];
        if query.ignore_case {
            options.push("--regexp-ignore-case");
        }
        if query.path.is_some() {
            options.extend(["--full-history", "--no-follow"]);
        }
        options.extend(max.as_deref());
        let path = query.path.as_ref().map(|p| os_from_bytes(p.as_bytes()));
        match self.log_ids(&options, range, path.as_deref(), job) {
            Err(EngineError::Git { stderr, .. })
                if query.regex && stderr.contains("invalid regex") =>
            {
                Err(EngineError::InvalidParams {
                    id: "repo.search",
                    reason: msg!(
                        "engine-search-bad-regex",
                        stderr = stderr
                            .trim()
                            .trim_start_matches("fatal: ")
                            .trim_start_matches("invalid regex: ")
                    ),
                })
            }
            other => other,
        }
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
