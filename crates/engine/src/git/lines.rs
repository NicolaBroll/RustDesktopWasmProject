//! Stage and unstage of single lines (`index.stage_lines`, `index.unstage_lines`): the part
//! shared by the backends.
//!
//! The backend rereads the file's diff ([`LineSelection::diff_spec`]), checks that it is the one
//! seen by whoever chose the lines and builds the patch with [`line_patch`]; then it computes the
//! new staged content with [`apply_patch`] and writes it to the index (the `CliBackend` with
//! `hash-object -w --no-filters` and `update-index --index-info`, the `MemoryBackend` in its
//! blobs).
//!
//! # Why not `git apply`
//!
//! `git apply` looks for a hunk's preimage even far from the written position, and a hunk
//! without trailing context is anchored at the end of the file: with a zero-context diff
//! (`diff.context=0` in the configuration) an insertion in the middle of the file ended up at the end and the
//! command succeeded. Now the context is fixed (`--unified=3` in `DIFF_ARGS`), [`line_patch`]
//! rejects patches with unanchored hunks and [`apply_patch`] applies each hunk only at its
//! position, on the blob of the index entry the diff was computed from (checked
//! by the backend): the result is exactly what the selection describes.
//!
//! # The seen diff
//!
//! The line numbers of a selection are valid only for the diff they were taken from: if the
//! file changed in the meantime (in the worktree or in the index) the same numbers may
//! point to different lines, and the patch would move into the index lines the user never
//! saw. For this reason the selection carries the [`file_digest`] of the `FileDiff` seen: the commands
//! `diff.worktree` and `diff.index` return it for every file (field `digest`), and the backend,
//! after rereading the diff, recomputes it. If it does not match: [`EngineError::StaleDiff`] and
//! nothing is written.
//!
//! The digest covers the whole `FileDiff` (paths, modes, ids, hunks with numbers and line
//! contents), not just the ids: in the worktree diff the id of the new side is computed by git from the file,
//! but in the `MemoryBackend` it is fake, and a digest of the whole diff works for all backends in the same
//! way. The parameter is mandatory: without it, a selection made on an old diff
//! would be applied to different lines without anyone noticing, and whoever chooses the lines
//! has anyway just read the diff (the numbers come from there).

use super::DiffSpec;
use crate::{EngineError, Message};
use project_domain::{
    Change, Diff, DomainError, FileDiff, LineKind, LineRef, PatchDirection, RepoPath, build_patch,
};

/// The lines to stage or unstage: parameters of `index.stage_lines` and
/// `index.unstage_lines`, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSelection {
    /// The file, with the path used to ask for its diff (the new one; for a rename also the
    /// old one).
    pub path: RepoPath,
    /// The chosen lines, with the numbers of the seen diff (see [`LineRef`]). Not empty.
    pub lines: Vec<LineRef>,
    /// The [`file_digest`] of the `FileDiff` the lines come from.
    pub digest: String,
    /// [`PatchDirection::Stage`]: lines of `diff.worktree`; [`PatchDirection::Unstage`]: lines
    /// of `diff.index`.
    pub direction: PatchDirection,
}

impl LineSelection {
    /// The command id: `index.stage_lines` or `index.unstage_lines`.
    pub fn command_id(&self) -> &'static str {
        match self.direction {
            PatchDirection::Stage => "index.stage_lines",
            PatchDirection::Unstage => "index.unstage_lines",
        }
    }

    /// The diff the lines come from, limited to the file.
    pub fn diff_spec(&self) -> DiffSpec {
        let path = Some(self.path.clone());
        match self.direction {
            PatchDirection::Stage => DiffSpec::Worktree { path },
            PatchDirection::Unstage => DiffSpec::Index { path },
        }
    }
}

/// Identifies a [`FileDiff`]: 32 hex digits, equal only for diffs equal in every field
/// (in practice: a 128-bit FNV-1a hash of the JSON serialization). Returned by
/// `diff.worktree` and `diff.index` for every file (`digest`); a `FileDiff` reread from the JSON of
/// those commands gives the same digest. It is not a cryptographic hash: it serves to recognize a changed
/// diff, not to defend against someone deliberately building two diffs with the same digest.
pub fn file_digest(file: &FileDiff) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let json = serde_json::to_vec(file).expect("the domain types are serializable");
    let hash = json
        .iter()
        .fold(OFFSET, |h, &b| (h ^ u128::from(b)).wrapping_mul(PRIME));
    format!("{hash:032x}")
}

/// The chosen file and the patch to apply to the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePatch {
    /// The file of the reread diff, the one seen by whoever chose the lines.
    pub file: FileDiff,
    /// The patch from [`build_patch`], to apply to the index.
    pub patch: FileDiff,
}

impl LinePatch {
    /// The path the patch changes in the index: the index side of the diff (the old one for
    /// stage, the new one for unstage).
    pub fn target(&self) -> &RepoPath {
        &self.patch.new_path
    }

    /// The touched paths, for the backup and the recheck of the index: the requested one and the two
    /// of the file (different for a rename).
    pub fn touched(&self, selection: &LineSelection) -> Vec<RepoPath> {
        let mut paths = vec![selection.path.clone()];
        for p in [
            &self.file.old_path,
            &self.file.new_path,
            &self.patch.new_path,
        ] {
            if !paths.contains(p) {
                paths.push(p.clone());
            }
        }
        paths
    }
}

/// Picks in `diff` (the diff of [`LineSelection::diff_spec`] just reread) the selection's file,
/// checks that it is the one seen and builds the patch.
///
/// - The file is the one whose new path equals `path`; if none, the one with the old
///   path (a rename requested with the old name). Two files on the same path (type change,
///   e.g. a file that became a link: git writes it as a deletion plus a new file):
///   [`EngineError::NotAllowed`], stage the whole file.
/// - No file (the path has no more changes) or different digest:
///   [`EngineError::StaleDiff`].
/// - File not suitable for a line patch (binary, submodule, link, conflict, copy,
///   rename in the stage): [`EngineError::NotAllowed`].
/// - Lines that are not changed lines of the diff, or empty selection:
///   [`EngineError::InvalidParams`].
pub fn line_patch(diff: &Diff, selection: &LineSelection) -> Result<LinePatch, EngineError> {
    let no_file = match selection.direction {
        PatchDirection::Stage => msg!("engine-lines-no-unstaged"),
        PatchDirection::Unstage => msg!("engine-lines-no-staged"),
    };
    let file = pick_file(diff, &selection.path, &selection.digest, no_file)?;
    patch_of(
        file,
        &selection.path,
        &selection.lines,
        selection.direction,
        selection.command_id(),
    )
}

/// The lines to discard from the worktree (`worktree.discard_lines`): the chosen changed lines
/// of the `diff.worktree` diff of `path` go back to the index version, the others stay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscardLines {
    /// The file (a tracked file with unstaged changes, not renamed).
    pub path: RepoPath,
    /// The chosen lines, with the numbers of the seen `diff.worktree` diff. Not empty.
    pub lines: Vec<LineRef>,
    /// The [`file_digest`] of the `FileDiff` the lines come from.
    pub digest: String,
}

impl DiscardLines {
    /// The diff the lines come from: `diff.worktree`, limited to the file.
    pub fn diff_spec(&self) -> DiffSpec {
        DiffSpec::Worktree {
            path: Some(self.path.clone()),
        }
    }
}

/// [`line_patch`] for `worktree.discard_lines`: picks the file in `diff` (the reread
/// [`DiscardLines::diff_spec`]), checks the digest and builds the patch that, applied to the
/// **worktree** content (the new side of the diff), takes the selected lines back to the index
/// version: the [`PatchDirection::Unstage`] rule of [`build_patch`] (the worktree is the new side
/// of the diff, as the index is for `diff.index`). The patch's preimage is the clean worktree
/// content, its postimage the content to write.
///
/// Errors as [`line_patch`]; besides, only a modified file ([`Change::Modified`]: not a file
/// deleted from the worktree, `intent-to-add`, renamed): [`EngineError::NotAllowed`], discard the
/// whole file.
pub fn discard_line_patch(diff: &Diff, selection: &DiscardLines) -> Result<LinePatch, EngineError> {
    let path = &selection.path;
    let file = pick_file(
        diff,
        path,
        &selection.digest,
        msg!("engine-lines-no-unstaged"),
    )?;
    if file.change != Change::Modified || file.old_path != file.new_path {
        return Err(EngineError::NotAllowed(msg!(
            "engine-discard-lines-whole-file",
            path = path
        )));
    }
    patch_of(
        file,
        path,
        &selection.lines,
        PatchDirection::Unstage,
        "worktree.discard_lines",
    )
}

/// The file of `diff` at `path` (see [`line_patch`]), if its digest is `digest`.
fn pick_file<'a>(
    diff: &'a Diff,
    path: &RepoPath,
    digest: &str,
    no_file: Message,
) -> Result<&'a FileDiff, EngineError> {
    let by_new: Vec<&FileDiff> = diff.files.iter().filter(|f| f.new_path == *path).collect();
    let candidates = if by_new.is_empty() {
        diff.files.iter().filter(|f| f.old_path == *path).collect()
    } else {
        by_new
    };
    let file = match candidates[..] {
        [file] => file,
        [] => {
            return Err(EngineError::StaleDiff {
                path: path.to_string(),
                reason: no_file,
            });
        }
        _ => {
            return Err(EngineError::NotAllowed(msg!(
                "engine-lines-type-change",
                path = path
            )));
        }
    };
    if file_digest(file) != digest {
        return Err(EngineError::StaleDiff {
            path: path.to_string(),
            reason: msg!("engine-lines-diff-changed"),
        });
    }
    Ok(file)
}

/// The patch of `lines` of `file` in `direction`, anchored (see [`line_patch`]).
fn patch_of(
    file: &FileDiff,
    path: &RepoPath,
    lines: &[LineRef],
    direction: PatchDirection,
    id: &'static str,
) -> Result<LinePatch, EngineError> {
    let patch = build_patch(file, lines, direction).map_err(|e| match e {
        DomainError::UnsupportedPatch { .. } => {
            EngineError::NotAllowed(msg!("engine-other", text = e.to_string()))
        }
        e @ DomainError::InvalidLineSelection { .. } => EngineError::InvalidParams {
            id,
            reason: msg!("engine-other", text = e.to_string()),
        },
        e => EngineError::Domain(e),
    })?;
    let patch = patch.ok_or_else(|| EngineError::InvalidParams {
        id,
        reason: msg!("engine-lines-none-selected"),
    })?;
    check_anchored(&patch).map_err(|reason| {
        EngineError::NotAllowed(msg!(
            "engine-lines-not-anchored",
            path = path,
            reason = reason
        ))
    })?;
    Ok(LinePatch {
        file: file.clone(),
        patch,
    })
}

/// Each hunk of the patch says by itself where it goes: it has a context line at the top, or
/// starts at the beginning of the file; it has a context line at the bottom, or it is the last hunk (and
/// [`apply_patch`] checks that it reaches the end of the file). A hunk without context (e.g. from a
/// diff with `diff.context=0`) does not pin its position, and `git apply` might apply it
/// elsewhere (an insertion without trailing context ends up at the end of the file). With `DIFF_ARGS`
/// (`--unified=3`) the patches from [`build_patch`] always satisfy it: it is an extra defense.
fn check_anchored(patch: &FileDiff) -> Result<(), Message> {
    let last = patch.hunks.len().saturating_sub(1);
    for (i, h) in patch.hunks.iter().enumerate() {
        let at_start = if h.old_count == 0 {
            h.old_start == 0
        } else {
            h.old_start == 1
        };
        let context =
            |l: Option<&project_domain::Line>| l.is_some_and(|l| l.kind == LineKind::Context);
        if !context(h.lines.first()) && !at_start {
            return Err(msg!("engine-lines-no-head-context", line = h.old_start));
        }
        if !context(h.lines.last()) && i != last {
            return Err(msg!("engine-lines-no-tail-context", line = h.old_start));
        }
    }
    Ok(())
}

/// A line of a file with its newline (the last one may lack it).
fn split_lines(content: &[u8]) -> Vec<&[u8]> {
    content.split_inclusive(|&b| b == b'\n').collect()
}

/// Applies a patch from [`build_patch`] to `base` (`None` = file absent from the index, or
/// present only as an `intent-to-add` placeholder), strictly: each hunk applies
/// **only** at its position (`old_start`, never searched elsewhere as `git apply` does), the
/// context and removed lines must match byte for byte, newline included, and a hunk without
/// leading or trailing context must touch the start or the end of the file. Result `None`: the
/// file leaves the index. Both backends use it: the `CliBackend` writes the resulting
/// content to the index.
pub fn apply_patch(base: Option<&[u8]>, patch: &FileDiff) -> Result<Option<Vec<u8>>, Message> {
    let base = match (base, patch.change) {
        (Some(_), Change::Added) => return Err(msg!("engine-lines-patch-added-exists")),
        (None, Change::Added) => &[][..],
        (None, _) => return Err(msg!("engine-lines-patch-not-in-index")),
        (Some(b), _) => b,
    };
    let src = split_lines(base);
    let mut out: Vec<u8> = Vec::with_capacity(base.len());
    let mut pos = 0usize;
    for h in &patch.hunks {
        let start = h.old_start as usize;
        let first = if h.old_count == 0 {
            start
        } else {
            start
                .checked_sub(1)
                .ok_or(msg!("engine-lines-patch-hunk-zero"))?
        };
        if first < pos || first > src.len() {
            return Err(msg!("engine-lines-patch-misplaced", start = start));
        }
        for l in &src[pos..first] {
            out.extend_from_slice(l);
        }
        pos = first;
        let context =
            |l: Option<&project_domain::Line>| l.is_some_and(|l| l.kind == LineKind::Context);
        // A hunk without leading context must start at the beginning of the file, one without
        // trailing context must reach the end: otherwise its position is not verified.
        if !context(h.lines.first()) && first != 0 {
            return Err(msg!("engine-lines-patch-no-head-context", start = start));
        }
        for l in &h.lines {
            let mut bytes = l.content.as_bytes().to_vec();
            if !l.no_newline {
                bytes.push(b'\n');
            }
            if l.kind != LineKind::Added {
                if src.get(pos).copied() != Some(&bytes[..]) {
                    return Err(msg!("engine-lines-patch-mismatch", line = pos + 1));
                }
                pos += 1;
            }
            if l.kind != LineKind::Removed {
                if out.last().is_some_and(|&b| b != b'\n') {
                    return Err(msg!("engine-lines-patch-no-newline"));
                }
                out.extend_from_slice(&bytes);
            }
        }
        if !context(h.lines.last()) && pos != src.len() {
            return Err(msg!("engine-lines-patch-no-tail-context", start = start));
        }
    }
    for l in &src[pos..] {
        if out.last().is_some_and(|&b| b != b'\n') {
            return Err(msg!("engine-lines-patch-no-newline"));
        }
        out.extend_from_slice(l);
    }
    if patch.change == Change::Deleted {
        if !out.is_empty() {
            return Err(msg!("engine-lines-patch-delete-leftover"));
        }
        return Ok(None);
    }
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::parse_diff;

    fn diff(text: &str) -> Diff {
        parse_diff(text.as_bytes()).unwrap()
    }

    const MODIFIED: &str = "diff --git a/f b/f\n\
        index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
        --- a/f\n\
        +++ b/f\n\
        @@ -1,3 +1,3 @@\n\
        \x20a\n\
        -b\n\
        +B\n\
        \x20c\n";

    fn selection(diff: &Diff, lines: Vec<LineRef>, direction: PatchDirection) -> LineSelection {
        LineSelection {
            path: RepoPath::from("f"),
            lines,
            digest: file_digest(&diff.files[0]),
            direction,
        }
    }

    #[test]
    fn digest_identifies_the_whole_file_diff() {
        let a = diff(MODIFIED);
        let d = file_digest(&a.files[0]);
        assert_eq!(d.len(), 32);
        assert!(d.bytes().all(|b| b.is_ascii_hexdigit()));
        // Reread from the JSON it gives the same digest.
        let json = serde_json::to_value(&a.files[0]).unwrap();
        let back: FileDiff = serde_json::from_value(json).unwrap();
        assert_eq!(file_digest(&back), d);
        // A single different byte in a line is enough.
        let b = diff(&MODIFIED.replace("+B", "+C"));
        assert_ne!(file_digest(&b.files[0]), d);
        // Or a line number.
        let c = diff(&MODIFIED.replace("@@ -1,3 +1,3 @@", "@@ -2,3 +2,3 @@"));
        assert_ne!(file_digest(&c.files[0]), d);
    }

    #[test]
    fn patch_from_the_seen_diff() {
        let d = diff(MODIFIED);
        let sel = selection(&d, vec![LineRef::New(2)], PatchDirection::Stage);
        let p = line_patch(&d, &sel).unwrap();
        assert_eq!(p.target(), &RepoPath::from("f"));
        assert_eq!(
            apply_patch(Some(b"a\nb\nc\n"), &p.patch).unwrap().unwrap(),
            b"a\nb\nB\nc\n"
        );
        assert_eq!(p.touched(&sel), [RepoPath::from("f")]);
    }

    #[test]
    fn errors() {
        let d = diff(MODIFIED);
        // Different digest.
        let mut sel = selection(&d, vec![LineRef::New(2)], PatchDirection::Stage);
        sel.digest = "0".repeat(32);
        let err = line_patch(&d, &sel).unwrap_err();
        assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
        // No file.
        let err = line_patch(&Diff::default(), &sel).unwrap_err();
        assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");
        // Context line or nonexistent.
        for lines in [vec![LineRef::New(1)], vec![LineRef::Old(9)], vec![]] {
            let sel = selection(&d, lines, PatchDirection::Stage);
            let err = line_patch(&d, &sel).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");
        }
        // Binary.
        let bin = diff(
            "diff --git a/f b/f\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             Binary files a/f and b/f differ\n",
        );
        let sel = selection(&bin, vec![LineRef::New(1)], PatchDirection::Stage);
        let err = line_patch(&bin, &sel).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        // Two files on the same path (type change).
        let two = diff(
            "diff --git a/f b/f\n\
             deleted file mode 100644\n\
             index 1111111111111111111111111111111111111111..0000000000000000000000000000000000000000\n\
             --- a/f\n\
             +++ /dev/null\n\
             @@ -1 +0,0 @@\n\
             -a\n\
             diff --git a/f b/f\n\
             new file mode 120000\n\
             index 0000000000000000000000000000000000000000..2222222222222222222222222222222222222222\n\
             --- /dev/null\n\
             +++ b/f\n\
             @@ -0,0 +1 @@\n\
             +x\n\
             \\ No newline at end of file\n",
        );
        let sel = selection(&two, vec![LineRef::Old(1)], PatchDirection::Stage);
        let err = line_patch(&two, &sel).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }

    #[test]
    fn rename_by_old_path() {
        let d = diff(
            "diff --git a/old b/f\n\
             similarity index 80%\n\
             rename from old\n\
             rename to f\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             --- a/old\n\
             +++ b/f\n\
             @@ -1,2 +1,2 @@\n\
             \x20a\n\
             -b\n\
             +B\n",
        );
        let mut sel = selection(&d, vec![LineRef::New(2)], PatchDirection::Unstage);
        sel.path = RepoPath::from("old");
        let p = line_patch(&d, &sel).unwrap();
        assert_eq!(p.target(), &RepoPath::from("f"));
        assert_eq!(
            p.touched(&sel),
            [RepoPath::from("old"), RepoPath::from("f")]
        );
        assert_eq!(
            apply_patch(Some(b"a\nB\n"), &p.patch).unwrap().unwrap(),
            b"a\n"
        );
    }

    #[test]
    fn hunks_without_context_must_touch_the_ends_of_the_file() {
        // Zero-context diff (as with `diff.context=0`): an insertion after line 2.
        let insert = diff(
            "diff --git a/f b/f\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             --- a/f\n\
             +++ b/f\n\
             @@ -2,0 +3 @@\n\
             +NEW\n",
        );
        let sel = selection(&insert, vec![LineRef::New(3)], PatchDirection::Stage);
        let err = line_patch(&insert, &sel).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        assert!(err.to_string().contains("leading context"), "{err}");

        // First hunk at the start but without trailing context, and it is not the last.
        let two = diff(
            "diff --git a/f b/f\n\
             index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644\n\
             --- a/f\n\
             +++ b/f\n\
             @@ -1 +1 @@\n\
             -a\n\
             +A\n\
             @@ -3 +3 @@\n\
             -c\n\
             +C\n",
        );
        let sel = selection(&two, two.files[0].line_refs(), PatchDirection::Stage);
        let err = line_patch(&two, &sel).unwrap_err();
        assert!(err.to_string().contains("trailing context"), "{err}");
        // Only the second: no leading context, not at the start.
        let sel = selection(
            &two,
            vec![LineRef::Old(3), LineRef::New(3)],
            PatchDirection::Stage,
        );
        let err = line_patch(&two, &sel).unwrap_err();
        assert!(err.to_string().contains("leading context"), "{err}");
        // Only the first, which becomes the last: `line_patch` accepts it, but `apply_patch`
        // checks that it really reaches the end of the file.
        let sel = selection(
            &two,
            vec![LineRef::Old(1), LineRef::New(1)],
            PatchDirection::Stage,
        );
        let p = line_patch(&two, &sel).unwrap().patch;
        assert_eq!(apply_patch(Some(b"a\n"), &p).unwrap().unwrap(), b"A\n");
        let err = apply_patch(Some(b"a\nb\nc\n"), &p).unwrap_err();
        assert!(err.contains("end of the file"), "{err}");
    }

    #[test]
    fn apply_checks_the_preimage() {
        let d = diff(MODIFIED);
        let sel = selection(&d, vec![LineRef::Old(2)], PatchDirection::Stage);
        let p = line_patch(&d, &sel).unwrap().patch;
        assert_eq!(
            apply_patch(Some(b"a\nb\nc\n"), &p).unwrap().unwrap(),
            b"a\nc\n"
        );
        assert!(apply_patch(Some(b"a\nx\nc\n"), &p).is_err());
        assert!(apply_patch(Some(b"a\nb\r\nc\n"), &p).is_err());
        assert!(apply_patch(None, &p).is_err());
    }
}
