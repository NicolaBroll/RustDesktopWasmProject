//! Parser of git's unified diff format (`git diff`), and the inverse serializer.
//!
//! # Format
//!
//! With [`DIFF_ARGS`] the output is a sequence of sections, one per file:
//!
//! ```text
//! diff --git a/<old> b/<new>
//! [old mode <m>, new mode <m> | new file mode <m> | deleted file mode <m>]
//! [similarity index <n>%, rename from <p>, rename to <p> | copy from <p>, copy to <p>]
//! [index <id>..<id>[ <mode>]]
//! [Binary files a/<p> and b/<p> differ
//!  | --- a/<p>|/dev/null, +++ b/<p>|/dev/null, hunk...]
//! ```
//!
//! Behavior verified with git 2.43:
//!
//! - the headers always come in this order; the ones not needed are missing: a mode-only
//!   change or an exact rename has no `index` nor hunks, an empty new file has only
//!   `new file mode` and `index`;
//! - `index` has the mode at the end only if it is the same on both sides (never for new or deleted files);
//!   with `--full-index` the ids are full;
//! - a name containing `"`, `\` or control characters (newline, tab, ...) is quoted
//!   with C-style escapes, prefix included (`"a/with\ttab.txt"`), even with
//!   `core.quotePath=false`; with `core.quotePath=true` non-ASCII bytes also become octal
//!   escapes. The parser decodes both forms; [`FileDiff::write_header`] writes the first;
//! - after `--- <name>` and `+++ <name>` git adds a tab if the written name contains a
//!   space;
//! - a type change (e.g. from file to symbolic link) is a pair of sections on the same
//!   path: deleted file, then new file;
//! - a submodule (mode `160000`) with `--submodule=short` is a one-line text file,
//!   `Subproject commit <id>` (with `-dirty` if it has changes);
//! - the hunk starts with `@@ -<start>[,<lines>] +<start>[,<lines>] @@[ <context>]`: the
//!   line count is missing when it is 1; with 0 lines the start is the preceding line;
//!   `<context>` is the function line containing the hunk, if git finds it;
//! - every hunk line starts with ` ` (context), `-` (removed) or `+` (added); a
//!   `\ No newline at end of file` line after a line says that line has no trailing newline.
//!   With `diff.suppressBlankEmpty` an empty context line is written without the space: the
//!   parser accepts it. The parser counts lines from the hunk header, so a removed
//!   line `-- x` (written `--- x`) is not mistaken for a header;
//! - during a merge with conflicts, in the worktree a conflicted file is a combined diff
//!   (`diff --cc <name>` with `@@@` hunks), and in the index a `* Unmerged path <name>` line (with
//!   the name unquoted and unescaped). The parser reports them as [`Change::Unmerged`] without
//!   hunks: the content of the combined diff is not read.

use super::quote::{unquote, unquote_prefix, write_quoted};
use super::status::{mode, oid, show};
use super::{ByteText, Change, Oid, RepoPath};
use crate::DomainError;
use serde::{Deserialize, Serialize};

/// Mandatory arguments of `git diff` to produce the input of [`parse_diff`], together with
/// `-c core.quotePath=false` before the subcommand (the backend already passes it to every command).
///
/// The caller adds what to compare (`--cached`, revisions) and the paths after `--`; it may
/// add `-C`/`--find-copies-harder`, but not options that change the format, such as
/// `--stat`, `--word-diff`, `--binary` or `-B`, nor another `--unified`.
///
/// - `--no-color`: no color sequences with `color.diff=always`;
/// - `--no-ext-diff`: no external program from `diff.external` or from attributes;
/// - `--no-textconv`: the real content of the files, not the one converted by a `textconv` filter;
/// - `--find-renames`: renames recognized even with `diff.renames=false`;
/// - `--full-index`: full ids in the `index` line, to read them as [`Oid`];
/// - `--src-prefix=a/` and `--dst-prefix=b/`: fixed prefixes whatever `diff.noprefix` and
///   `diff.mnemonicPrefix` are (which would remove them or change them to `i/`, `w/`, `c/`...);
/// - `--submodule=short`: a submodule as a `Subproject commit <id>` line even with
///   `diff.submodule=log`, which would write the list of commits instead of the diff;
/// - `--no-relative`: paths from the repo root even with `diff.relative=true`;
/// - `--unified=3`: three context lines even with `diff.context=0` (or another value) in the
///   configuration. The displayed diff, its digest and the line patches built from it
///   (`build_patch`) thus always have the same context, and a hunk without context at the start or
///   end means it touches the start or end of the file: without context an insert-only hunk
///   (`@@ -2,0 +3 @@`) no longer says where it goes, and `git apply` would anchor it elsewhere.
pub const DIFF_ARGS: &[&str] = &[
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
    "--full-index",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--submodule=short",
    "--no-relative",
    "--unified=3",
];

/// The text git writes after a line without a trailing newline.
pub const NO_NEWLINE_MARKER: &[u8] = b"\\ No newline at end of file";

const GITLINK: u32 = 0o160000;

/// A diff: one element per file, in git's order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Diff {
    pub files: Vec<FileDiff>,
}

impl Diff {
    /// Rewrites the diff in git's format (see [`FileDiff::to_patch`]). For a diff read from
    /// `git diff` with [`DIFF_ARGS`] and without conflicts the result is identical to the original.
    pub fn to_patch(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for file in &self.files {
            file.write_patch(&mut out);
        }
        out
    }
}

/// The changes to a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiff {
    /// Path before the change (without `a/`). For a new file it equals `new_path`,
    /// as in git's `diff --git` line.
    pub old_path: RepoPath,
    /// Path after the change (without `b/`). For a deleted file it equals `old_path`.
    pub new_path: RepoPath,
    /// Only [`Change::Added`], [`Change::Deleted`], [`Change::Modified`] (content, mode or
    /// both), [`Change::Renamed`], [`Change::Copied`] or [`Change::Unmerged`] (conflicted
    /// file: without hunks, modes or ids).
    pub change: Change,
    /// Similarity as a percentage (0–100) for renames and copies.
    pub similarity: Option<u8>,
    /// Mode before the change, in octal (e.g. `0o100644`, `0o100755`, `0o120000` for a
    /// link, `0o160000` for a submodule). `None` for a new file, or if git does not write it
    /// (exact rename without a mode change).
    pub old_mode: Option<u32>,
    /// Mode after the change. `None` for a deleted file, or if git does not write it.
    pub new_mode: Option<u32>,
    /// Id of the content before the change (all zeros for a new file); `None` if git does not
    /// write the `index` line (unchanged content: mode-only change, exact rename).
    pub old_oid: Option<Oid>,
    /// Id of the content after the change (all zeros for a deleted file); `None` as above.
    pub new_oid: Option<Oid>,
    /// True if git considers the file binary (`Binary files ... differ`): no hunks.
    pub binary: bool,
    /// The hunks, in order. Empty for binaries, empty files, mode-only changes, exact renames and
    /// conflicted files.
    pub hunks: Vec<Hunk>,
}

impl FileDiff {
    /// The path to display: the new one (for a deleted file it coincides with the old one).
    pub fn path(&self) -> &RepoPath {
        &self.new_path
    }

    /// True if the path is a submodule (mode `160000`) before or after the change.
    pub fn is_submodule(&self) -> bool {
        self.old_mode == Some(GITLINK) || self.new_mode == Some(GITLINK)
    }

    /// Number of added and removed lines, for a summary like `+3 −1`.
    pub fn line_counts(&self) -> (usize, usize) {
        let lines = self.hunks.iter().flat_map(|h| &h.lines);
        lines.fold((0, 0), |(add, del), l| match l.kind {
            LineKind::Added => (add + 1, del),
            LineKind::Removed => (add, del + 1),
            LineKind::Context => (add, del),
        })
    }

    /// The file's headers as git writes them (from `diff --git` to `+++`, or up to
    /// `Binary files ... differ`), with names quoted only where needed. Followed by the
    /// hunks (see [`Hunk::write_to`]) they form a patch for `git apply`.
    ///
    /// The `---`/`+++` lines are present only if the file has hunks, as in git. For a
    /// conflicted file it writes only `* Unmerged path <name>`, which `git apply` does not accept.
    pub fn write_header(&self, out: &mut Vec<u8>) {
        if self.change == Change::Unmerged {
            out.extend_from_slice(b"* Unmerged path ");
            out.extend_from_slice(self.new_path.as_bytes());
            out.push(b'\n');
            return;
        }
        let (old, new) = (self.old_path.as_bytes(), self.new_path.as_bytes());
        out.extend_from_slice(b"diff --git ");
        write_quoted(out, b"a/", old);
        out.push(b' ');
        write_quoted(out, b"b/", new);
        out.push(b'\n');

        let added = self.change == Change::Added;
        let deleted = self.change == Change::Deleted;
        if added {
            line(
                out,
                &format!("new file mode {:06o}", self.new_mode.unwrap_or(0o100644)),
            );
        } else if deleted {
            line(
                out,
                &format!(
                    "deleted file mode {:06o}",
                    self.old_mode.unwrap_or(0o100644)
                ),
            );
        } else if let (Some(o), Some(n)) = (self.old_mode, self.new_mode)
            && o != n
        {
            line(out, &format!("old mode {o:06o}"));
            line(out, &format!("new mode {n:06o}"));
        }

        let verb: &[u8] = match self.change {
            Change::Renamed => b"rename",
            Change::Copied => b"copy",
            _ => b"",
        };
        if !verb.is_empty() {
            if let Some(score) = self.similarity {
                line(out, &format!("similarity index {score}%"));
            }
            for (dir, name) in [(&b" from "[..], old), (b" to ", new)] {
                out.extend_from_slice(verb);
                out.extend_from_slice(dir);
                write_quoted(out, b"", name);
                out.push(b'\n');
            }
        }

        if let (Some(o), Some(n)) = (self.old_oid, self.new_oid) {
            let same_mode = match (self.old_mode, self.new_mode) {
                (Some(a), Some(b)) if a == b && !added && !deleted => Some(a),
                _ => None,
            };
            match same_mode {
                Some(m) => line(out, &format!("index {o}..{n} {m:06o}")),
                None => line(out, &format!("index {o}..{n}")),
            }
        }

        let label = |prefix: &[u8], name: &[u8], missing: bool| {
            let mut l = Vec::new();
            if missing {
                l.extend_from_slice(b"/dev/null");
            } else {
                write_quoted(&mut l, prefix, name);
            }
            l
        };
        let minus = label(b"a/", old, added);
        let plus = label(b"b/", new, deleted);
        if self.binary {
            out.extend_from_slice(b"Binary files ");
            out.extend_from_slice(&minus);
            out.extend_from_slice(b" and ");
            out.extend_from_slice(&plus);
            out.extend_from_slice(b" differ\n");
        } else if !self.hunks.is_empty() {
            for (sign, l) in [(&b"--- "[..], minus), (b"+++ ", plus)] {
                out.extend_from_slice(sign);
                out.extend_from_slice(&l);
                if l.contains(&b' ') {
                    out.push(b'\t');
                }
                out.push(b'\n');
            }
        }
    }

    /// Headers and hunks of the file, in git's format.
    pub fn to_patch(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write_patch(&mut out);
        out
    }

    fn write_patch(&self, out: &mut Vec<u8>) {
        self.write_header(out);
        if self.change != Change::Unmerged && !self.binary {
            for hunk in &self.hunks {
                hunk.write_to(out);
            }
        }
    }
}

/// A block of changed lines, with the context around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
    /// First line in the old file (from 1); with `old_count == 0` it is the line after which
    /// the insertion happens (0 for the start of the file).
    pub old_start: u32,
    /// Lines of the old file in the hunk (context and removed).
    pub old_count: u32,
    /// First line in the new file, like `old_start`.
    pub new_start: u32,
    /// Lines of the new file in the hunk (context and added).
    pub new_count: u32,
    /// The text after the second `@@` (without the leading space), usually the line of the
    /// function containing the hunk; empty if git writes none.
    pub section: ByteText,
    pub lines: Vec<Line>,
}

impl Hunk {
    /// The header `@@ -a,b +c,d @@ context`, without newline; the line count is omitted
    /// when it is 1, as git does.
    pub fn header(&self) -> Vec<u8> {
        let range = |start: u32, count: u32| match count {
            1 => start.to_string(),
            _ => format!("{start},{count}"),
        };
        let mut out = format!(
            "@@ -{} +{} @@",
            range(self.old_start, self.old_count),
            range(self.new_start, self.new_count)
        )
        .into_bytes();
        if !self.section.is_empty() {
            out.push(b' ');
            out.extend_from_slice(self.section.as_bytes());
        }
        out
    }

    /// Writes header and lines in git's format, with `\ No newline at end of file` after
    /// the lines without a newline. The header counts are those of the fields, not
    /// recomputed from the lines (for a patch with removed lines use `git apply --recount`).
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.header());
        out.push(b'\n');
        for l in &self.lines {
            l.write_to(out);
        }
    }
}

/// The kind of a hunk line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LineKind {
    /// ` `: the same in both files.
    Context,
    /// `+`: only in the new file.
    Added,
    /// `-`: only in the old file.
    Removed,
}

impl LineKind {
    /// The character git starts the line with.
    pub fn prefix(self) -> u8 {
        match self {
            LineKind::Context => b' ',
            LineKind::Added => b'+',
            LineKind::Removed => b'-',
        }
    }
}

/// A hunk line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub kind: LineKind,
    /// Line number in the old file (from 1); `None` for added lines.
    pub old_line: Option<u32>,
    /// Line number in the new file (from 1); `None` for removed lines.
    pub new_line: Option<u32>,
    /// The content without the prefix and without the trailing newline (a possible `\r` stays).
    pub content: ByteText,
    /// True if in the file the line has no trailing newline (followed by
    /// `\ No newline at end of file`): only the last line of a side can be.
    pub no_newline: bool,
}

impl Line {
    /// Writes the line in git's format: prefix, content, newline and, if needed, the
    /// `\ No newline at end of file` line.
    pub fn write_to(&self, out: &mut Vec<u8>) {
        out.push(self.kind.prefix());
        out.extend_from_slice(self.content.as_bytes());
        out.push(b'\n');
        if self.no_newline {
            out.extend_from_slice(NO_NEWLINE_MARKER);
            out.push(b'\n');
        }
    }
}

fn line(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
    out.push(b'\n');
}

/// Reads the output of `git diff` produced with [`DIFF_ARGS`].
///
/// An empty input gives a diff without files. An input that does not end with a newline, an
/// unexpected line, an invalid header, a hunk with fewer lines than declared or a name
/// without the `a/`/`b/` prefixes gives [`DomainError::InvalidGitOutput`], with the number of the line
/// (from 1) involved.
pub fn parse_diff(input: &[u8]) -> Result<Diff, DomainError> {
    if input.is_empty() {
        return Ok(Diff::default());
    }
    let Some(body) = input.strip_suffix(b"\n") else {
        return Err(invalid(
            "the output does not end with a newline: it is truncated".to_owned(),
        ));
    };
    let lines: Vec<&[u8]> = body.split(|&b| b == b'\n').collect();
    let mut parser = Parser { lines, pos: 0 };
    let mut files = Vec::new();
    while let Some(first) = parser.peek() {
        let n = parser.pos + 1;
        let file = if let Some(rest) = first.strip_prefix(b"diff --git ") {
            parser.pos += 1;
            parser.git_file(rest, n)
        } else if let Some(rest) = first
            .strip_prefix(b"diff --cc ")
            .or_else(|| first.strip_prefix(b"diff --combined "))
        {
            parser.pos += 1;
            parser.combined(rest, n)
        } else if let Some(rest) = first.strip_prefix(b"* Unmerged path ") {
            parser.pos += 1;
            Ok(parser.unmerged(rest))
        } else {
            Err(format!(
                "line {n}: unexpected `{}`, expected the start of a file (`diff --git`)",
                show(first)
            ))
        };
        files.push(file.map_err(invalid)?);
    }
    Ok(Diff { files })
}

struct Parser<'a> {
    lines: Vec<&'a [u8]>,
    pos: usize,
}

/// Headers read for a file, before establishing the paths.
#[derive(Default)]
struct Headers {
    change: Option<Change>,
    similarity: Option<u8>,
    old_mode: Option<u32>,
    new_mode: Option<u32>,
    oids: Option<(Oid, Oid)>,
    index_mode: Option<u32>,
    from: Option<RepoPath>,
    to: Option<RepoPath>,
    binary: bool,
    labels: Option<(Option<RepoPath>, Option<RepoPath>)>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&'a [u8]> {
        self.lines.get(self.pos).copied()
    }

    /// True if the current line starts a new file (or there are no more lines).
    fn at_file_start(&self) -> bool {
        self.peek()
            .is_none_or(|l| l.starts_with(b"diff ") || l.starts_with(b"* Unmerged path "))
    }

    /// A `diff --git` section; `n` is the number of its first line.
    fn git_file(&mut self, rest: &[u8], n: usize) -> Result<FileDiff, String> {
        let at = |n: usize, reason: String| format!("line {n}: {reason}");
        let git_paths = git_line_paths(rest).map_err(|e| at(n, e))?;
        let mut h = Headers::default();
        let set_change = |h: &mut Headers, c: Change, n: usize| match h.change {
            Some(old) if old != c => Err(at(n, "headers of different change types".into())),
            _ => {
                h.change = Some(c);
                Ok(())
            }
        };
        while let Some(l) = self.peek() {
            let n = self.pos + 1;
            let err = |reason: String| at(n, reason);
            if l.starts_with(b"diff ") || l.starts_with(b"* Unmerged path ") {
                break;
            }
            self.pos += 1;
            if let Some(m) = l.strip_prefix(b"old mode ") {
                h.old_mode = Some(mode(m).map_err(err)?);
            } else if let Some(m) = l.strip_prefix(b"new mode ") {
                h.new_mode = Some(mode(m).map_err(err)?);
            } else if let Some(m) = l.strip_prefix(b"deleted file mode ") {
                set_change(&mut h, Change::Deleted, n)?;
                h.old_mode = Some(mode(m).map_err(err)?);
            } else if let Some(m) = l.strip_prefix(b"new file mode ") {
                set_change(&mut h, Change::Added, n)?;
                h.new_mode = Some(mode(m).map_err(err)?);
            } else if let Some(s) = l.strip_prefix(b"similarity index ") {
                h.similarity = Some(percent(s).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"rename from ") {
                set_change(&mut h, Change::Renamed, n)?;
                h.from = Some(header_path(p).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"rename to ") {
                set_change(&mut h, Change::Renamed, n)?;
                h.to = Some(header_path(p).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"copy from ") {
                set_change(&mut h, Change::Copied, n)?;
                h.from = Some(header_path(p).map_err(err)?);
            } else if let Some(p) = l.strip_prefix(b"copy to ") {
                set_change(&mut h, Change::Copied, n)?;
                h.to = Some(header_path(p).map_err(err)?);
            } else if let Some(i) = l.strip_prefix(b"index ") {
                let (o, new, m) = index_line(i).map_err(err)?;
                h.oids = Some((o, new));
                h.index_mode = m;
            } else if l.starts_with(b"Binary files ") && l.ends_with(b" differ") {
                h.binary = true;
                break;
            } else if let Some(minus) = l.strip_prefix(b"--- ") {
                let minus = label(minus, b"a/").map_err(err)?;
                let n = self.pos + 1;
                let plus = match self.peek().and_then(|p| p.strip_prefix(b"+++ ")) {
                    Some(p) => label(p, b"b/").map_err(|e| at(n, e))?,
                    None => return Err(at(n, "missing `+++` line after `---`".into())),
                };
                self.pos += 1;
                h.labels = Some((minus, plus));
                break;
            } else if l.starts_with(b"GIT binary patch") {
                return Err(err(
                    "binary patch (`--binary`) not supported: use the arguments of DIFF_ARGS"
                        .into(),
                ));
            } else if l.starts_with(b"dissimilarity index ") {
                return Err(err(
                    "complete rewrite (`-B`) not supported: use the arguments of DIFF_ARGS".into(),
                ));
            } else if l.starts_with(b"@@") {
                return Err(err(
                    "hunk without the `---` and `+++` lines before it".into()
                ));
            } else {
                return Err(err(format!("unknown header `{}`", show(l))));
            }
        }

        let change = h.change.unwrap_or(Change::Modified);
        let (old_path, new_path) = match (h.from, h.to, &h.labels) {
            (Some(from), Some(to), _) => (from, to),
            (None, None, Some((minus, plus))) => match (minus, plus) {
                (Some(a), Some(b)) => (a.clone(), b.clone()),
                (Some(a), None) => (a.clone(), a.clone()),
                (None, Some(b)) => (b.clone(), b.clone()),
                (None, None) => {
                    return Err(at(n, "`---` and `+++` are both `/dev/null`".into()));
                }
            },
            (None, None, None) => git_paths.ok_or_else(|| {
                at(
                    n,
                    format!("cannot get the paths from `diff --git {}`", show(rest)),
                )
            })?,
            _ => {
                return Err(at(
                    n,
                    "rename or copy without both `from` and `to` lines".into(),
                ));
            }
        };
        let (mut old_mode, mut new_mode) = (h.old_mode, h.new_mode);
        if let Some(m) = h.index_mode {
            old_mode = old_mode.or(Some(m));
            new_mode = new_mode.or(Some(m));
        }
        if matches!(change, Change::Added) && h.labels.as_ref().is_some_and(|l| l.0.is_some()) {
            return Err(at(n, "new file with `---` other than `/dev/null`".into()));
        }
        if matches!(change, Change::Deleted) && h.labels.as_ref().is_some_and(|l| l.1.is_some()) {
            return Err(at(
                n,
                "deleted file with `+++` other than `/dev/null`".into(),
            ));
        }

        let mut hunks = Vec::new();
        if h.labels.is_some() {
            while let Some(l) = self.peek() {
                if !l.starts_with(b"@@") {
                    break;
                }
                hunks.push(self.hunk(l)?);
            }
            if hunks.is_empty() {
                return Err(at(self.pos + 1, "no hunk after the `+++` line".into()));
            }
            if !self.at_file_start() {
                let l = self.peek().unwrap_or_default();
                return Err(at(
                    self.pos + 1,
                    format!("unexpected `{}` after the last hunk", show(l)),
                ));
            }
        }

        Ok(FileDiff {
            old_path,
            new_path,
            change,
            similarity: h.similarity,
            old_mode,
            new_mode,
            old_oid: h.oids.map(|o| o.0),
            new_oid: h.oids.map(|o| o.1),
            binary: h.binary,
            hunks,
        })
    }

    /// A hunk starting at the current line (`header`).
    fn hunk(&mut self, header: &[u8]) -> Result<Hunk, String> {
        let n = self.pos + 1;
        let at = |n: usize, reason: String| format!("line {n}: {reason}");
        let (old_start, old_count, new_start, new_count, section) =
            hunk_header(header).map_err(|e| at(n, e))?;
        self.pos += 1;
        let (mut old_left, mut new_left) = (old_count, new_count);
        let (mut old_no, mut new_no) = (old_start, new_start);
        let mut lines: Vec<Line> = Vec::new();
        loop {
            let pending = old_left > 0 || new_left > 0;
            let n = self.pos + 1;
            let Some(l) = self.peek() else {
                if pending {
                    return Err(at(
                        n,
                        format!(
                            "truncated hunk: {old_left} lines of the old file and \
                             {new_left} of the new one are missing"
                        ),
                    ));
                }
                break;
            };
            if l.starts_with(b"\\") {
                let Some(last) = lines.last_mut() else {
                    return Err(at(
                        n,
                        "`\\` at the start of a hunk, without a line before it".into(),
                    ));
                };
                last.no_newline = true;
                self.pos += 1;
                continue;
            }
            if !pending {
                break;
            }
            let (kind, content) = match l.split_first() {
                Some((b' ', c)) => (LineKind::Context, c),
                // `diff.suppressBlankEmpty`: empty context line without the space.
                None => (LineKind::Context, &[][..]),
                Some((b'-', c)) => (LineKind::Removed, c),
                Some((b'+', c)) => (LineKind::Added, c),
                Some(_) => {
                    return Err(at(
                        n,
                        format!(
                            "`{}` is not a hunk line (it must start with ` `, `-`, `+` or \
                             `\\`); {old_left} lines of the old file and {new_left} of the new \
                             one are missing",
                            show(l)
                        ),
                    ));
                }
            };
            let uses_old = kind != LineKind::Added;
            let uses_new = kind != LineKind::Removed;
            if (uses_old && old_left == 0) || (uses_new && new_left == 0) {
                return Err(at(
                    n,
                    format!(
                        "`{}`: the hunk has more lines than declared in `{}`",
                        show(l),
                        show(header)
                    ),
                ));
            }
            let mut line = Line {
                kind,
                old_line: None,
                new_line: None,
                content: ByteText::from(content),
                no_newline: false,
            };
            if uses_old {
                line.old_line = Some(old_no);
                old_left -= 1;
                old_no = old_no.saturating_add(1);
            }
            if uses_new {
                line.new_line = Some(new_no);
                new_left -= 1;
                new_no = new_no.saturating_add(1);
            }
            lines.push(line);
            self.pos += 1;
        }
        Ok(Hunk {
            old_start,
            old_count,
            new_start,
            new_count,
            section: ByteText::from(section),
            lines,
        })
    }

    /// `diff --cc <name>`: conflicted file; the content of the combined diff is skipped up to the
    /// next file (its lines start with ` `, `+`, `-` or `\`, never with `diff `).
    fn combined(&mut self, rest: &[u8], n: usize) -> Result<FileDiff, String> {
        let path = unquote(rest).map_err(|e| format!("line {n}: {e}"))?;
        if path.is_empty() {
            return Err(format!("line {n}: empty path in `diff --cc`"));
        }
        while !self.at_file_start() {
            self.pos += 1;
        }
        Ok(unmerged_file(RepoPath::from(path)))
    }

    /// `* Unmerged path <name>`: the name is written as is, so a newline in the name
    /// continues it on the following lines, up to the start of the next file.
    fn unmerged(&mut self, rest: &[u8]) -> FileDiff {
        let mut path = rest.to_vec();
        while !self.at_file_start() {
            path.push(b'\n');
            path.extend_from_slice(self.peek().unwrap_or_default());
            self.pos += 1;
        }
        unmerged_file(RepoPath::from(path))
    }
}

pub(super) fn unmerged_file(path: RepoPath) -> FileDiff {
    FileDiff {
        old_path: path.clone(),
        new_path: path,
        change: Change::Unmerged,
        similarity: None,
        old_mode: None,
        new_mode: None,
        old_oid: None,
        new_oid: None,
        binary: false,
        hunks: Vec::new(),
    }
}

/// The two paths of the `diff --git a/<old> b/<new>` line, without prefixes. `None` if
/// they are ambiguous (two unquoted names, different, with ` b/` repeated): then the
/// `rename`/`copy` or `---`/`+++` lines are needed.
fn git_line_paths(rest: &[u8]) -> Result<Option<(RepoPath, RepoPath)>, String> {
    let strip = |name: Vec<u8>, prefix: &[u8]| -> Result<RepoPath, String> {
        match name.strip_prefix(prefix) {
            Some(p) if !p.is_empty() => Ok(RepoPath::from(p)),
            _ => Err(missing_prefix(&name, prefix)),
        }
    };
    if rest.starts_with(b"\"") {
        let (a, used) = unquote_prefix(rest)?;
        let Some(b) = rest[used..].strip_prefix(b" ") else {
            return Err(format!(
                "missing space between the names in `{}`",
                show(rest)
            ));
        };
        return Ok(Some((strip(a, b"a/")?, strip(unquote(b)?, b"b/")?)));
    }
    if rest.ends_with(b"\"") {
        // First name unquoted, second quoted: look for the ` "` from which the rest is
        // exactly a quoted string.
        for (i, w) in rest.windows(2).enumerate() {
            if w == b" \""
                && let Ok(b) = unquote(&rest[i + 1..])
            {
                return Ok(Some((strip(rest[..i].to_vec(), b"a/")?, strip(b, b"b/")?)));
            }
        }
        return Err(format!("invalid quoted names in `{}`", show(rest)));
    }
    // Same name on both sides: `a/<n> b/<n>`.
    if rest.len() >= 5 && (rest.len() - 5).is_multiple_of(2) {
        let n = (rest.len() - 5) / 2;
        let (a, b) = rest.split_at(2 + n);
        if a.starts_with(b"a/") && b.starts_with(b" b/") && a[2..] == b[3..] {
            let p = RepoPath::from(&a[2..]);
            return Ok(Some((p.clone(), p)));
        }
    }
    let seps: Vec<usize> = rest
        .windows(3)
        .enumerate()
        .filter(|(_, w)| *w == b" b/")
        .map(|(i, _)| i)
        .collect();
    match seps[..] {
        [i] => Ok(Some((
            strip(rest[..i].to_vec(), b"a/")?,
            strip(rest[i + 1..].to_vec(), b"b/")?,
        ))),
        _ => Ok(None),
    }
}

/// The name of `---`/`+++`: `None` for `/dev/null`, otherwise without `prefix`. The tab that git
/// adds after a name with spaces is removed.
fn label(raw: &[u8], prefix: &[u8]) -> Result<Option<RepoPath>, String> {
    let name = if raw.starts_with(b"\"") {
        let (name, used) = unquote_prefix(raw)?;
        match &raw[used..] {
            [] | [b'\t'] => name,
            _ => return Err(format!("text after the name in `{}`", show(raw))),
        }
    } else {
        let name = raw.strip_suffix(b"\t").unwrap_or(raw);
        if name == b"/dev/null" {
            return Ok(None);
        }
        name.to_vec()
    };
    match name.strip_prefix(prefix) {
        Some(p) if !p.is_empty() => Ok(Some(RepoPath::from(p))),
        _ => Err(missing_prefix(&name, prefix)),
    }
}

fn missing_prefix(name: &[u8], prefix: &[u8]) -> String {
    format!(
        "the name `{}` does not start with `{}`: `--src-prefix=a/ --dst-prefix=b/` is needed (DIFF_ARGS)",
        show(name),
        show(prefix)
    )
}

/// A path of `rename from`/`rename to`/`copy from`/`copy to`, without prefix.
fn header_path(raw: &[u8]) -> Result<RepoPath, String> {
    let p = unquote(raw)?;
    if p.is_empty() {
        return Err("empty path".to_owned());
    }
    Ok(RepoPath::from(p))
}

/// `<n>%` with `n` between 0 and 100.
fn percent(s: &[u8]) -> Result<u8, String> {
    s.strip_suffix(b"%")
        .and_then(super::log::parse_digits)
        .and_then(|v| u8::try_from(v).ok())
        .filter(|v| *v <= 100)
        .ok_or_else(|| format!("`{}` is not a valid percentage", show(s)))
}

/// `<id>..<id>[ <mode>]`.
fn index_line(s: &[u8]) -> Result<(Oid, Oid, Option<u32>), String> {
    let (ids, m) = match s.iter().position(|&b| b == b' ') {
        Some(i) => (&s[..i], Some(mode(&s[i + 1..])?)),
        None => (s, None),
    };
    let Some(dots) = ids.windows(2).position(|w| w == b"..") else {
        return Err(format!("`index {}` line without `..`", show(s)));
    };
    let hint = |e: String| format!("{e}; with `--full-index` (DIFF_ARGS) the ids are complete");
    let old = oid(&ids[..dots], "old id").map_err(hint)?;
    let new = oid(&ids[dots + 2..], "new id").map_err(hint)?;
    Ok((old, new, m))
}

/// `@@ -a[,b] +c[,d] @@[ context]`.
fn hunk_header(h: &[u8]) -> Result<(u32, u32, u32, u32, &[u8]), String> {
    let bad = || format!("`{}` is not a hunk header `@@ -a,b +c,d @@`", show(h));
    let rest = h.strip_prefix(b"@@ -").ok_or_else(bad)?;
    let sp = rest.iter().position(|&b| b == b' ').ok_or_else(bad)?;
    let (old_start, old_count) = range(&rest[..sp]).ok_or_else(bad)?;
    let rest = rest[sp + 1..].strip_prefix(b"+").ok_or_else(bad)?;
    let sp = rest.iter().position(|&b| b == b' ').ok_or_else(bad)?;
    let (new_start, new_count) = range(&rest[..sp]).ok_or_else(bad)?;
    let rest = rest[sp + 1..].strip_prefix(b"@@").ok_or_else(bad)?;
    let section = match rest {
        [] => rest,
        [b' ', s @ ..] => s,
        _ => return Err(bad()),
    };
    Ok((old_start, old_count, new_start, new_count, section))
}

/// `<start>[,<lines>]`; without lines it is 1.
fn range(s: &[u8]) -> Option<(u32, u32)> {
    let num = |d: &[u8]| super::log::parse_digits(d).and_then(|v| u32::try_from(v).ok());
    match s.iter().position(|&b| b == b',') {
        Some(i) => Some((num(&s[..i])?, num(&s[i + 1..])?)),
        None => Some((num(s)?, 1)),
    }
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "diff",
        reason,
    }
}
