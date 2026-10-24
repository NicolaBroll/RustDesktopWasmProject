//! Parser for the output of `git diff-tree` in the `--raw -z` format: the files changed by a commit.
//!
//! # Format
//!
//! With [`DIFF_TREE_ARGS`] every changed file is a record of two or three fields, each
//! **terminated** by a NUL:
//!
//! ```text
//! :<old mode> <new mode> <old id> <new id> <status>[<score>] \0 <path> \0
//! :<old mode> <new mode> <old id> <new id> R<score> \0 <source> \0 <destination> \0
//! ```
//!
//! Behavior verified with git 2.43:
//!
//! - modes are six-digit octal, `000000` for the side that does not exist (added or
//!   deleted file); ids are full (with `--no-abbrev`), all zeros for the side that does not exist;
//! - the status is a letter: `A` added, `D` deleted, `M` modified (even only in mode, e.g.
//!   made executable), `T` type changed (e.g. from file to symbolic link), `R` renamed and `C`
//!   copied. `R` and `C` are followed by the similarity as a percentage and have two paths:
//!   first the source, then the destination. Copies appear only with `-C`, which
//!   [`DIFF_TREE_ARGS`] does not use; the parser reads them anyway;
//! - with `-z` paths are never quoted or escaped: they may contain spaces,
//!   newlines and non-UTF-8 bytes, and stay exact bytes in [`RepoPath`];
//! - `-r` descends into subfolders: one record per file, never per folder. A submodule is a
//!   record with mode `160000`;
//! - a commit that changes nothing (e.g. `--allow-empty`) gives an empty output;
//! - without `--no-commit-id`, with a single commit as argument, git first writes the commit id
//!   followed by a NUL: [`DIFF_TREE_ARGS`] excludes it.

use super::log::parse_digits;
use super::status::{mode, oid, show};
use super::{Change, Commit, Oid, RepoPath};
use crate::DomainError;
use serde::{Deserialize, Serialize};

/// Mandatory arguments of `git diff-tree` to produce the input of [`parse_diff_tree`].
///
/// The caller adds the two revisions to compare (`<parent> <commit>`), or
/// `--root <commit>` for a commit without parents (comparison with the empty tree); options that
/// change the format must not be added, such as `--name-status`, `--stat` or `--patch`.
///
/// - `-z`: NUL-terminated records and paths without quotes or escapes;
/// - `-r`: one record per file also inside subfolders;
/// - `--raw`: modes, ids and status for each file;
/// - `--no-abbrev`: full ids;
/// - `-M`: renames recognized (`diff-tree` does not read `diff.renames` from the configuration);
/// - `--no-commit-id`: no commit id at the head of the output.
pub const DIFF_TREE_ARGS: &[&str] = &["-z", "-r", "--raw", "--no-abbrev", "-M", "--no-commit-id"];

/// A file changed by a commit relative to the parent it is compared with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Path in the commit (for a deleted file, the one it had before; for a rename or
    /// a copy, the destination).
    pub path: RepoPath,
    /// The kind of change: only [`Change::Added`], [`Change::Deleted`], [`Change::Modified`],
    /// [`Change::TypeChanged`], [`Change::Renamed`] or [`Change::Copied`].
    pub change: Change,
    /// Only for renames and copies: the source path.
    pub original: Option<RepoPath>,
    /// Similarity as a percentage (0–100) for renames and copies; `None` for the other statuses.
    pub score: Option<u8>,
    /// Mode before the commit, in octal (e.g. `0o100644`, `0o100755`, `0o120000` for a link,
    /// `0o160000` for a submodule); `0` if the file did not exist.
    pub old_mode: u32,
    /// Mode after the commit; `0` if the file was deleted.
    pub new_mode: u32,
    /// Id of the content before the commit; all zeros if the file did not exist.
    pub old_oid: Oid,
    /// Id of the content after the commit; all zeros if the file was deleted.
    pub new_oid: Oid,
}

impl ChangedFile {
    /// Path before the commit: the source for renames and copies, otherwise [`Self::path`].
    pub fn old_path(&self) -> &RepoPath {
        self.original.as_ref().unwrap_or(&self.path)
    }

    /// True if the path is a submodule (mode `160000`) before or after the commit.
    pub fn is_submodule(&self) -> bool {
        const GITLINK: u32 = 0o160000;
        self.old_mode == GITLINK || self.new_mode == GITLINK
    }
}

/// Details of a commit: metadata and changed files (result of `commit.show`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitDetails {
    pub commit: Commit,
    /// The commit the files are compared with: the **first parent** (also for a merge,
    /// so the files are those brought by the merge into the branch it was made on); `None` for a
    /// root commit, compared with the empty tree.
    pub compared_to: Option<Oid>,
    /// Changed files, in git's order (by path, byte by byte).
    pub files: Vec<ChangedFile>,
}

/// Reads the output of `git diff-tree` produced with [`DIFF_TREE_ARGS`].
///
/// An empty input gives an empty vector. An input that does not end with NUL, with an
/// incomplete record, an unknown status or invalid fields gives [`DomainError::InvalidGitOutput`],
/// with the number of the record (from 1) involved.
pub fn parse_diff_tree(input: &[u8]) -> Result<Vec<ChangedFile>, DomainError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = input.strip_suffix(b"\0") else {
        return Err(invalid(
            "the output does not end with NUL: it is truncated or was not produced with `-z`"
                .to_owned(),
        ));
    };
    let mut tokens = body.split(|&b| b == 0);
    let mut files = Vec::new();
    while let Some(header) = tokens.next() {
        let n = files.len() + 1;
        let err = |reason: String| invalid(format!("record {n}: {reason}"));
        let mut next_path = |what: &str| match tokens.next() {
            Some([]) => Err(err(format!("empty {what}"))),
            Some(p) => Ok(RepoPath::from(p)),
            None => Err(err(format!("missing {what}"))),
        };
        let raw = parse_header(header).map_err(err)?;
        let first = next_path("path")?;
        let (path, original) = if matches!(raw.change, Change::Renamed | Change::Copied) {
            (next_path("destination path")?, Some(first))
        } else {
            (first, None)
        };
        files.push(ChangedFile {
            path,
            change: raw.change,
            original,
            score: raw.score,
            old_mode: raw.old_mode,
            new_mode: raw.new_mode,
            old_oid: raw.old_oid,
            new_oid: raw.new_oid,
        });
    }
    Ok(files)
}

struct RawHeader {
    old_mode: u32,
    new_mode: u32,
    old_oid: Oid,
    new_oid: Oid,
    change: Change,
    score: Option<u8>,
}

/// `:<mH> <mN> <hH> <hN> <status>`.
fn parse_header(header: &[u8]) -> Result<RawHeader, String> {
    let Some(rest) = header.strip_prefix(b":") else {
        return Err(format!(
            "`{}` is not a `--raw` record (it must start with `:`)",
            show(header)
        ));
    };
    let f: Vec<&[u8]> = rest.split(|&b| b == b' ').collect();
    let [old_mode, new_mode, old_oid, new_oid, status] = f[..] else {
        return Err(format!(
            "header `{}` with {} fields instead of 5",
            show(header),
            f.len()
        ));
    };
    let (change, score) = parse_change_status(status)?;
    Ok(RawHeader {
        old_mode: mode(old_mode)?,
        new_mode: mode(new_mode)?,
        old_oid: oid(old_oid, "old id")?,
        new_oid: oid(new_oid, "new id")?,
        change,
        score,
    })
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "diff-tree",
        reason,
    }
}

/// Reads a status of `--raw` or `--name-status` (`A`, `D`, `M`, `T`, `R<score>`, `C<score>`)
/// into the change and, for renames and copies, the similarity (0–100).
pub(super) fn parse_change_status(status: &[u8]) -> Result<(Change, Option<u8>), String> {
    let [letter, score @ ..] = status else {
        return Err("empty status".to_owned());
    };
    let change = match letter {
        b'A' => Change::Added,
        b'D' => Change::Deleted,
        b'M' => Change::Modified,
        b'T' => Change::TypeChanged,
        b'R' => Change::Renamed,
        b'C' => Change::Copied,
        _ => return Err(format!("unknown status `{}`", show(status))),
    };
    let needs_score = matches!(change, Change::Renamed | Change::Copied);
    let score = match (needs_score, score) {
        (false, []) => None,
        // `M` can have a dissimilarity only with `-B`, which we do not use.
        (false, _) => return Err(format!("unexpected status `{}`", show(status))),
        (true, digits) => Some(
            parse_digits(digits)
                .and_then(|v| u8::try_from(v).ok())
                .filter(|v| *v <= 100)
                .ok_or_else(|| {
                    format!(
                        "`{}` is not a valid score (expected `{}<0-100>`)",
                        show(status),
                        char::from(*letter)
                    )
                })?,
        ),
    };
    Ok((change, score))
}
