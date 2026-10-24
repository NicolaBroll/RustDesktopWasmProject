//! Parser for the output of `git status --porcelain=v2 -z --branch --untracked-files=all`.
//!
//! # Format
//!
//! With `-z` every record is **terminated** by a NUL, the last included; paths are never
//! quoted or escaped, so they may contain spaces, newlines and non-UTF-8 bytes. First
//! come the headers (with `--branch`), then one entry per file:
//!
//! ```text
//! # branch.oid <commit> | (initial)
//! # branch.head <branch> | (detached)
//! # branch.upstream <upstream>            only if there is an upstream
//! # branch.ab +<ahead> -<behind>          only if the upstream still exists
//! 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
//! 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path> \0 <origPath>
//! u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
//! ? <path>
//! ! <path>                                only with `--ignored`
//! ```
//!
//! Behavior verified with git 2.43:
//!
//! - in a repo without commits `branch.oid` is `(initial)`; with a detached HEAD `branch.head` is
//!   `(detached)` and `branch.oid` is the commit;
//! - `branch.upstream` is the **short** name (e.g. `origin/main`); if the upstream is configured but the
//!   ref no longer exists `branch.ab` is missing;
//! - in the `2` record the original path is a field of its own after a NUL; `<X><score>` is `R` or `C`
//!   followed by the similarity as a percentage (copies appear only with
//!   `status.renames=copies`);
//! - `<sub>` is `N...` for normal files and `S<c><m><u>` for submodules;
//! - modes are six-digit octal, ids are full (zeros if the object does not exist);
//! - unknown headers (e.g. `# stash <n>` with `--show-stash`) must be ignored, as
//!   git's documentation requires.
//!
//! The operation in progress (merge, rebase, ...) does not appear in the output: [`parse_status`] leaves
//! [`Status::operation`] as `None` and the backend fills it by reading `.git`.

use super::log::parse_digits;
use super::{
    Change, Conflict, EntryKind, Head, Oid, RepoPath, Status, StatusEntry, SubmoduleStatus,
    TrackedInfo, UnmergedInfo, Upstream,
};
use crate::DomainError;

/// Mandatory arguments of `git status` to produce the input of [`parse_status`]. The caller
/// may add `--ignored` ([`EntryKind::Ignored`] entries) or paths after `--`; options that change
/// the format must not be added, such as `--short` or `--porcelain=v1`.
///
/// - `--porcelain=v2`: stable format, with modes, ids and the original path of renames;
/// - `-z`: NUL-terminated records and paths without quotes or escapes;
/// - `--branch`: headers with HEAD, upstream and ahead/behind;
/// - `--untracked-files=all`: one record for every untracked file, also inside new
///   folders (without it, git shows only the folder).
pub const STATUS_ARGS: &[&str] = &["--porcelain=v2", "-z", "--branch", "--untracked-files=all"];

/// Reads the output of `git status` produced with [`STATUS_ARGS`].
///
/// The entries stay in git's order. [`Status::operation`] is always `None`: the output does not
/// contain it. An input that does not end with NUL, without the `branch.oid` and
/// `branch.head` headers, with an unknown record kind or with invalid fields gives
/// [`DomainError::InvalidGitOutput`], with the number of the record (from 1) involved.
pub fn parse_status(input: &[u8]) -> Result<Status, DomainError> {
    let body = if input.is_empty() {
        input
    } else {
        input.strip_suffix(b"\0").ok_or_else(|| {
            invalid(
                "the output does not end with NUL: it is truncated or was not produced with `-z`"
                    .to_owned(),
            )
        })?
    };
    let mut headers = Headers::default();
    let mut entries = Vec::new();
    let mut tokens = body.split(|&b| b == 0);
    let mut n = 0;
    // With an empty `body` `split` gives a single empty token: no records.
    while let Some(record) = tokens.next().filter(|_| !body.is_empty()) {
        n += 1;
        let err = |reason: String| invalid(format!("record {n}: {reason}"));
        let (kind, rest) = match record {
            [kind, b' ', rest @ ..] => (*kind, rest),
            [] => return Err(err("empty record".to_owned())),
            _ => {
                return Err(err(format!(
                    "`{}` is not a valid record (expected `<type> <fields>`)",
                    show(record)
                )));
            }
        };
        match kind {
            b'#' => headers.read(rest).map_err(err)?,
            b'1' => entries.push(parse_ordinary(rest).map_err(err)?),
            b'2' => {
                let original = tokens.next().ok_or_else(|| {
                    err("missing original path of the rename after the NUL".to_owned())
                })?;
                entries.push(parse_renamed(rest, original).map_err(err)?);
            }
            b'u' => entries.push(parse_unmerged(rest).map_err(err)?),
            b'?' => {
                entries.push(simple(rest, Change::Untracked, EntryKind::Untracked).map_err(err)?)
            }
            b'!' => entries.push(simple(rest, Change::Ignored, EntryKind::Ignored).map_err(err)?),
            other => {
                return Err(err(format!(
                    "unknown record type `{}` (expected `#`, `1`, `2`, `u`, `?`, `!`)",
                    char::from(other).escape_default()
                )));
            }
        }
    }
    let (head, upstream) = headers.finish()?;
    Ok(Status {
        head,
        upstream,
        entries,
        operation: None,
    })
}

/// Headers read so far.
#[derive(Default)]
struct Headers {
    /// `Some(None)` per `(initial)`.
    oid: Option<Option<Oid>>,
    /// `Some(None)` per `(detached)`.
    head: Option<Option<String>>,
    upstream: Option<String>,
    ab: Option<(u32, u32)>,
}

impl Headers {
    fn read(&mut self, line: &[u8]) -> Result<(), String> {
        let (key, value) = match line.iter().position(|&b| b == b' ') {
            Some(i) => (&line[..i], &line[i + 1..]),
            None => (line, &line[line.len()..]),
        };
        let once = |present: bool| {
            if present {
                Err(format!("repeated header `{}`", show(key)))
            } else {
                Ok(())
            }
        };
        match key {
            b"branch.oid" => {
                once(self.oid.is_some())?;
                self.oid = Some(if value == b"(initial)" {
                    None
                } else {
                    Some(oid(value, "branch.oid")?)
                });
            }
            b"branch.head" => {
                once(self.head.is_some())?;
                if value.is_empty() {
                    return Err("`branch.head` without branch name".to_owned());
                }
                self.head = Some((value != b"(detached)").then(|| show(value)));
            }
            b"branch.upstream" => {
                once(self.upstream.is_some())?;
                if value.is_empty() {
                    return Err("`branch.upstream` without name".to_owned());
                }
                self.upstream = Some(show(value));
            }
            b"branch.ab" => {
                once(self.ab.is_some())?;
                self.ab = Some(parse_ab(value)?);
            }
            // Future or optional headers (e.g. `stash`): to be ignored.
            _ => {}
        }
        Ok(())
    }

    fn finish(self) -> Result<(Head, Option<Upstream>), DomainError> {
        let missing = |key: &str| {
            invalid(format!(
                "missing header `# {key}`: the output was not produced with `--branch`"
            ))
        };
        let oid = self.oid.ok_or_else(|| missing("branch.oid"))?;
        let head = match self.head.ok_or_else(|| missing("branch.head"))? {
            Some(name) => Head::Branch { name, target: oid },
            None => Head::Detached(oid.ok_or_else(|| {
                invalid("detached HEAD (`(detached)`) but `branch.oid` is `(initial)`".to_owned())
            })?),
        };
        let upstream = match (self.upstream, self.ab) {
            (Some(name), ab) => Some(Upstream {
                name,
                ahead: ab.map(|(a, _)| a),
                behind: ab.map(|(_, b)| b),
            }),
            (None, Some(_)) => {
                return Err(invalid(
                    "`branch.ab` present without `branch.upstream`".to_owned(),
                ));
            }
            (None, None) => None,
        };
        Ok((head, upstream))
    }
}

/// `+<ahead> -<behind>`.
fn parse_ab(value: &[u8]) -> Result<(u32, u32), String> {
    let bad = || {
        format!(
            "invalid `branch.ab {}` (expected `+<ahead> -<behind>`)",
            show(value)
        )
    };
    let mut parts = value.split(|&b| b == b' ');
    let (Some(a), Some(b), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(bad());
    };
    let num = |s: &[u8], sign: u8| {
        s.strip_prefix(&[sign])
            .and_then(parse_digits)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(bad)
    };
    Ok((num(a, b'+')?, num(b, b'-')?))
}

/// Splits `rest` into `n` fields separated by a space; the last (the path) takes all the
/// rest, spaces included.
fn split_fields<'a>(rest: &'a [u8], n: usize, what: &str) -> Result<Vec<&'a [u8]>, String> {
    let fields: Vec<&[u8]> = rest.splitn(n, |&b| b == b' ').collect();
    if fields.len() != n {
        return Err(format!(
            "{what} with {} fields instead of {n}",
            fields.len()
        ));
    }
    Ok(fields)
}

fn parse_ordinary(rest: &[u8]) -> Result<StatusEntry, String> {
    let f = split_fields(rest, 8, "entry `1`")?;
    let (index, worktree) = parse_xy(f[0])?;
    Ok(StatusEntry {
        path: path(f[7])?,
        index,
        worktree,
        kind: EntryKind::Ordinary(tracked_info(&f[1..7])?),
    })
}

fn parse_renamed(rest: &[u8], original: &[u8]) -> Result<StatusEntry, String> {
    let f = split_fields(rest, 9, "entry `2`")?;
    let (index, worktree) = parse_xy(f[0])?;
    let info = tracked_info(&f[1..7])?;
    let (copy, score) = match f[7] {
        [b'R', score @ ..] => (false, score),
        [b'C', score @ ..] => (true, score),
        other => {
            return Err(format!(
                "`{}` is not a valid rename score (expected `R<n>` or `C<n>`)",
                show(other)
            ));
        }
    };
    let score = parse_digits(score)
        .and_then(|v| u8::try_from(v).ok())
        .filter(|&v| v <= 100)
        .ok_or_else(|| {
            format!(
                "`{}`: the similarity must be a number from 0 to 100",
                show(f[7])
            )
        })?;
    let path = path(f[8])?;
    let original = self::path(original).map_err(|e| format!("original path: {e}"))?;
    let kind = if copy {
        EntryKind::Copied {
            info,
            original,
            score,
        }
    } else {
        EntryKind::Renamed {
            info,
            original,
            score,
        }
    };
    Ok(StatusEntry {
        path,
        index,
        worktree,
        kind,
    })
}

fn parse_unmerged(rest: &[u8]) -> Result<StatusEntry, String> {
    let f = split_fields(rest, 10, "entry `u`")?;
    let (index, worktree) = parse_xy(f[0])?;
    let conflict = match f[0] {
        b"DD" => Conflict::BothDeleted,
        b"AU" => Conflict::AddedByUs,
        b"UD" => Conflict::DeletedByThem,
        b"UA" => Conflict::AddedByThem,
        b"DU" => Conflict::DeletedByUs,
        b"AA" => Conflict::BothAdded,
        b"UU" => Conflict::BothModified,
        other => {
            return Err(format!(
                "`{}` is not a conflict status (expected DD, AU, UD, UA, DU, AA, UU)",
                show(other)
            ));
        }
    };
    Ok(StatusEntry {
        path: path(f[9])?,
        index,
        worktree,
        kind: EntryKind::Unmerged(UnmergedInfo {
            conflict,
            submodule: parse_sub(f[1])?,
            mode_base: mode(f[2])?,
            mode_ours: mode(f[3])?,
            mode_theirs: mode(f[4])?,
            mode_worktree: mode(f[5])?,
            oid_base: oid(f[6], "base id")?,
            oid_ours: oid(f[7], "“ours” id")?,
            oid_theirs: oid(f[8], "“theirs” id")?,
        }),
    })
}

fn simple(rest: &[u8], change: Change, kind: EntryKind) -> Result<StatusEntry, String> {
    Ok(StatusEntry {
        path: path(rest)?,
        index: change,
        worktree: change,
        kind,
    })
}

/// `<sub> <mH> <mI> <mW> <hH> <hI>`.
fn tracked_info(f: &[&[u8]]) -> Result<TrackedInfo, String> {
    Ok(TrackedInfo {
        submodule: parse_sub(f[0])?,
        mode_head: mode(f[1])?,
        mode_index: mode(f[2])?,
        mode_worktree: mode(f[3])?,
        oid_head: oid(f[4], "id in HEAD")?,
        oid_index: oid(f[5], "id in the index")?,
    })
}

fn parse_xy(xy: &[u8]) -> Result<(Change, Change), String> {
    let bad = || {
        format!(
            "`{}` is not a valid XY code (two letters among `.MTADRCU`)",
            show(xy)
        )
    };
    let change = |c: u8| {
        Some(match c {
            b'.' => Change::Unmodified,
            b'M' => Change::Modified,
            b'T' => Change::TypeChanged,
            b'A' => Change::Added,
            b'D' => Change::Deleted,
            b'R' => Change::Renamed,
            b'C' => Change::Copied,
            b'U' => Change::Unmerged,
            _ => return None,
        })
    };
    match xy {
        [x, y] => Ok((change(*x).ok_or_else(bad)?, change(*y).ok_or_else(bad)?)),
        _ => Err(bad()),
    }
}

/// `N...` or `S<c><m><u>`.
fn parse_sub(sub: &[u8]) -> Result<Option<SubmoduleStatus>, String> {
    let flag = |c: u8, yes: u8| match c {
        b'.' => Some(false),
        c if c == yes => Some(true),
        _ => None,
    };
    let parsed = match sub {
        b"N..." => return Ok(None),
        [b'S', c, m, u] => (|| {
            Some(SubmoduleStatus {
                commit_changed: flag(*c, b'C')?,
                modified: flag(*m, b'M')?,
                untracked: flag(*u, b'U')?,
            })
        })(),
        _ => None,
    };
    parsed.map(Some).ok_or_else(|| {
        format!(
            "`{}` is not a valid submodule status (expected `N...` or `S<C|.><M|.><U|.>`)",
            show(sub)
        )
    })
}

/// Modo in ottale, es. `100644`.
pub(super) fn mode(m: &[u8]) -> Result<u32, String> {
    let bad = || format!("`{}` is not a valid octal mode", show(m));
    if m.is_empty() || m.len() > 7 {
        return Err(bad());
    }
    m.iter().try_fold(0u32, |acc, &b| match b {
        b'0'..=b'7' => Ok(acc * 8 + u32::from(b - b'0')),
        _ => Err(bad()),
    })
}

pub(super) fn oid(hex: &[u8], what: &str) -> Result<Oid, String> {
    Oid::from_hex(hex).map_err(|e| match e {
        DomainError::InvalidOid { input, reason } => {
            format!("{what}: `{input}` is not a valid id ({reason})")
        }
        other => format!("{what}: {other}"),
    })
}

fn path(p: &[u8]) -> Result<RepoPath, String> {
    if p.is_empty() {
        return Err("empty path".to_owned());
    }
    Ok(RepoPath::from(p))
}

pub(super) fn show(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "status",
        reason,
    }
}
