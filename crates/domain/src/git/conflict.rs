//! Parser of conflict markers in the worktree file and resolution of the blocks.
//!
//! [`parse_conflicts`] splits the bytes of the file into common text and conflict blocks
//! ([`ConflictText`]); [`resolve_block`] and [`resolve_all`] replace one or all the blocks
//! with the chosen version ([`Choice`]). Everything works on bytes: nothing is lost if the file is not
//! UTF-8.
//!
//! # Markers
//!
//! A marker is at the start of a line and is made of exactly `marker_size` equal characters
//! (7 by default, `conflict-marker-size` attribute), followed by end of line or by a space and
//! a label:
//!
//! ```text
//! <<<<<<< ours          start, label of our side (e.g. `HEAD`)
//! our lines
//! ||||||| base          only with `merge.conflictStyle=diff3`/`zdiff3`
//! base lines
//! =======
//! their lines
//! >>>>>>> theirs        end, label of their side (e.g. the merged branch)
//! ```
//!
//! - A line with more characters (`<<<<<<<<` with `marker_size` 7) or with anything after the characters
//!   without a space (`=======x`) is text.
//! - Line endings `\n` or `\r\n`, preserved as they were; the last line may have no newline.
//! - Outside a block, a line that looks like a middle or end marker (`|||||||`,
//!   `=======`, `>>>>>>>`) is plain text: it may be a Markdown setext heading or any
//!   line of a real file.
//! - Inside a block the markers must come in the order `<` (`|`) `=` `>`: an out-of-order
//!   marker (a second `<<<<<<<`, `>>>>>>>` before `=======`, ...) or a block left unclosed at
//!   the end of the file gives [`DomainError::InvalidConflict`]. In that case the editor shows the three
//!   versions and the file as it is, without blocks.

use crate::DomainError;
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// The worktree file split into common text and conflict blocks, in file order.
///
/// Concatenating the segments, with each block replaced by the bytes of the text in its
/// [`ConflictBlock::range`], gives back the file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConflictText {
    pub segments: Vec<Segment>,
}

impl ConflictText {
    /// The conflict blocks, in file order. The index of a block here is the one to
    /// pass to [`resolve_block`].
    pub fn blocks(&self) -> impl Iterator<Item = &ConflictBlock> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Conflict(block) => Some(block),
            Segment::Common(_) => None,
        })
    }

    /// Number of conflict blocks.
    pub fn block_count(&self) -> usize {
        self.blocks().count()
    }
}

/// A piece of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Segment {
    /// Bytes outside the blocks, newlines included. Never empty.
    Common(Vec<u8>),
    Conflict(ConflictBlock),
}

/// A `<<<<<<<` ... `>>>>>>>` block.
///
/// The parts are the bytes of the lines between the markers, newlines included (empty if the side has no
/// lines); the labels are the text after the marker without the leading space and without the
/// newline, empty if missing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictBlock {
    pub ours: Vec<u8>,
    /// The lines between `|||||||` and `=======`; `None` if the block has no base marker
    /// (`merge` style).
    pub base: Option<Vec<u8>>,
    pub theirs: Vec<u8>,
    pub ours_label: Vec<u8>,
    /// Empty also if the block has no base.
    pub base_label: Vec<u8>,
    pub theirs_label: Vec<u8>,
    /// Byte range of the block in the text read, from the initial `<` to the newline of the
    /// `>>>>>>>` marker included (or to the end of the text if that line has no newline).
    pub range: Range<usize>,
}

/// What to replace a block with. In JSON: `"ours"`, `"theirs"`, `"base"`,
/// `"ours_then_theirs"`, `"theirs_then_ours"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    /// Our lines.
    Ours,
    /// Their lines.
    Theirs,
    /// The base lines: only for a block that has one (`diff3`/`zdiff3` style).
    Base,
    /// Ours and then theirs.
    OursThenTheirs,
    /// Theirs and then ours.
    TheirsThenOurs,
}

impl ConflictBlock {
    /// The bytes that replace the block with the given choice.
    ///
    /// [`Choice::Base`] on a block without a base gives [`DomainError::InvalidConflict`].
    pub fn resolved(&self, choice: Choice) -> Result<Vec<u8>, DomainError> {
        Ok(match choice {
            Choice::Ours => self.ours.clone(),
            Choice::Theirs => self.theirs.clone(),
            Choice::Base => self.base.clone().ok_or_else(|| invalid(
                "the block has no base version (`merge.conflictStyle=diff3` or `zdiff3` is needed)",
            ))?,
            Choice::OursThenTheirs => [self.ours.as_slice(), &self.theirs].concat(),
            Choice::TheirsThenOurs => [self.theirs.as_slice(), &self.ours].concat(),
        })
    }
}

/// The kind of marker, from its character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    Start,
    Base,
    Separator,
    End,
}

impl Marker {
    fn text(self) -> &'static str {
        match self {
            Marker::Start => "<<<<<<<",
            Marker::Base => "|||||||",
            Marker::Separator => "=======",
            Marker::End => ">>>>>>>",
        }
    }
}

/// If the line (without newline) is a marker, its kind and label.
fn marker(line: &[u8], size: usize) -> Option<(Marker, &[u8])> {
    let kind = match line.first()? {
        b'<' => Marker::Start,
        b'|' => Marker::Base,
        b'=' => Marker::Separator,
        b'>' => Marker::End,
        _ => return None,
    };
    let c = line[0];
    if line.len() < size || !line[..size].iter().all(|&b| b == c) {
        return None;
    }
    match line.get(size) {
        None => Some((kind, &[])),
        Some(b' ') => Some((kind, &line[size + 1..])),
        Some(_) => None,
    }
}

/// The line without the trailing `\n` or `\r\n`.
fn strip_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn invalid(reason: impl Into<String>) -> DomainError {
    DomainError::InvalidConflict {
        reason: reason.into(),
    }
}

/// Where the parser is inside an open block.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Ours,
    Base,
    Theirs,
}

/// Splits the worktree file into common text and conflict blocks.
///
/// `marker_size` is the length of the markers (7, or the `conflict-marker-size` attribute of the
/// file); it must be at least 1. A block left unclosed or with out-of-order markers gives
/// [`DomainError::InvalidConflict`] with the line number (from 1). A file without blocks gives
/// a single [`Segment::Common`] (none if it is empty).
pub fn parse_conflicts(text: &[u8], marker_size: usize) -> Result<ConflictText, DomainError> {
    if marker_size == 0 {
        return Err(invalid("the marker length must be at least 1"));
    }
    let mut segments = Vec::new();
    // Start of the common text not yet saved.
    let mut common_start = 0;
    // The open block, the part we are in and the line of its `<<<<<<<`.
    let mut open: Option<(ConflictBlock, Part, usize)> = None;
    let mut offset = 0;
    for (index, line) in text.split_inclusive(|&b| b == b'\n').enumerate() {
        let number = index + 1;
        let start = offset;
        offset += line.len();
        let found = marker(strip_eol(line), marker_size);
        let Some((mut block, part, opened_at)) = open.take() else {
            if let Some((Marker::Start, label)) = found {
                if common_start < start {
                    segments.push(Segment::Common(text[common_start..start].to_vec()));
                }
                let block = ConflictBlock {
                    ours: Vec::new(),
                    base: None,
                    theirs: Vec::new(),
                    ours_label: label.to_vec(),
                    base_label: Vec::new(),
                    theirs_label: Vec::new(),
                    range: start..start,
                };
                open = Some((block, Part::Ours, number));
            }
            // Outside a block every other line, even if it looks like a marker, is text.
            continue;
        };
        let next = match (part, found) {
            (_, None) => {
                let dest = match part {
                    Part::Ours => &mut block.ours,
                    Part::Base => block.base.get_or_insert_with(Vec::new),
                    Part::Theirs => &mut block.theirs,
                };
                dest.extend_from_slice(line);
                part
            }
            (Part::Ours, Some((Marker::Base, label))) => {
                block.base = Some(Vec::new());
                block.base_label = label.to_vec();
                Part::Base
            }
            (Part::Ours | Part::Base, Some((Marker::Separator, _))) => Part::Theirs,
            (Part::Theirs, Some((Marker::End, label))) => {
                block.theirs_label = label.to_vec();
                block.range.end = offset;
                segments.push(Segment::Conflict(block));
                common_start = offset;
                continue;
            }
            (_, Some((kind, _))) => {
                let expected = match part {
                    Part::Ours => "`|||||||` or `=======`",
                    Part::Base => "`=======`",
                    Part::Theirs => "`>>>>>>>`",
                };
                return Err(invalid(format!(
                    "marker `{}` out of order at line {number} in the block opened at line \
                     {opened_at}: expected {expected}",
                    kind.text()
                )));
            }
        };
        open = Some((block, next, opened_at));
    }
    if let Some((_, _, opened_at)) = open {
        return Err(invalid(format!(
            "the block opened at line {opened_at} is not closed by `>>>>>>>`"
        )));
    }
    if common_start < text.len() {
        segments.push(Segment::Common(text[common_start..].to_vec()));
    }
    Ok(ConflictText { segments })
}

/// The text with block number `index` (from 0, in [`ConflictText::blocks`]) replaced by the
/// choice; the other blocks, resolved or not, stay as they are.
///
/// `text` must be the one read by [`parse_conflicts`] to obtain `parsed`: a nonexistent
/// index, or a range that does not fit in the text, gives [`DomainError::InvalidConflict`].
pub fn resolve_block(
    text: &[u8],
    parsed: &ConflictText,
    index: usize,
    choice: Choice,
) -> Result<Vec<u8>, DomainError> {
    let count = parsed.block_count();
    let block = parsed.blocks().nth(index).ok_or_else(|| {
        invalid(format!(
            "block {index} does not exist: the file has {count} conflict blocks"
        ))
    })?;
    replace(text, [(block, choice)])
}

/// The text with all the blocks replaced by the same choice.
///
/// As for [`resolve_block`], `text` must be the one read by [`parse_conflicts`]. With
/// [`Choice::Base`] all the blocks need to have the base.
pub fn resolve_all(
    text: &[u8],
    parsed: &ConflictText,
    choice: Choice,
) -> Result<Vec<u8>, DomainError> {
    replace(text, parsed.blocks().map(|b| (b, choice)))
}

/// Copies `text` replacing the ranges of the given blocks (in order) with their choice.
fn replace<'a>(
    text: &[u8],
    blocks: impl IntoIterator<Item = (&'a ConflictBlock, Choice)>,
) -> Result<Vec<u8>, DomainError> {
    let mut out = Vec::with_capacity(text.len());
    let mut pos = 0;
    for (block, choice) in blocks {
        let Range { start, end } = block.range.clone();
        if start < pos || end <= start || end > text.len() || text[start] != b'<' {
            return Err(invalid(format!(
                "the range {start}..{end} of the block does not match the text ({} bytes): did the \
                 file change after it was read?",
                text.len()
            )));
        }
        out.extend_from_slice(&text[pos..start]);
        out.extend_from_slice(&block.resolved(choice)?);
        pos = end;
    }
    out.extend_from_slice(&text[pos..]);
    Ok(out)
}
