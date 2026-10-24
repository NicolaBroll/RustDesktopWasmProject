//! Parser for the output of `git log` in the fixed-field format separated by NULs.
//!
//! # Format
//!
//! Each commit is written as [`LOG_FIELDS`] fields, each **terminated** by a NUL:
//!
//! ```text
//! %H \0 %P \0 %an \0 %ae \0 %ad \0 %cn \0 %ce \0 %cd \0 %s \0 %b \0
//! ```
//!
//! Behavior verified with git 2.43:
//!
//! - in the format the first nine fields are separated by `%x00`; the tenth NUL is added by `-z`, which with
//!   `tformat:` (the default of `--format=`) **terminates** every record, the last included. The output
//!   therefore always ends with a NUL, and a log without commits is empty (zero bytes). With `format:`
//!   instead `-z` separates the records and the last has no final NUL: this is why [`LOG_ARGS`] uses
//!   `tformat:` explicitly;
//! - `%P` are the parents separated by a space; it is empty for a root commit;
//! - `%ad` and `%cd` with `--date=raw` are `<seconds> <+hhmm|-hhmm>`: unlike `%at`/`%ct`
//!   they also carry the time zone of the signer;
//! - `%s` is the first paragraph of the message with lines joined by a space (as in
//!   `git log --oneline`); `%b` is the rest, without the leading blank lines. `%b` is empty if the
//!   message has only the subject, otherwise it ends with `\n` (and with `--cleanup=verbatim` it may
//!   end with several `\n`): the parser strips the trailing newlines;
//! - a message, a name or an email cannot contain NUL, so counting the fields is
//!   unambiguous even when the body is empty or contains blank lines;
//! - `%an`/`%ae` are the values written in the commit, without `.mailmap` (those would be `%aN`/`%aE`);
//! - with `--encoding=UTF-8` git converts to UTF-8 the commits that declare another encoding
//!   (`encoding` header); commits without the header pass through as they are and may
//!   contain non-UTF-8 bytes, which here become `U+FFFD` with `from_utf8_lossy`.

use super::{Commit, Oid, Signature};
use crate::DomainError;

/// Number of fields per commit in the [`LOG_FORMAT`] format.
pub const LOG_FIELDS: usize = 10;

macro_rules! log_format {
    () => {
        "%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b"
    };
}

/// The format string read by [`parse_log`]. On its own it is not enough: it must be used with the other
/// arguments of [`LOG_ARGS`], which already contains it.
pub const LOG_FORMAT: &str = log_format!();

/// Mandatory arguments of `git log` to produce the input of [`parse_log`]. The caller adds
/// only the order (e.g. `--topo-order`), the limits (e.g. `--max-count`) and the range; options that
/// change the output must not be added, such as `--graph`, `--stat` or `--patch`.
///
/// - `-z`: NUL at the end of every record;
/// - `--date=raw`: `%ad`/`%cd` with seconds and time zone;
/// - `--encoding=UTF-8`: messages in UTF-8 whatever `i18n.logOutputEncoding` is;
/// - `--no-show-signature`: no gpg output between records with `log.showSignature`;
/// - `--no-color`: no color sequences;
/// - `--format=tformat:...`: [`LOG_FORMAT`] with the final NUL also on the last record.
pub const LOG_ARGS: &[&str] = &[
    "-z",
    "--date=raw",
    "--encoding=UTF-8",
    "--no-show-signature",
    "--no-color",
    concat!("--format=tformat:", log_format!()),
];

/// Names of the fields, for error messages.
const FIELD_NAMES: [&str; LOG_FIELDS] = [
    "id (%H)",
    "parents (%P)",
    "author name (%an)",
    "author email (%ae)",
    "author date (%ad)",
    "committer name (%cn)",
    "committer email (%ce)",
    "committer date (%cd)",
    "subject (%s)",
    "body (%b)",
];

/// Reads the output of `git log` produced with [`LOG_ARGS`], in the order git wrote it.
///
/// An empty input gives an empty vector. An input that does not end with NUL, that has a number of
/// fields not a multiple of [`LOG_FIELDS`] or with invalid ids, parents or dates gives
/// [`DomainError::InvalidGitOutput`], with the commit number (from 1) and the field involved.
pub fn parse_log(input: &[u8]) -> Result<Vec<Commit>, DomainError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = input.strip_suffix(b"\0") else {
        return Err(invalid(
            "the output does not end with NUL: it is truncated or was not produced with `-z` and \
             `--format=tformat:`"
                .to_owned(),
        ));
    };
    let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    let (records, rest) = fields.as_chunks::<LOG_FIELDS>();
    if !rest.is_empty() {
        return Err(invalid(format!(
            "commit {} has {} fields instead of {LOG_FIELDS}: the format is not that of \
             `LOG_FORMAT`",
            records.len() + 1,
            rest.len()
        )));
    }
    records
        .iter()
        .enumerate()
        .map(|(i, record)| parse_record(i + 1, record))
        .collect()
}

fn parse_record(n: usize, f: &[&[u8]; LOG_FIELDS]) -> Result<Commit, DomainError> {
    let field_err = |idx: usize, reason: String| {
        invalid(format!("commit {n}, field {}: {reason}", FIELD_NAMES[idx]))
    };
    let oid = |idx: usize, hex: &[u8]| {
        Oid::from_hex(hex).map_err(|e| match e {
            DomainError::InvalidOid { input, reason } => {
                field_err(idx, format!("`{input}` is not a valid id ({reason})"))
            }
            other => other,
        })
    };
    let date = |idx: usize| parse_raw_date(f[idx]).map_err(|reason| field_err(idx, reason));

    let id = oid(0, f[0])?;
    let parents = f[1]
        .split(|&b| b == b' ')
        .filter(|p| !p.is_empty())
        .map(|p| oid(1, p))
        .collect::<Result<Vec<_>, _>>()?;
    let (author_time, author_offset) = date(4)?;
    let (committer_time, committer_offset) = date(7)?;
    Ok(Commit {
        id,
        parents,
        author: Signature {
            name: text(f[2]),
            email: text(f[3]),
            time: author_time,
            offset_min: author_offset,
        },
        committer: Signature {
            name: text(f[5]),
            email: text(f[6]),
            time: committer_time,
            offset_min: committer_offset,
        },
        summary: text(f[8]),
        body: text(trim_trailing_newlines(f[9])),
    })
}

/// Reads a `--date=raw` date: `<seconds> <sign><hh><mm>`, e.g. `1700000000 -0530`.
/// Returns the seconds and the time zone in minutes (`-330` in the example).
fn parse_raw_date(raw: &[u8]) -> Result<(i64, i32), String> {
    let shown = || String::from_utf8_lossy(raw).into_owned();
    let bad = |what: &str| format!("invalid date `{}`: {what}", shown());
    let Some(space) = raw.iter().position(|&b| b == b' ') else {
        return Err(bad(
            "missing time zone (expected format `<seconds> <+hhmm>`)",
        ));
    };
    let (secs, tz) = (&raw[..space], &raw[space + 1..]);
    let time = parse_digits(secs)
        .and_then(|v| i64::try_from(v).ok())
        .ok_or_else(|| bad("the seconds must be a non-negative integer"))?;
    let sign = match tz.first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Err(bad("the time zone must start with `+` or `-`")),
    };
    let hhmm = &tz[1..];
    if hhmm.len() != 4 {
        return Err(bad("the time zone must have four digits (`+hhmm`)"));
    }
    let (hh, mm) = hhmm.split_at(2);
    let hours = parse_digits(hh).ok_or_else(|| bad("non-numeric time zone hours"))?;
    let minutes = parse_digits(mm).ok_or_else(|| bad("non-numeric time zone minutes"))?;
    // At most 99 * 60 + 99: always fits in an i32.
    let offset = i32::try_from(hours * 60 + minutes).unwrap_or(i32::MAX);
    Ok((time, sign * offset))
}

/// ASCII decimal digits (at least one) into a `u64`; `None` if empty, non-numeric or too large.
pub(super) fn parse_digits(s: &[u8]) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    s.iter().try_fold(0u64, |acc, &b| {
        if b.is_ascii_digit() {
            acc.checked_mul(10)?.checked_add(u64::from(b - b'0'))
        } else {
            None
        }
    })
}

fn trim_trailing_newlines(s: &[u8]) -> &[u8] {
    let end = s.iter().rposition(|&b| b != b'\n').map_or(0, |i| i + 1);
    &s[..end]
}

fn text(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "log",
        reason,
    }
}
