//! Patch builder for staging and unstaging single lines.
//!
//! Given a [`FileDiff`] read with [`super::parse_diff`] and the lines chosen by the user,
//! [`build_patch`] returns the patch to pass to `git apply` with the arguments of
//! [`APPLY_ARGS`] (patch on stdin). The patch is a [`FileDiff`]: [`FileDiff::to_patch`] gives the bytes.
//!
//! # The two directions
//!
//! `git apply --cached` compares the **preimage** of the patch (context lines and `-` lines)
//! with the content of the file in the index, and if it matches it replaces it with the **postimage**
//! (context and `+` lines). In both directions the patch is built *forwards* relative
//! to the index, that is its preimage is the current content of the index, and it is applied with the
//! same arguments (no `--reverse`):
//!
//! - [`PatchDirection::Stage`], on the worktree → index diff (`diff.worktree`, `git diff`): the
//!   old side of the diff is the index, so it is the preimage. Selected added lines: `+`;
//!   unselected added lines: dropped; selected removed lines: `-`; unselected removed lines:
//!   context (they are in the index and stay there).
//! - [`PatchDirection::Unstage`], on the HEAD → index diff (`diff.index`, `git diff --cached`): here
//!   the index is the **new** side of the diff. The builder inverts the diff (index → HEAD) and then
//!   applies the same rule, so the rule is reversed relative to the lines shown: selected
//!   added lines (staged, to be removed): `-`; unselected added lines: context (they are
//!   in the index and stay there); selected removed lines (to be put back): `+`; unselected removed lines:
//!   dropped (they are not in the index and do not come back).
//!
//! In both directions the order in the result is that of the displayed diff, with one
//! refinement: in every group of changes between two context lines the first removed line is
//! paired with the first added one, the second with the second, and so on (git writes all the removed
//! lines and then all the added ones). So of `-x -y +x' +y'` staging only `x → x'` gives `x' y`, not
//! `y x'`. It follows that unstaging a selection gives the same content as staging
//! the unselected lines starting from HEAD.
//!
//! # Selection
//!
//! A changed line is indicated by its line number on the side where it exists, as the read
//! diff shows it ([`Line::old_line`] / [`Line::new_line`]): [`LineRef::Old`] for a removed line,
//! [`LineRef::New`] for an added one. In JSON `{"old": 3}` and `{"new": 5}`. In the worktree diff
//! `old` is the number in the index and `new` in the worktree; in the index diff `old` is in HEAD and
//! `new` in the index. A whole hunk is the set of its changed lines
//! ([`Hunk::line_refs`]); context lines cannot be selected.
//!
//! - Empty selection: `Ok(None)`, no patch.
//! - A reference that does not match a changed line of the diff (nonexistent number, context
//!   line, wrong side): [`DomainError::InvalidLineSelection`] and no patch. If the
//!   file changed since the selection was made the numbers may no longer match:
//!   better an error than a patch on different lines. Repeated references count once.
//!
//! # Special cases
//!
//! - **End of file without a newline**: a line without a newline can only be the last. If in the
//!   result a line without a newline is no longer the last (e.g. the old last line is kept and
//!   a line is added after it), it gets the newline: if it was in the preimage it becomes the pair
//!   `-line` + `\ No newline at end of file` and `+line`.
//! - **New file** (in the worktree diff only for a file added with `git add -N`, which git
//!   shows as new; in the index diff for the unstage of a file deleted in the index): the
//!   patch creates the file with only the selected lines (`new file mode`). `git apply --cached`
//!   agrees to create a file that in the index is only an `intent-to-add` placeholder; an
//!   untracked file does not appear in `git diff` and must be staged whole.
//! - **Deleted file** (in the worktree diff: deleted from the worktree; in the index diff: a
//!   new staged file being unstaged): with all lines selected the patch deletes
//!   the file from the index (`deleted file mode`); with a partial selection the file stays with the
//!   unselected lines and the patch is a modification.
//! - **Rename**: line patches never rename. In the index diff (staged rename)
//!   the patch modifies the file at the new path, the one in the index: the
//!   old path stays out of the index; to undo the rename the whole-file unstage is used.
//!   In the worktree diff a rename (with `git add -N`) is rejected: the whole file is
//!   staged.
//! - **Mode change**: it is not part of the lines and never enters the patch, which leaves the
//!   index mode as it is; it is staged with the whole file. A mode-only change has no
//!   lines to select.
//! - **Headers**: the patch has no `index` line (the ids would no longer be those of the
//!   content) nor `similarity`/`rename`; names are written by [`FileDiff::write_header`],
//!   quoted where needed, with exact bytes. The hunk counts are recomputed, so
//!   the patch is valid even without `--recount`. The content (CRLF, non-UTF-8 bytes) is copied
//!   byte by byte.
//! - **Rejected** with [`DomainError::UnsupportedPatch`]: binary files, submodules, symbolic
//!   links, conflicted files, copies, renames in the worktree diff. This check comes before
//!   the selection check.

use super::{Change, FileDiff, Hunk, Line, LineKind, RepoPath};
use crate::DomainError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Arguments of `git apply` to apply a [`build_patch`] patch to the index, read from
/// stdin; they hold for both directions.
///
/// - `--cached`: only the index, never the worktree;
/// - `--recount`: line counts recomputed from the content (the written ones are already
///   correct, it is an extra safety);
/// - `--whitespace=nowarn`: with `apply.whitespace=error` in the configuration a line with trailing
///   spaces would make the patch fail; here the user's content is copied as it is;
/// - `--no-ignore-whitespace`: with `apply.ignoreWhitespace=change` the preimage could
///   match an index that differs in whitespace.
///
/// Before writing, the patch can be tried with `--check` added.
///
/// Warning: `git apply` looks for a hunk's preimage even far from the written position,
/// and a hunk without trailing context is anchored at the end of the file. It is safe only with patches from a
/// diff read with `DIFF_ARGS` (fixed context `--unified=3`, even with `diff.context=0` in the
/// configuration) and applied to the same index the diff was computed from. The commands
/// `index.stage_lines`/`index.unstage_lines` therefore do not use `git apply`: they compute the
/// result with a strict applier in the engine and write it to the index.
pub const APPLY_ARGS: &[&str] = &[
    "--cached",
    "--recount",
    "--whitespace=nowarn",
    "--no-ignore-whitespace",
];

const SYMLINK: u32 = 0o120000;

/// The direction the selected lines move in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchDirection {
    /// From the worktree to the index: the diff is the worktree one (`diff.worktree`).
    Stage,
    /// From the index back to HEAD: the diff is the index one (`diff.index`).
    Unstage,
}

/// A changed line of a diff, indicated by the line number on the side where it exists.
///
/// In JSON: `{"old": <n>}` or `{"new": <n>}`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum LineRef {
    /// A removed line, with its number in the old file ([`Line::old_line`]).
    Old(u32),
    /// An added line, with its number in the new file ([`Line::new_line`]).
    New(u32),
}

impl Line {
    /// The reference to select this line with; `None` for a context line.
    pub fn line_ref(&self) -> Option<LineRef> {
        match self.kind {
            LineKind::Context => None,
            LineKind::Removed => self.old_line.map(LineRef::Old),
            LineKind::Added => self.new_line.map(LineRef::New),
        }
    }
}

impl Hunk {
    /// The references of all the changed lines of the hunk, in order: the selection of the whole
    /// hunk.
    pub fn line_refs(&self) -> Vec<LineRef> {
        self.lines.iter().filter_map(Line::line_ref).collect()
    }
}

impl FileDiff {
    /// The references of all the changed lines of the file, in order.
    pub fn line_refs(&self) -> Vec<LineRef> {
        self.hunks.iter().flat_map(Hunk::line_refs).collect()
    }
}

/// Builds the patch that moves into the index (or removes from the index) only the lines `lines` of
/// `file`, to be applied with `git apply` and the arguments of [`APPLY_ARGS`].
///
/// `file` is a file of `diff.worktree` for [`PatchDirection::Stage`], of `diff.index` for
/// [`PatchDirection::Unstage`]; `lines` uses the line numbers of `file` (see [`LineRef`]).
/// Returns `Ok(None)` if `lines` is empty. See the module documentation for the
/// semantics, the special cases and the rejected ones.
pub fn build_patch(
    file: &FileDiff,
    lines: &[LineRef],
    direction: PatchDirection,
) -> Result<Option<FileDiff>, DomainError> {
    check_supported(file, direction)?;
    if lines.is_empty() {
        return Ok(None);
    }
    let selected = selection(file, lines)?;
    let (shown, selected) = interleave(file, &selected);
    let base = match direction {
        PatchDirection::Stage => shown,
        PatchDirection::Unstage => invert(&shown),
    };
    Ok(select(&base, &selected))
}

/// The diff with the lines of each group of changes (between two context lines) alternated: the
/// first removed, the first added, the second removed, the second added... with the selection
/// reordered the same way. Each side keeps its order, so the diff means the
/// same thing; it serves to decide where the lines that stay end up, in the result, relative
/// to those that come in: a change `-x -y +x' +y'` of which only `x → x'` is chosen gives
/// `x' y`, not `y x'`.
fn interleave(file: &FileDiff, selected: &[Vec<bool>]) -> (FileDiff, Vec<Vec<bool>>) {
    let mut out = file.clone();
    let mut flags = Vec::with_capacity(file.hunks.len());
    for (hunk, sel) in out.hunks.iter_mut().zip(selected) {
        let lines = std::mem::take(&mut hunk.lines);
        let mut new_lines = Vec::with_capacity(lines.len());
        let mut new_sel = Vec::with_capacity(lines.len());
        let mut removed: Vec<(Line, bool)> = Vec::new();
        let mut added: Vec<(Line, bool)> = Vec::new();
        for (i, l) in lines.into_iter().enumerate() {
            let s = sel.get(i).copied().unwrap_or(false);
            match l.kind {
                LineKind::Removed => removed.push((l, s)),
                LineKind::Added => added.push((l, s)),
                LineKind::Context => {
                    flush(&mut removed, &mut added, &mut new_lines, &mut new_sel);
                    new_lines.push(l);
                    new_sel.push(s);
                }
            }
        }
        flush(&mut removed, &mut added, &mut new_lines, &mut new_sel);
        hunk.lines = new_lines;
        flags.push(new_sel);
    }
    (out, flags)
}

/// Empties a group of changes into `out`, alternating removed and added.
fn flush(
    removed: &mut Vec<(Line, bool)>,
    added: &mut Vec<(Line, bool)>,
    out: &mut Vec<Line>,
    sel: &mut Vec<bool>,
) {
    let mut r = removed.drain(..);
    let mut a = added.drain(..);
    loop {
        let (x, y) = (r.next(), a.next());
        if x.is_none() && y.is_none() {
            break;
        }
        for (l, s) in x.into_iter().chain(y) {
            out.push(l);
            sel.push(s);
        }
    }
}

fn check_supported(file: &FileDiff, direction: PatchDirection) -> Result<(), DomainError> {
    let reason = if file.change == Change::Unmerged {
        Some("the file is conflicted: it must be resolved and staged as a whole")
    } else if file.binary {
        Some("the file is binary: it has no lines to select, it is staged as a whole")
    } else if file.is_submodule() {
        Some("it is a submodule: it has no lines to select, it is staged as a whole")
    } else if file.old_mode == Some(SYMLINK) || file.new_mode == Some(SYMLINK) {
        Some("it is a symbolic link: its content is staged only as a whole")
    } else if file.change == Change::Copied {
        Some("it is a copy: only the whole file is staged")
    } else if file.change == Change::Renamed && direction == PatchDirection::Stage {
        Some("it is a rename not yet staged: the whole file is staged, with both paths")
    } else if !matches!(
        file.change,
        Change::Added | Change::Deleted | Change::Modified | Change::Renamed
    ) {
        Some("change type not expected in a diff")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(DomainError::UnsupportedPatch {
            path: file.path().to_string_lossy().into_owned(),
            reason: reason.to_owned(),
        }),
        None => Ok(()),
    }
}

/// For each hunk and line of `file`, true if the line is selected.
fn selection(file: &FileDiff, lines: &[LineRef]) -> Result<Vec<Vec<bool>>, DomainError> {
    let wanted: HashSet<LineRef> = lines.iter().copied().collect();
    let mut found = HashSet::new();
    let selected = file
        .hunks
        .iter()
        .map(|h| {
            h.lines
                .iter()
                .map(|l| match l.line_ref() {
                    Some(r) if wanted.contains(&r) => {
                        found.insert(r);
                        true
                    }
                    _ => false,
                })
                .collect()
        })
        .collect();
    if found.len() != wanted.len() {
        let mut missing: Vec<LineRef> = wanted.difference(&found).copied().collect();
        missing.sort();
        let list: Vec<String> = missing
            .iter()
            .map(|r| match r {
                LineRef::Old(n) => format!("removed {n}"),
                LineRef::New(n) => format!("added {n}"),
            })
            .collect();
        return Err(DomainError::InvalidLineSelection {
            path: file.path().to_string_lossy().into_owned(),
            reason: format!(
                "there is no line {} in the diff (did the file change in the meantime?)",
                list.join(", ")
            ),
        });
    }
    Ok(selected)
}

/// The diff in the other direction: removed and added lines swapped, like the sides and paths.
fn invert(file: &FileDiff) -> FileDiff {
    let change = match file.change {
        Change::Added => Change::Deleted,
        Change::Deleted => Change::Added,
        c => c,
    };
    let hunks = file
        .hunks
        .iter()
        .map(|h| Hunk {
            old_start: h.new_start,
            old_count: h.new_count,
            new_start: h.old_start,
            new_count: h.old_count,
            section: h.section.clone(),
            lines: h
                .lines
                .iter()
                .map(|l| Line {
                    kind: match l.kind {
                        LineKind::Added => LineKind::Removed,
                        LineKind::Removed => LineKind::Added,
                        LineKind::Context => LineKind::Context,
                    },
                    old_line: l.new_line,
                    new_line: l.old_line,
                    content: l.content.clone(),
                    no_newline: l.no_newline,
                })
                .collect(),
        })
        .collect();
    FileDiff {
        old_path: file.new_path.clone(),
        new_path: file.old_path.clone(),
        change,
        similarity: file.similarity,
        old_mode: file.new_mode,
        new_mode: file.old_mode,
        old_oid: file.new_oid,
        new_oid: file.old_oid,
        binary: file.binary,
        hunks,
    }
}

/// The stage rule on `base` (whose old side is the index): `None` if no selected line
/// changes anything.
fn select(base: &FileDiff, selected: &[Vec<bool>]) -> Option<FileDiff> {
    // Lines of the result, in diff order: context, unselected removed, selected
    // added. `later[h][i]`: after line `i` of hunk `h` there is another line of the result.
    let in_result = |l: &Line, sel: bool| match l.kind {
        LineKind::Context => true,
        LineKind::Removed => !sel,
        LineKind::Added => sel,
    };
    let mut later: Vec<Vec<bool>> = Vec::with_capacity(base.hunks.len());
    let mut seen = false;
    for (h, sel) in base.hunks.iter().zip(selected).rev() {
        let mut row = vec![false; h.lines.len()];
        for (i, l) in h.lines.iter().enumerate().rev() {
            row[i] = seen;
            seen |= in_result(l, sel.get(i).copied().unwrap_or(false));
        }
        later.push(row);
    }
    later.reverse();

    let mut hunks = Vec::new();
    let mut delta: i64 = 0;
    for ((h, sel), later) in base.hunks.iter().zip(selected).zip(&later) {
        let mut out: Vec<Line> = Vec::with_capacity(h.lines.len());
        let mut changed = false;
        let mut push = |kind: LineKind, l: &Line, no_newline: bool| {
            out.push(Line {
                kind,
                old_line: None,
                new_line: None,
                content: l.content.clone(),
                no_newline,
            });
        };
        for (i, l) in h.lines.iter().enumerate() {
            let sel = sel.get(i).copied().unwrap_or(false);
            let later = later.get(i).copied().unwrap_or(false);
            match (l.kind, sel) {
                (LineKind::Context, _) | (LineKind::Removed, false) => {
                    if l.no_newline && later {
                        // Last line of the preimage without a newline, followed by other lines in the
                        // result: in the result it gets the newline.
                        push(LineKind::Removed, l, true);
                        push(LineKind::Added, l, false);
                        changed = true;
                    } else {
                        push(LineKind::Context, l, l.no_newline);
                    }
                }
                (LineKind::Removed, true) => {
                    push(LineKind::Removed, l, l.no_newline);
                    changed = true;
                }
                (LineKind::Added, true) => {
                    push(LineKind::Added, l, l.no_newline && !later);
                    changed = true;
                }
                (LineKind::Added, false) => {}
            }
        }
        if !changed {
            continue;
        }
        let lines = canonical_order(out);
        let old_count = count(&lines, LineKind::Added);
        let new_count = count(&lines, LineKind::Removed);
        let old_pos = i64::from(h.old_start) + i64::from(old_count == 0);
        let new_pos = old_pos + delta;
        let new_start = new_pos - i64::from(new_count == 0);
        delta += i64::from(new_count) - i64::from(old_count);
        let mut hunk = Hunk {
            old_start: h.old_start,
            old_count,
            new_start: u32::try_from(new_start.max(0)).unwrap_or(u32::MAX),
            new_count,
            section: h.section.clone(),
            lines,
        };
        number(&mut hunk);
        hunks.push(hunk);
    }
    if hunks.is_empty() {
        return None;
    }

    let empty_result = hunks.iter().all(|h| h.new_count == 0);
    let (change, path) = match base.change {
        Change::Added => (Change::Added, &base.new_path),
        Change::Deleted if empty_result => (Change::Deleted, &base.old_path),
        _ => (Change::Modified, &base.old_path),
    };
    let path: RepoPath = path.clone();
    Some(FileDiff {
        old_path: path.clone(),
        new_path: path,
        change,
        similarity: None,
        old_mode: (change == Change::Deleted)
            .then_some(base.old_mode)
            .flatten(),
        new_mode: (change == Change::Added).then_some(base.new_mode).flatten(),
        old_oid: None,
        new_oid: None,
        binary: false,
        hunks,
    })
}

/// Lines of the old side (`skip` = [`LineKind::Added`]) or new side (`skip` =
/// [`LineKind::Removed`]).
fn count(lines: &[Line], skip: LineKind) -> u32 {
    let n = lines.iter().filter(|l| l.kind != skip).count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// In each group of changed lines between two context lines, first the removed and then the
/// added, as git writes. The order of each side does not change, so neither does the meaning.
fn canonical_order(lines: Vec<Line>) -> Vec<Line> {
    let mut out = Vec::with_capacity(lines.len());
    let mut added = Vec::new();
    for l in lines {
        match l.kind {
            LineKind::Removed => out.push(l),
            LineKind::Added => added.push(l),
            LineKind::Context => {
                out.append(&mut added);
                out.push(l);
            }
        }
    }
    out.append(&mut added);
    out
}

/// Assigns the line numbers of the two sides starting from the hunk header.
fn number(hunk: &mut Hunk) {
    let (mut old, mut new) = (hunk.old_start, hunk.new_start);
    for l in &mut hunk.lines {
        if l.kind != LineKind::Added {
            l.old_line = Some(old);
            old = old.saturating_add(1);
        }
        if l.kind != LineKind::Removed {
            l.new_line = Some(new);
            new = new.saturating_add(1);
        }
    }
}
