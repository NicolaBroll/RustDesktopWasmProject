//! Parser for the output of `git blame --porcelain`.
//!
//! # Format
//!
//! Behavior verified with git 2.56. The output is a sequence of groups of consecutive lines
//! coming from the same commit:
//!
//! ```text
//! <id> <line in the original file> <line in the final file> [<lines in the group>]
//! author <name>                      \
//! author-mail <<email>>               |
//! author-time <seconds>               |
//! author-tz <+hhmm>                   |  only the first time the commit appears
//! committer ... (the same four)       |
//! summary <subject>                   |
//! boundary                            |  (only if the commit is a boundary, e.g. the root)
//! previous <id> <path>                |  (only if the commit has a parent with the file)
//! filename <path>                    /
//! \t<content of the line>
//! ```
//!
//! - the headers come only the first time a commit appears; the other lines of a group, and
//!   later groups of the same commit, have only the `<id> <orig> <final>` line followed by the
//!   content. git repeats `previous` and `filename` (without the other headers) for a commit
//!   that was blamed under more than one path;
//! - the content line starts with a TAB and is the exact bytes of the file (it may be empty or
//!   not UTF-8). git always ends it with `\n`, even when the file has no final newline, so this
//!   information is not in the output;
//! - `filename` and the path of `previous` are C-quoted if they contain `"`, `\`, control
//!   characters or (with `core.quotePath`, the default) non-ASCII bytes;
//! - the lines not yet committed (blame of the worktree) have the all-zero id, author `Not
//!   Committed Yet` and the current time: they become [`BlameLine::commit`] `None`;
//! - unknown headers (e.g. `ignored` and `unblamable` with `blame.markIgnoredLines`) are skipped.

use super::log::{parse_digits, parse_tz};
use super::quote::unquote;
use super::{ByteText, Oid, RepoPath, Signature};
use crate::DomainError;
use serde::{Deserialize, Serialize};

/// The blame of a file: who last changed each line.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Blame {
    /// The lines of the file in order: line `n` is `lines[n - 1]`.
    pub lines: Vec<BlameLine>,
    /// The commits the lines come from, in order of first appearance.
    pub commits: Vec<BlameCommit>,
}

/// A line of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameLine {
    /// Content without the final newline (exact bytes).
    pub text: ByteText,
    /// Index in [`Blame::commits`]; `None` for a line not yet committed.
    pub commit: Option<usize>,
    /// Number of the line (from 1) in the file as it was in that commit.
    pub orig_line: u32,
}

/// A commit that last changed some lines of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameCommit {
    pub id: Oid,
    pub author: Signature,
    pub committer: Signature,
    /// Subject of the message (`from_utf8_lossy`).
    pub summary: String,
    /// Name of the file in this commit (`filename`). A commit appears more than once in
    /// [`Blame::commits`] only if git blamed lines from it under different names.
    pub path: RepoPath,
    /// The parent and the file's name in it (`previous`): where to look for the older version of
    /// the lines. `None` if the commit added the file or is a boundary.
    pub previous: Option<(Oid, RepoPath)>,
    /// The commit is a boundary (`boundary`): a root commit (git's `blame.showRoot` is false by
    /// default) or the limit of a range; its lines may be older.
    pub boundary: bool,
}

/// Headers of a commit, being read.
#[derive(Default)]
struct Headers {
    author: SigParts,
    committer: SigParts,
    summary: Option<String>,
    previous: Option<(Oid, RepoPath)>,
    boundary: bool,
}

#[derive(Default)]
struct SigParts {
    name: Option<String>,
    email: Option<String>,
    time: Option<i64>,
    offset_min: Option<i32>,
}

impl SigParts {
    fn finish(self, who: &str) -> Result<Signature, String> {
        let missing = |what: &str| format!("missing header `{who}{what}`");
        Ok(Signature {
            name: self.name.ok_or_else(|| missing(""))?,
            email: self.email.ok_or_else(|| missing("-mail"))?,
            time: self.time.ok_or_else(|| missing("-time"))?,
            offset_min: self.offset_min.ok_or_else(|| missing("-tz"))?,
        })
    }
}

/// Reads the output of `git blame --porcelain`.
///
/// An empty input (empty file) gives an empty blame. A group line or header that cannot be read,
/// a commit without the mandatory headers, lines out of order or a missing content line give
/// [`DomainError::InvalidGitOutput`] with the number of the output line (from 1).
pub fn parse_blame(input: &[u8]) -> Result<Blame, DomainError> {
    let mut blame = Blame::default();
    // For each commit id, the indices in `blame.commits` (more than one if under several names).
    let mut known: Vec<(Oid, usize)> = Vec::new();
    let mut zero_seen = false;
    let body = input.strip_suffix(b"\n").unwrap_or(input);
    if body.is_empty() {
        return Ok(blame);
    }
    let mut rows = body.split(|&b| b == b'\n').enumerate();
    while let Some((i, row)) = rows.next() {
        let err = |line: usize, reason: String| invalid(format!("line {}: {reason}", line + 1));
        let (id, orig_line, final_line) = parse_group_line(row).map_err(|r| err(i, r))?;
        let expected = blame.lines.len() + 1;
        if usize::try_from(final_line).ok() != Some(expected) {
            return Err(err(
                i,
                format!(
                    "final line {final_line} instead of {expected}: the output is not in order"
                ),
            ));
        }
        // Headers until the content line.
        let mut headers = Headers::default();
        let mut filename = None;
        let mut any_header = false;
        let text = loop {
            let Some((j, row)) = rows.next() else {
                return Err(err(
                    i,
                    "missing the content line (starting with TAB)".to_owned(),
                ));
            };
            if let Some(text) = row.strip_prefix(b"\t") {
                break text;
            }
            any_header = true;
            read_header(row, &mut headers, &mut filename).map_err(|r| err(j, r))?;
        };
        let commit = if id == Oid::from_bytes([0; Oid::LEN]) {
            if !zero_seen && !any_header {
                return Err(err(i, "uncommitted lines without their headers".to_owned()));
            }
            zero_seen = true;
            None
        } else if let Some(name) = filename {
            if headers.summary.is_some() {
                let commit = BlameCommit {
                    id,
                    author: headers.author.finish("author").map_err(|r| err(i, r))?,
                    committer: headers
                        .committer
                        .finish("committer")
                        .map_err(|r| err(i, r))?,
                    summary: headers.summary.unwrap_or_default(),
                    path: name,
                    previous: headers.previous,
                    boundary: headers.boundary,
                };
                if known.iter().any(|&(k, _)| k == id) {
                    return Err(err(i, format!("headers of commit {id} repeated")));
                }
                blame.commits.push(commit);
                known.push((id, blame.commits.len() - 1));
                Some(blame.commits.len() - 1)
            } else {
                // Only `previous`/`filename`: the same commit under another name.
                let Some(&(_, first)) = known.iter().find(|&&(k, _)| k == id) else {
                    return Err(err(i, format!("commit {id} without its headers")));
                };
                match known
                    .iter()
                    .find(|&&(k, idx)| k == id && blame.commits[idx].path == name)
                {
                    Some(&(_, idx)) => Some(idx),
                    None => {
                        let mut commit = blame.commits[first].clone();
                        commit.path = name;
                        commit.previous = headers.previous;
                        blame.commits.push(commit);
                        known.push((id, blame.commits.len() - 1));
                        Some(blame.commits.len() - 1)
                    }
                }
            }
        } else if any_header {
            return Err(err(i, format!("commit {id} without the `filename` header")));
        } else {
            // The commit seen last under any name: git repeats `filename` when it changes.
            let Some(&(_, idx)) = known.iter().rev().find(|&&(k, _)| k == id) else {
                return Err(err(i, format!("commit {id} without its headers")));
            };
            Some(idx)
        };
        blame.lines.push(BlameLine {
            text: ByteText::from(text),
            commit,
            orig_line,
        });
    }
    Ok(blame)
}

/// `<id> <orig> <final> [<count>]`.
fn parse_group_line(row: &[u8]) -> Result<(Oid, u32, u32), String> {
    let f: Vec<&[u8]> = row.split(|&b| b == b' ').collect();
    let (hex, orig, fin) = match f[..] {
        [hex, orig, fin] | [hex, orig, fin, _] => (hex, orig, fin),
        _ => {
            return Err(format!(
                "`{}` is not a group line `<id> <orig> <final> [<count>]`",
                String::from_utf8_lossy(row)
            ));
        }
    };
    let id = Oid::from_hex(hex).map_err(|e| e.to_string())?;
    let number = |s: &[u8]| {
        parse_digits(s)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|&v| v > 0)
            .ok_or_else(|| {
                format!(
                    "`{}` is not a valid line number",
                    String::from_utf8_lossy(s)
                )
            })
    };
    if let [.., count] = f[..]
        && f.len() == 4
    {
        number(count)?;
    }
    Ok((id, number(orig)?, number(fin)?))
}

fn read_header(row: &[u8], h: &mut Headers, filename: &mut Option<RepoPath>) -> Result<(), String> {
    let (key, value) = match row.iter().position(|&b| b == b' ') {
        Some(p) => (&row[..p], &row[p + 1..]),
        None => (row, &b""[..]),
    };
    let text = || String::from_utf8_lossy(value).into_owned();
    let bad = |what: &str| format!("invalid `{}` header: {what}", String::from_utf8_lossy(key));
    let time = || {
        parse_digits(value)
            .and_then(|v| i64::try_from(v).ok())
            .ok_or_else(|| bad("the seconds must be a non-negative integer"))
    };
    let tz = || parse_tz(value).map_err(bad);
    let path = || {
        unquote(value)
            .map(RepoPath::from)
            .map_err(|r| bad(&r))
            .and_then(|p| {
                if p.as_bytes().is_empty() {
                    Err(bad("empty path"))
                } else {
                    Ok(p)
                }
            })
    };
    match key {
        b"author" => h.author.name = Some(text()),
        b"author-mail" => h.author.email = Some(email(value)),
        b"author-time" => h.author.time = Some(time()?),
        b"author-tz" => h.author.offset_min = Some(tz()?),
        b"committer" => h.committer.name = Some(text()),
        b"committer-mail" => h.committer.email = Some(email(value)),
        b"committer-time" => h.committer.time = Some(time()?),
        b"committer-tz" => h.committer.offset_min = Some(tz()?),
        b"summary" => h.summary = Some(text()),
        b"boundary" => h.boundary = true,
        b"previous" => {
            let Some(space) = value.iter().position(|&b| b == b' ') else {
                return Err(bad("expected `<id> <path>`"));
            };
            let id = Oid::from_hex(&value[..space]).map_err(|e| bad(&e.to_string()))?;
            let (_, rest) = value.split_at(space + 1);
            let p = unquote(rest).map_err(|r| bad(&r))?;
            if p.is_empty() {
                return Err(bad("empty path"));
            }
            h.previous = Some((id, RepoPath::from(p)));
        }
        b"filename" => *filename = Some(path()?),
        // Headers of newer versions or options (`ignored`, `unblamable`): not needed.
        _ => {}
    }
    Ok(())
}

/// `<email>` without the angle brackets (kept as they are if absent).
fn email(value: &[u8]) -> String {
    let inner = value
        .strip_prefix(b"<")
        .and_then(|v| v.strip_suffix(b">"))
        .unwrap_or(value);
    String::from_utf8_lossy(inner).into_owned()
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "blame",
        reason,
    }
}
