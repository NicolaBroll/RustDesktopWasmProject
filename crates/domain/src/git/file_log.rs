//! Parser for the history of one file: `git log` with [`FILE_LOG_ARGS`] (the [`LOG_FORMAT`]
//! records followed by the `--name-status -z` entries).
//!
//! # Format
//!
//! Behavior verified with git 2.30, 2.43 and 2.56:
//!
//! - each commit is a [`LOG_FORMAT`] record of [`LOG_FIELDS`] NUL-terminated fields, as in
//!   [`super::parse_log`];
//! - if the commit has a diff, a `\n` follows the record, then one entry per changed path: the status
//!   and the path, each terminated by NUL (`M\0path\0`); a rename or a copy has the score and two
//!   paths, source then destination (`R100\0old\0new\0`). With `-z` paths are never quoted;
//! - a commit without a diff (a merge without `-m`) has no `\n` and no entries: the next record
//!   starts right after it. [`FILE_LOG_ARGS`] uses `-m` (`--diff-merges=first-parent` exists
//!   only from git 2.31), so a merge has one record per parent, the first parent first: git
//!   shows a merge only if the file differs from every parent (as `git log -- <path>`), so each
//!   of its records has an entry and the parser keeps the first one;
//! - `--root` shows the root commit as an addition even with `log.showRoot=false` (otherwise
//!   git would hide the commit altogether); `--no-relative` ignores `diff.relative`;
//! - with `--follow` (a single path) the rename appears as `R<score>` with the old name, and the
//!   older commits have the old name as their path.
//!
//! [`LOG_FORMAT`]: super::LOG_FORMAT
//! [`LOG_FIELDS`]: super::LOG_FIELDS

use super::diff_tree::parse_change_status;
use super::log::{log_format, parse_record};
use super::{Change, Commit, LOG_FIELDS, RepoPath};
use crate::DomainError;
use serde::{Deserialize, Serialize};

/// Mandatory arguments of `git log` to produce the input of [`parse_file_log`]: those of
/// [`super::LOG_ARGS`] (format included) plus
///
/// - `--name-status`: the status and path of the file in each commit;
/// - `-m`: merges with their entries, one record per parent (the first one is kept). The caller
///   passes `-c log.diffMerges=separate`, so that from git 2.33 `-m` does not follow the
///   user's `log.diffMerges`;
/// - `--root`: the root commit with its `A` entry whatever `log.showRoot` is;
/// - `--no-relative`: paths from the root of the repo whatever `diff.relative` is.
///
/// The caller adds `--follow` if wanted, the limits (`--skip`, `-n`), `--end-of-options <oid>`
/// and `-- <path>`; options that change the output must not be added (`--stat`, `-p`, `-m`,
/// `--graph`...).
pub const FILE_LOG_ARGS: &[&str] = &[
    "-z",
    "--date=raw",
    "--encoding=UTF-8",
    "--no-show-signature",
    "--no-color",
    concat!("--format=tformat:", log_format!()),
    "--name-status",
    "-m",
    "--root",
    "--no-relative",
];

/// A commit that changes the file, with the file's name in that commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLogEntry {
    pub commit: Commit,
    /// Name of the file in this commit (for a deletion, the name it had).
    pub path: RepoPath,
    /// Only for a rename or a copy: the name in the parent.
    pub old_path: Option<RepoPath>,
    /// Only [`Change::Added`], [`Change::Deleted`], [`Change::Modified`],
    /// [`Change::TypeChanged`], [`Change::Renamed`] or [`Change::Copied`].
    pub status: Change,
}

/// Reads the output of `git log` produced with [`FILE_LOG_ARGS`], newest first as git wrote it.
/// Of the records of a merge (one per parent) only the first, against the first parent, is kept.
///
/// An empty input gives an empty vector. An input that does not end with NUL, an incomplete
/// record, a commit without a `--name-status` entry or with more than one, an unknown status or
/// invalid fields give [`DomainError::InvalidGitOutput`] with the commit number (from 1).
pub fn parse_file_log(input: &[u8]) -> Result<Vec<FileLogEntry>, DomainError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = input.strip_suffix(b"\0") else {
        return Err(invalid(
            "the output does not end with NUL: it is truncated or was not produced with `-z`"
                .to_owned(),
        ));
    };
    let tokens: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    let mut rest = &tokens[..];
    let mut entries = Vec::new();
    while !rest.is_empty() {
        let n = entries.len() + 1;
        let err = |reason: String| invalid(format!("commit {n}: {reason}"));
        let Some((record, after)) = rest.split_first_chunk::<LOG_FIELDS>() else {
            return Err(err(format!(
                "{} fields instead of {LOG_FIELDS}: the format is not that of `LOG_FORMAT`",
                rest.len()
            )));
        };
        let commit = parse_record(n, record)?;
        rest = after;
        // The entries start with `\n` glued to the first status.
        let Some(status) = rest.first().and_then(|t| t.strip_prefix(b"\n")) else {
            return Err(err(format!(
                "commit {} has no `--name-status` entry: the arguments are not those of \
                 `FILE_LOG_ARGS` (`-m`, `--root`)",
                commit.id
            )));
        };
        let (change, _score) = parse_change_status(status).map_err(err)?;
        let two_paths = matches!(change, Change::Renamed | Change::Copied);
        let count = if two_paths { 3 } else { 2 };
        let Some(entry) = rest.get(..count) else {
            return Err(err("incomplete `--name-status` entry".to_owned()));
        };
        if entry[1..].iter().any(|p| p.is_empty()) {
            return Err(err("empty path in the `--name-status` entry".to_owned()));
        }
        rest = &rest[count..];
        // A further entry (status, not a commit id) means more than one path.
        if let Some(next) = rest.first()
            && parse_change_status(next).is_ok()
        {
            return Err(err(
                "more than one `--name-status` entry: the log must be limited to one path"
                    .to_owned(),
            ));
        }
        let (path, old_path) = if two_paths {
            (RepoPath::from(entry[2]), Some(RepoPath::from(entry[1])))
        } else {
            (RepoPath::from(entry[1]), None)
        };
        // The other records of a merge (against the second parent and after) follow the first.
        if entries
            .last()
            .is_some_and(|last: &FileLogEntry| last.commit.id == commit.id)
        {
            continue;
        }
        entries.push(FileLogEntry {
            commit,
            path,
            old_path,
            status: change,
        });
    }
    Ok(entries)
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "log",
        reason,
    }
}
