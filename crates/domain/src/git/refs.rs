//! Parser for the output of `git for-each-ref` in the NUL-terminated field format.
//!
//! # Format
//!
//! Each ref is written as [`REFS_FIELDS`] fields, each **terminated** by a NUL; after the
//! format git always adds a newline (`for-each-ref` has no `-z`):
//!
//! ```text
//! %(refname) \0 %(objectname) \0 %(*objectname) \0 %(upstream) \0 %(upstream:track,nobracket) \0 %(symref) \0 \n
//! ```
//!
//! The parser splits the input **only on NULs**: git's newline stays at the start of the first field
//! of the next record (and alone after the last NUL) and is removed. This way the format is not
//! ambiguous even if a field contains a newline. Behavior verified with git 2.43:
//!
//! - a ref name cannot contain NUL, newline or other control characters
//!   (`git check-ref-format` rejects them), but it may contain non-ASCII and even non-UTF-8 bytes;
//! - `%(upstream)` instead comes from `branch.<b>.merge` in the config and **can** contain a newline
//!   (config values cannot contain NUL): this is why it is not split on lines;
//! - a repo without refs gives an empty output (zero bytes);
//! - `%(*objectname)` is the object pointed to by an annotated tag, empty for other refs; git 2.43
//!   peels only one level, so for a tag of a tag it is the id of the inner tag;
//! - `%(upstream)` is the full name (e.g. `refs/remotes/origin/main`, or `refs/heads/main` for a
//!   local upstream with `branch.<b>.remote = .`), empty if the branch has none;
//! - `%(upstream:track,nobracket)` is empty (level, or without upstream), `ahead <n>`,
//!   `behind <m>`, `ahead <n>, behind <m>` or `gone` if the upstream's ref no longer exists;
//! - `%(symref)` is the ref pointed to by a symbolic ref (e.g. `refs/remotes/origin/HEAD`), empty
//!   for the others; a broken symbolic ref does not appear at all.

use std::collections::HashMap;

use super::log::parse_digits;
use super::{Oid, RefInfo, RefKind, Upstream};
use crate::DomainError;

/// Number of fields per ref in the [`REFS_FORMAT`] format.
pub const REFS_FIELDS: usize = 6;

macro_rules! refs_format {
    () => {
        "%(refname)%00%(objectname)%00%(*objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00"
    };
}

/// The format string read by [`parse_refs`]. It is already contained in [`REFS_ARGS`].
pub const REFS_FORMAT: &str = refs_format!();

/// Arguments of `git for-each-ref` to produce the input of [`parse_refs`]: the format
/// ([`REFS_FORMAT`]) and the patterns of the refs read (local branches, remotes, tags and `refs/stash`).
/// The caller may add filters such as `--merged` or `--points-at`, but not `--format`,
/// `--shell`/`--perl`/`--python`/`--tcl` (they change the format) nor other patterns: refs outside
/// these kinds would be ignored anyway.
///
/// The order is git's default, by ref name (byte by byte).
pub const REFS_ARGS: &[&str] = &[
    concat!("--format=", refs_format!()),
    "refs/heads/",
    "refs/remotes/",
    "refs/tags/",
    "refs/stash",
];

/// Names of the fields, for error messages.
const FIELD_NAMES: [&str; REFS_FIELDS] = [
    "name (%(refname))",
    "id (%(objectname))",
    "id pointed to by the tag (%(*objectname))",
    "upstream (%(upstream))",
    "ahead/behind (%(upstream:track,nobracket))",
    "symbolic ref (%(symref))",
];

/// Reads the output of `git for-each-ref` produced with [`REFS_ARGS`].
///
/// The refs stay in git's order. Rules:
///
/// - the kind comes from the prefix: `refs/heads/` → [`RefKind::LocalBranch`], `refs/remotes/` →
///   [`RefKind::RemoteBranch`], `refs/tags/` → [`RefKind::Tag`], exactly `refs/stash` →
///   [`RefKind::Stash`]. Other refs (e.g. `refs/notes/*`, `refs/stash/x`) are **ignored**:
///   the patterns of [`REFS_ARGS`] do not let them out, but if present they are not an error;
/// - symbolic refs (`%(symref)` non-empty, e.g. `refs/remotes/origin/HEAD`) are **ignored**:
///   they duplicate the ref they point to, which is already in the list;
/// - refs with a non-UTF-8 name are **ignored**: [`RefInfo::name`] is a `String` and a name
///   converted lossily would indicate a different ref to git;
/// - for an annotated tag [`RefInfo::target`] is the object pointed to by the tag and
///   [`RefInfo::tag_object`] the id of the tag. If the tag points to another annotated tag present
///   in the list it is followed all the way down; a tag on a tree or a blob has that
///   object as `target`, which is not a commit;
/// - the upstream has the full name; ahead/behind are `0`/`0` if level and `None` if git
///   answers `gone`. A non-UTF-8 upstream name is converted with `from_utf8_lossy` (for display
///   only: for commands use `<branch>@{upstream}`).
///
/// An input that does not end with NUL and newline, with a number of fields that is not a multiple of
/// [`REFS_FIELDS`] or with invalid fields gives [`DomainError::InvalidGitOutput`], with the number
/// of the record (from 1) involved.
pub fn parse_refs(input: &[u8]) -> Result<Vec<RefInfo>, DomainError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let body = input.strip_suffix(b"\0\n").ok_or_else(|| {
        invalid(
            "the output does not end with NUL and newline: it is truncated or was not produced with \
             the format of REFS_ARGS"
                .to_owned(),
        )
    })?;
    let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    let (records, rest) = fields.as_chunks::<REFS_FIELDS>();
    if !rest.is_empty() {
        let n = records.len() + 1;
        let have = rest.len();
        return Err(invalid(format!(
            "record {n}: {have} fields instead of {REFS_FIELDS} (missing {})",
            FIELD_NAMES[have]
        )));
    }
    let mut refs = Vec::new();
    for (i, record) in records.iter().enumerate() {
        let n = i + 1;
        if let Some(info) = parse_record(record, i == 0)
            .map_err(|reason| invalid(format!("record {n}: {reason}")))?
        {
            refs.push(info);
        }
    }
    peel_nested_tags(&mut refs);
    Ok(refs)
}

/// Reads a record; `None` if the ref is to be ignored.
fn parse_record(f: &[&[u8]; REFS_FIELDS], first: bool) -> Result<Option<RefInfo>, String> {
    let name = if first {
        f[0]
    } else {
        f[0].strip_prefix(b"\n")
            .ok_or_else(|| format!("missing newline before {}", FIELD_NAMES[0]))?
    };
    if name.is_empty() {
        return Err(format!("empty {}", FIELD_NAMES[0]));
    }
    let object = parse_oid(f[1], 1)?;
    let peeled = if f[2].is_empty() {
        None
    } else {
        Some(parse_oid(f[2], 2)?)
    };
    let upstream = parse_upstream(f[3], f[4])?;
    if !f[5].is_empty() {
        return Ok(None);
    }
    let Ok(name) = std::str::from_utf8(name) else {
        return Ok(None);
    };
    let Some(kind) = kind_of(name) else {
        return Ok(None);
    };
    let (target, tag_object) = match peeled {
        Some(peeled) => (peeled, Some(object)),
        None => (object, None),
    };
    Ok(Some(RefInfo {
        name: name.to_owned(),
        kind,
        target,
        tag_object,
        upstream,
    }))
}

fn kind_of(name: &str) -> Option<RefKind> {
    if name == "refs/stash" {
        Some(RefKind::Stash)
    } else if name.starts_with("refs/heads/") {
        Some(RefKind::LocalBranch)
    } else if name.starts_with("refs/remotes/") {
        Some(RefKind::RemoteBranch)
    } else if name.starts_with("refs/tags/") {
        Some(RefKind::Tag)
    } else {
        None
    }
}

fn parse_oid(field: &[u8], i: usize) -> Result<Oid, String> {
    Oid::from_hex(field).map_err(|e| format!("{}: {e}", FIELD_NAMES[i]))
}

fn parse_upstream(name: &[u8], track: &[u8]) -> Result<Option<Upstream>, String> {
    if name.is_empty() {
        return if track.is_empty() {
            Ok(None)
        } else {
            Err(format!(
                "{} is `{}` but {} is empty",
                FIELD_NAMES[4],
                show(track),
                FIELD_NAMES[3]
            ))
        };
    }
    let (ahead, behind) = parse_track(track)
        .ok_or_else(|| format!("invalid {}: `{}`", FIELD_NAMES[4], show(track)))?;
    Ok(Some(Upstream {
        name: show(name),
        ahead,
        behind,
    }))
}

/// `""` → `0`/`0`, `gone` → `None`/`None`, otherwise `ahead <n>`, `behind <m>` or
/// `ahead <n>, behind <m>`.
pub(super) fn parse_track(track: &[u8]) -> Option<(Option<u32>, Option<u32>)> {
    match track {
        b"" => return Some((Some(0), Some(0))),
        b"gone" => return Some((None, None)),
        _ => {}
    }
    let count = |s: &[u8]| parse_digits(s).and_then(|n| u32::try_from(n).ok());
    let (ahead, rest) = match track.strip_prefix(b"ahead ") {
        Some(rest) => match rest.iter().position(|&b| b == b',') {
            Some(i) => (count(&rest[..i])?, rest[i..].strip_prefix(b", ")?),
            None => return Some((Some(count(rest)?), Some(0))),
        },
        None => (0, track),
    };
    let behind = count(rest.strip_prefix(b"behind ")?)?;
    Some((Some(ahead), Some(behind)))
}

/// For tags of tags: if `target` is the object of another annotated tag in the list, it replaces it
/// with its `target`, all the way down.
fn peel_nested_tags(refs: &mut [RefInfo]) {
    let peeled: HashMap<Oid, Oid> = refs
        .iter()
        .filter_map(|r| r.tag_object.map(|tag| (tag, r.target)))
        .collect();
    for r in refs.iter_mut().filter(|r| r.tag_object.is_some()) {
        // Ids are content hashes, so there are no cycles; the limit protects anyway
        // from a deliberately crafted input.
        for _ in 0..peeled.len() {
            match peeled.get(&r.target) {
                Some(&next) if next != r.target => r.target = next,
                _ => break,
            }
        }
    }
}

fn show(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "for-each-ref",
        reason,
    }
}
