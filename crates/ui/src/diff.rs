//! File diff view: which diff to ask for (`diff.commit`, `diff.worktree` or
//! `diff.index`), which response to show and
//! how to turn it into lines to draw. Data only, no egui: tested with unit tests.
//!
//! Request rules (like the details panel):
//! - a diff is requested when the user opens a file, not on every frame;
//! - every request has a number; a response whose number is not the one of the open diff
//!   (the user opened another file, or closed the view) is discarded.

use crate::format::change_name;
use crate::lines::LineSelection;
use crate::{Tab, word_diff};
use project_domain::{Change, Diff, FileDiff, LineKind, Oid, RepoPath};
use project_engine::{DiffCommitParams, DiffParams};
use project_i18n::tr;
use std::collections::HashMap;
use std::ops::Range;

/// The actions of this area, dispatched by [`Tab::apply`](crate::Tab::apply).
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Opens a file's diff of the selected commit in place of the graph.
    Open(DiffTarget),
    /// Closes the diff and returns to the graph.
    Close,
    /// Asks again for the open diff after an error.
    Retry,
}

/// Where the diff of a file comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSource {
    /// The commit against its first parent (`diff.commit`).
    Commit(Oid),
    /// Unstaged changes, working tree against the index (`diff.worktree`).
    Worktree,
    /// Staged changes, index against HEAD (`diff.index`).
    Index,
}

/// The file to show the diff of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTarget {
    pub source: DiffSource,
    /// The new path of the file (for renames and copies, the destination).
    pub path: RepoPath,
}

impl DiffTarget {
    pub fn commit(commit: Oid, path: RepoPath) -> Self {
        Self {
            source: DiffSource::Commit(commit),
            path,
        }
    }

    /// The command and parameters that request this diff.
    pub fn request(&self) -> (&'static str, serde_json::Value) {
        let path = Some(self.path.clone());
        match self.source {
            DiffSource::Commit(id) => (
                "diff.commit",
                crate::params(DiffCommitParams {
                    id: id.to_string(),
                    path,
                }),
            ),
            DiffSource::Worktree => ("diff.worktree", crate::params(DiffParams { path })),
            DiffSource::Index => ("diff.index", crate::params(DiffParams { path })),
        }
    }
}

/// What the view knows about the open diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane<'a> {
    /// No diff open: the graph is in the center.
    Closed,
    Loading(&'a DiffTarget),
    Loaded(&'a DiffTarget, &'a DiffDoc),
    Failed(&'a DiffTarget, &'a str),
}

#[derive(Debug)]
struct Open {
    seq: u64,
    target: DiffTarget,
    answer: Option<Result<DiffDoc, String>>,
    /// Selected lines of the loaded document; cleared when a response brings a different diff.
    selection: LineSelection,
    /// Request `seq` re-reads the loaded diff: until it answers the old one stays on screen.
    reloading: bool,
}

#[derive(Debug, Default)]
pub struct DiffState {
    open: Option<Open>,
    next: u64,
}

impl DiffState {
    /// Opens the diff of `target` and returns the number of the request to send, if needed:
    /// not needed if that diff is already open (loaded or on its way).
    pub fn open(&mut self, target: DiffTarget) -> Option<u64> {
        if let Some(open) = &self.open
            && open.target == target
            && !matches!(open.answer, Some(Err(_)))
        {
            return None;
        }
        let seq = self.next;
        self.next += 1;
        self.open = Some(Open {
            seq,
            target,
            answer: None,
            selection: LineSelection::default(),
            reloading: false,
        });
        Some(seq)
    }

    /// After an error, requests the same diff again: returns the number of the new request.
    pub fn retry(&mut self) -> Option<u64> {
        let target = self.open.as_ref()?.target.clone();
        matches!(self.open.as_ref()?.answer, Some(Err(_)))
            .then(|| self.open(target))
            .flatten()
    }

    /// Requests the open diff even if it is already loaded (the file may have changed): returns
    /// the number of the new request. A loaded diff and its selection stay on screen until the
    /// response arrives (the automatic refresh on focus must not blank the view); after an
    /// error the view shows the loading state.
    pub fn reload(&mut self) -> Option<u64> {
        let open = self.open.as_mut()?;
        open.seq = self.next;
        open.reloading = matches!(open.answer, Some(Ok(_)));
        if !open.reloading {
            open.answer = None;
        }
        self.next += 1;
        Some(open.seq)
    }

    /// A loaded diff is on screen while a newer one is on its way: the lines must not be
    /// written from it.
    pub fn is_reloading(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.reloading)
    }

    /// Closes the view: incoming responses will be discarded.
    pub fn close(&mut self) {
        self.open = None;
    }

    pub fn target(&self) -> Option<&DiffTarget> {
        self.open.as_ref().map(|o| &o.target)
    }

    /// The response to request `seq`. Kept only if it is the one of the open diff:
    /// returns `false` if it was discarded.
    pub fn receive(&mut self, seq: u64, result: Result<Diff, String>) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        if open.seq != seq || (open.answer.is_some() && !open.reloading) {
            return false;
        }
        open.reloading = false;
        // The same diff again: document, selection and scroll stay as they are.
        if let (Some(Ok(doc)), Ok(diff)) = (&open.answer, &result)
            && doc.diff == *diff
        {
            return true;
        }
        open.answer = Some(result.map(|diff| DiffDoc::new(diff, &open.target.path)));
        open.selection.clear();
        true
    }

    /// The selected lines of the open diff (empty if there is none).
    pub fn selection(&self) -> Option<&LineSelection> {
        self.open.as_ref().map(|o| &o.selection)
    }

    /// The loaded diff with its selection, to modify.
    pub fn loaded_mut(&mut self) -> Option<(&DiffTarget, &DiffDoc, &mut LineSelection)> {
        let open = self.open.as_mut()?;
        match &open.answer {
            Some(Ok(doc)) => Some((&open.target, doc, &mut open.selection)),
            _ => None,
        }
    }

    /// The loaded diff with its selection.
    pub fn loaded(&self) -> Option<(&DiffTarget, &DiffDoc, &LineSelection)> {
        let open = self.open.as_ref()?;
        match &open.answer {
            Some(Ok(doc)) => Some((&open.target, doc, &open.selection)),
            _ => None,
        }
    }

    pub fn pane(&self) -> Pane<'_> {
        match &self.open {
            None => Pane::Closed,
            Some(Open { target, answer, .. }) => match answer {
                None => Pane::Loading(target),
                Some(Ok(doc)) => Pane::Loaded(target, doc),
                Some(Err(e)) => Pane::Failed(target, e),
            },
        }
    }
}

/// A view row: indices into the document's [`Diff`], so thousands of rows do not
/// copy the content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// Title of the file `files[f]`.
    File(usize),
    /// Information under the title (modes, binary, empty file...).
    Note(usize, String),
    /// Header of the hunk `files[f].hunks[h]`.
    Hunk(usize, usize),
    /// Row `files[f].hunks[h].lines[l]`.
    Line(usize, usize, usize),
    /// `\ No newline at end of file` after the previous row.
    NoNewline,
    /// Space between one file and the next.
    Gap,
}

/// A diff ready to draw: the rows in order, the column widths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffDoc {
    pub diff: Diff,
    pub rows: Vec<Row>,
    /// Digits of the line number columns.
    pub number_width: usize,
    /// Text columns of the widest row (after tabs and any cut).
    pub max_cols: usize,
    /// The changed parts of the paired removed and added lines, in columns, by
    /// `(file, hunk, line)` (see [`word_diff`]).
    pub emphasis: HashMap<(usize, usize, usize), Vec<Range<usize>>>,
    /// The rows of the side by side view.
    pub split: Vec<SplitRow>,
}

/// A row of the side by side view: the old file on the left, the new on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SplitRow {
    File(usize),
    Note(usize, String),
    Hunk(usize, usize),
    /// Lines `l` of hunk `h` of file `f`: removed or context on the left, added or context on
    /// the right; a side is empty where the other has more lines.
    Lines(usize, usize, Option<usize>, Option<usize>),
    Gap,
}

impl DiffDoc {
    /// `diff` is the response of `diff.commit` for `path`: the engine also returns the files
    /// that have `path` as source (e.g. a copy from `path`); the ones that arrive at
    /// `path` are kept (two for a type change), or all of them if none arrives there.
    pub fn new(mut diff: Diff, path: &RepoPath) -> Self {
        if diff.files.iter().any(|f| f.new_path == *path) {
            diff.files.retain(|f| f.new_path == *path);
        }
        let mut rows = Vec::new();
        let mut max_number = 0;
        let mut max_cols = 0;
        for (f, file) in diff.files.iter().enumerate() {
            if f > 0 {
                rows.push(Row::Gap);
            }
            rows.push(Row::File(f));
            rows.extend(file_notes(file).into_iter().map(|n| Row::Note(f, n)));
            for (h, hunk) in file.hunks.iter().enumerate() {
                rows.push(Row::Hunk(f, h));
                max_number = max_number
                    .max(hunk.old_start + hunk.old_count)
                    .max(hunk.new_start + hunk.new_count);
                for (l, line) in hunk.lines.iter().enumerate() {
                    rows.push(Row::Line(f, h, l));
                    let text = display_line(&line.content.to_string_lossy());
                    max_cols = max_cols.max(text.cols);
                    if line.no_newline {
                        rows.push(Row::NoNewline);
                    }
                }
            }
        }
        let emphasis = emphasis(&diff);
        let split = split_rows(&diff, &rows);
        Self {
            diff,
            rows,
            number_width: number_width(max_number),
            max_cols,
            emphasis,
            split,
        }
    }

    /// Lines added and removed in all files.
    pub fn line_counts(&self) -> (usize, usize) {
        self.diff
            .files
            .iter()
            .map(FileDiff::line_counts)
            .fold((0, 0), |(a, d), (fa, fd)| (a + fa, d + fd))
    }
}

/// The changed parts of every pair of removed and added lines, in columns.
fn emphasis(diff: &Diff) -> HashMap<(usize, usize, usize), Vec<Range<usize>>> {
    let mut out = HashMap::new();
    for (f, file) in diff.files.iter().enumerate() {
        for (h, hunk) in file.hunks.iter().enumerate() {
            let kinds: Vec<LineKind> = hunk.lines.iter().map(|l| l.kind).collect();
            for (old, new) in word_diff::pairs(&kinds) {
                let a = hunk.lines[old].content.to_string_lossy();
                let b = hunk.lines[new].content.to_string_lossy();
                if let Some((in_old, in_new)) = word_diff::changed(&a, &b) {
                    out.insert((f, h, old), columns(&a, &in_old));
                    out.insert((f, h, new), columns(&b, &in_new));
                }
            }
        }
    }
    out
}

/// The side by side rows: the unified rows, with each run of removed lines next to the run of
/// added lines after it.
fn split_rows(diff: &Diff, rows: &[Row]) -> Vec<SplitRow> {
    let mut out = Vec::new();
    for row in rows {
        match row {
            Row::File(f) => out.push(SplitRow::File(*f)),
            Row::Note(f, note) => out.push(SplitRow::Note(*f, note.clone())),
            Row::Gap => out.push(SplitRow::Gap),
            Row::Hunk(f, h) => {
                out.push(SplitRow::Hunk(*f, *h));
                let lines = &diff.files[*f].hunks[*h].lines;
                let mut l = 0;
                while l < lines.len() {
                    if lines[l].kind == LineKind::Context {
                        out.push(SplitRow::Lines(*f, *h, Some(l), Some(l)));
                        l += 1;
                        continue;
                    }
                    let removed: Vec<usize> = (l..lines.len())
                        .take_while(|&i| lines[i].kind == LineKind::Removed)
                        .collect();
                    let after = l + removed.len();
                    let added: Vec<usize> = (after..lines.len())
                        .take_while(|&i| lines[i].kind == LineKind::Added)
                        .collect();
                    for k in 0..removed.len().max(added.len()) {
                        out.push(SplitRow::Lines(
                            *f,
                            *h,
                            removed.get(k).copied(),
                            added.get(k).copied(),
                        ));
                    }
                    l = after + added.len();
                }
            }
            Row::Line(..) | Row::NoNewline => {}
        }
    }
    out
}

/// File title: the path, or `old -> new` for renames and copies (ASCII: the default fonts
/// have no arrows).
pub fn file_title(file: &FileDiff) -> String {
    match file.change {
        Change::Renamed | Change::Copied if file.old_path != file.new_path => format!(
            "{} -> {}",
            file.old_path.to_string_lossy(),
            file.new_path.to_string_lossy()
        ),
        _ => file.path().to_string_lossy().into_owned(),
    }
}

/// Kind of change, similarity and changed lines, e.g. `renamed 92%  +1 -1`.
pub fn file_summary(file: &FileDiff) -> String {
    let mut summary = file_kind(file);
    let (added, removed) = file.line_counts();
    if added + removed > 0 {
        summary.push_str(&format!("  +{added} -{removed}"));
    }
    summary
}

/// Kind of change and similarity, e.g. `renamed  92%`.
pub fn file_kind(file: &FileDiff) -> String {
    let mut parts = vec![change_name(file.change)];
    if let Some(similarity) = file.similarity {
        parts.push(format!("{similarity}%"));
    }
    if file.is_submodule() {
        parts.push(tr!("diff-submodule-badge"));
    }
    parts.join("  ")
}

/// The information rows under the title: changed modes and, if the file has no hunks, why.
pub fn file_notes(file: &FileDiff) -> Vec<String> {
    let mut notes = Vec::new();
    match (file.old_mode, file.new_mode) {
        (Some(old), Some(new)) if old != new => notes.push(tr!(
            "diff-note-mode",
            old = format!("{old:o}"),
            new = format!("{new:o}")
        )),
        (None, Some(new)) if file.change == Change::Added => {
            notes.push(tr!("diff-note-new-file", mode = format!("{new:o}")));
        }
        (Some(old), None) if file.change == Change::Deleted => {
            notes.push(tr!("diff-note-deleted-file", mode = format!("{old:o}")));
        }
        _ => {}
    }
    if file.is_submodule() {
        notes.push(tr!("diff-note-submodule"));
    }
    if !file.hunks.is_empty() {
        return notes;
    }
    let reason = if file.change == Change::Unmerged {
        tr!("diff-reason-unmerged")
    } else if file.binary {
        tr!("diff-reason-binary")
    } else {
        match file.change {
            Change::Renamed => tr!("diff-reason-exact-rename"),
            Change::Copied => tr!("diff-reason-exact-copy"),
            Change::Added => tr!("diff-reason-empty-file"),
            Change::Deleted => tr!("diff-reason-empty-file-deleted"),
            _ if file.old_mode != file.new_mode => tr!("diff-reason-mode-only"),
            _ => tr!("diff-reason-no-change"),
        }
    };
    notes.push(reason);
    notes
}

/// Spaces in a tab.
pub const TAB_WIDTH: usize = 4;
/// Columns shown at most per row: beyond that the row is cut and says so.
pub const MAX_LINE_COLS: usize = 1000;

/// The content of a row as it is to be drawn.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayLine {
    /// Pieces of text; `true` for the marks put in place of control characters (`^M` for
    /// `\r`), to be drawn dimmer.
    pub spans: Vec<(String, bool)>,
    /// Columns taken (one character, one column).
    pub cols: usize,
    /// Characters cut at the end because the row exceeds [`MAX_LINE_COLS`].
    pub cut: usize,
}

/// Makes a row readable: tabs expanded to the next multiple of [`TAB_WIDTH`], `\r` and
/// the other control characters as `^M`, `^[`...; beyond [`MAX_LINE_COLS`] columns the
/// row is cut.
pub fn display_line(content: &str) -> DisplayLine {
    let mut out = DisplayLine::default();
    let push = |out: &mut DisplayLine, text: &str, special: bool| match out.spans.last_mut() {
        Some((last, s)) if *s == special => last.push_str(text),
        _ => out.spans.push((text.to_owned(), special)),
    };
    let mut chars = content.chars();
    while let Some(c) = chars.next() {
        if out.cols >= MAX_LINE_COLS {
            out.cut = 1 + chars.count();
            break;
        }
        match c {
            '\t' => {
                let n = TAB_WIDTH - out.cols % TAB_WIDTH;
                push(&mut out, &" ".repeat(n), false);
                out.cols += n;
            }
            c if c.is_ascii_control() => {
                let shown = char::from((c as u8) ^ 0x40);
                push(&mut out, &format!("^{shown}"), true);
                out.cols += 2;
            }
            c => {
                let mut buf = [0; 4];
                push(&mut out, c.encode_utf8(&mut buf), false);
                out.cols += 1;
            }
        }
    }
    out
}

/// The char ranges `ranges` of `content` as the columns [`display_line`] puts them in (tabs
/// expanded, control characters two columns wide, nothing past the cut).
pub fn columns(content: &str, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut starts = Vec::with_capacity(content.len() + 1);
    let mut cols = 0;
    for c in content.chars() {
        starts.push(cols);
        cols += match c {
            '\t' => TAB_WIDTH - cols % TAB_WIDTH,
            c if c.is_ascii_control() => 2,
            _ => 1,
        };
    }
    starts.push(cols);
    ranges
        .iter()
        .filter_map(|r| {
            let start = *starts.get(r.start)?;
            let end = (*starts.get(r.end)?).min(MAX_LINE_COLS);
            (start < end).then_some(start..end)
        })
        .collect()
}

/// Digits needed to write `max` (at least 1).
pub fn number_width(max: u32) -> usize {
    max.max(1).ilog10() as usize + 1
}

/// Line number right-aligned to `width` digits, or spaces if the row does not exist on that side.
pub fn line_number(number: Option<u32>, width: usize) -> String {
    match number {
        Some(n) => format!("{n:>width$}"),
        None => " ".repeat(width),
    }
}

/// The sign of the row as git writes it: ` `, `+`, `-`.
pub fn line_prefix(kind: LineKind) -> char {
    char::from(kind.prefix())
}

impl Tab {
    /// The actions of this area, dispatched by [`Tab::apply`].
    pub(crate) fn apply_diff(&mut self, action: Action) {
        match action {
            Action::Open(target) => {
                if let Some(repo) = &mut self.repo {
                    repo.conflict.close();
                    repo.file.close();
                }
                if let Some(seq) = self.repo.as_mut().and_then(|r| r.diff.open(target.clone())) {
                    self.request_diff(&target, seq);
                }
            }
            Action::Close => {
                if let Some(repo) = &mut self.repo {
                    repo.diff.close();
                }
            }
            Action::Retry => {
                let repo = self.repo.as_mut();
                if let Some((seq, target)) = repo.and_then(|r| {
                    let seq = r.diff.retry()?;
                    Some((seq, r.diff.target()?.clone()))
                }) {
                    self.request_diff(&target, seq);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{ByteText, Hunk, Line};

    fn oid(n: u8) -> Oid {
        Oid::from_bytes([n; 20])
    }

    fn target(n: u8, path: &str) -> DiffTarget {
        DiffTarget {
            source: DiffSource::Commit(oid(n)),
            path: path.into(),
        }
    }

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>, text: &str) -> Line {
        Line {
            kind,
            old_line: old,
            new_line: new,
            content: ByteText::from(text),
            no_newline: false,
        }
    }

    fn file(change: Change, old: &str, new: &str, hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            old_path: old.into(),
            new_path: new.into(),
            change,
            similarity: None,
            old_mode: Some(0o100644),
            new_mode: Some(0o100644),
            old_oid: Some(oid(1)),
            new_oid: Some(oid(2)),
            binary: false,
            hunks,
        }
    }

    fn hunk() -> Hunk {
        let mut last = line(LineKind::Added, None, Some(10), "end");
        last.no_newline = true;
        Hunk {
            old_start: 8,
            old_count: 2,
            new_start: 8,
            new_count: 3,
            section: ByteText::from("fn main() {"),
            lines: vec![
                line(LineKind::Context, Some(8), Some(8), "a"),
                line(LineKind::Removed, Some(9), None, "b\tc"),
                line(LineKind::Added, None, Some(9), "B"),
                last,
            ],
        }
    }

    fn diff(files: Vec<FileDiff>) -> Diff {
        Diff { files }
    }

    #[test]
    fn opening_asks_once_and_keeps_the_answer() {
        let mut s = DiffState::default();
        assert_eq!(s.pane(), Pane::Closed);
        let seq = s.open(target(1, "a.rs")).expect("first request");
        assert_eq!(s.open(target(1, "a.rs")), None);
        assert_eq!(s.pane(), Pane::Loading(&target(1, "a.rs")));
        assert!(s.receive(
            seq,
            Ok(diff(vec![file(
                Change::Modified,
                "a.rs",
                "a.rs",
                vec![hunk()]
            )]))
        ));
        assert!(matches!(s.pane(), Pane::Loaded(t, _) if *t == target(1, "a.rs")));
        // A second response with the same number does not replace the first.
        assert!(!s.receive(seq, Err("late".to_owned())));
        assert_eq!(s.open(target(1, "a.rs")), None);
    }

    #[test]
    fn answers_for_another_file_or_after_closing_are_dropped() {
        let mut s = DiffState::default();
        let first = s.open(target(1, "a.rs")).unwrap();
        let second = s.open(target(1, "b.rs")).unwrap();
        assert_ne!(first, second);
        assert!(!s.receive(first, Ok(diff(Vec::new()))));
        assert_eq!(s.pane(), Pane::Loading(&target(1, "b.rs")));
        // Same path, different commit: it is another diff.
        let third = s.open(target(2, "b.rs")).unwrap();
        assert!(!s.receive(second, Ok(diff(Vec::new()))));
        s.close();
        assert!(!s.receive(third, Ok(diff(Vec::new()))));
        assert_eq!(s.pane(), Pane::Closed);
        assert_eq!(s.target(), None);
    }

    #[test]
    fn errors_are_shown_and_can_be_retried() {
        let mut s = DiffState::default();
        let seq = s.open(target(1, "a.rs")).unwrap();
        assert_eq!(s.retry(), None, "nothing to retry while loading");
        assert!(s.receive(seq, Err("unknown commit".to_owned())));
        assert_eq!(s.pane(), Pane::Failed(&target(1, "a.rs"), "unknown commit"));
        let again = s.retry().expect("new request");
        assert_ne!(again, seq);
        assert_eq!(s.pane(), Pane::Loading(&target(1, "a.rs")));
        // Reopening the same file after an error also requests it.
        assert!(s.receive(again, Err("again".to_owned())));
        assert!(s.open(target(1, "a.rs")).is_some());
    }

    #[test]
    fn rows_follow_files_hunks_and_lines() {
        let doc = DiffDoc::new(
            diff(vec![file(Change::Modified, "a.rs", "a.rs", vec![hunk()])]),
            &"a.rs".into(),
        );
        assert_eq!(
            doc.rows,
            vec![
                Row::File(0),
                Row::Hunk(0, 0),
                Row::Line(0, 0, 0),
                Row::Line(0, 0, 1),
                Row::Line(0, 0, 2),
                Row::Line(0, 0, 3),
                Row::NoNewline,
            ]
        );
        assert_eq!(doc.number_width, 2, "up to line 11");
        // `b\tc`: the tab brings `c` to column 4.
        assert_eq!(doc.max_cols, 5);
        assert_eq!(doc.line_counts(), (2, 1));
        let header = String::from_utf8(doc.diff.files[0].hunks[0].header()).unwrap();
        assert_eq!(header, "@@ -8,2 +8,3 @@ fn main() {");
    }

    #[test]
    fn only_files_that_end_at_the_path_are_kept() {
        // The engine also returns the copy that starts from `a.rs`.
        let copy = file(Change::Copied, "a.rs", "b.rs", vec![hunk()]);
        let modified = file(Change::Modified, "a.rs", "a.rs", vec![hunk()]);
        let doc = DiffDoc::new(diff(vec![modified.clone(), copy]), &"a.rs".into());
        assert_eq!(doc.diff.files, vec![modified]);
        // A type change: two files on the same path, kept separate.
        let deleted = file(Change::Deleted, "l", "l", Vec::new());
        let added = file(Change::Added, "l", "l", Vec::new());
        let doc = DiffDoc::new(diff(vec![deleted, added]), &"l".into());
        assert_eq!(doc.diff.files.len(), 2);
        assert!(doc.rows.contains(&Row::Gap));
        assert_eq!(doc.number_width, 1);
    }

    #[test]
    fn titles_and_notes() {
        let mut renamed = file(
            Change::Renamed,
            "src/main.rs",
            "src/bin/app.rs",
            vec![hunk()],
        );
        renamed.similarity = Some(92);
        assert_eq!(file_title(&renamed), "src/main.rs -> src/bin/app.rs");
        assert_eq!(file_summary(&renamed), "renamed  92%  +2 -1");
        assert_eq!(file_kind(&renamed), "renamed  92%");
        assert!(file_notes(&renamed).is_empty());

        renamed.hunks.clear();
        renamed.old_mode = None;
        renamed.new_mode = None;
        assert_eq!(file_notes(&renamed), ["exact rename: content unchanged"]);

        let mut mode = file(Change::Modified, "run.sh", "run.sh", Vec::new());
        mode.new_mode = Some(0o100755);
        assert_eq!(file_title(&mode), "run.sh");
        assert_eq!(
            file_notes(&mode),
            [
                "mode 100644 -> 100755",
                "mode change only: content unchanged"
            ]
        );

        let mut empty = file(Change::Deleted, ".gitkeep", ".gitkeep", Vec::new());
        empty.new_mode = None;
        assert_eq!(
            file_notes(&empty),
            ["deleted file, mode 100644", "empty file deleted"]
        );

        let mut binary = file(Change::Added, "logo.png", "logo.png", Vec::new());
        binary.old_mode = None;
        binary.binary = true;
        assert_eq!(
            file_notes(&binary),
            [
                "new file, mode 100644",
                "binary file: the content is not shown"
            ]
        );

        let mut sub = file(Change::Modified, "vendor/lib", "vendor/lib", vec![hunk()]);
        sub.old_mode = Some(0o160000);
        sub.new_mode = Some(0o160000);
        assert!(file_summary(&sub).contains("[submodule]"));
        assert_eq!(file_notes(&sub).len(), 1);

        let mut conflict = file(Change::Unmerged, "x", "x", Vec::new());
        conflict.old_mode = None;
        conflict.new_mode = None;
        assert!(file_notes(&conflict)[0].starts_with("unmerged file"));
    }

    #[test]
    fn tabs_and_control_characters_are_readable() {
        let plain = display_line("let x = 1;");
        assert_eq!(plain.spans, [("let x = 1;".to_owned(), false)]);
        assert_eq!((plain.cols, plain.cut), (10, 0));

        // Tab up to the next multiple of 4, from the column where it is.
        assert_eq!(display_line("\tx").spans, [("    x".to_owned(), false)]);
        assert_eq!(display_line("ab\tx").spans, [("ab  x".to_owned(), false)]);
        assert_eq!(display_line("abcd\tx").cols, 9);

        // The `\r` of a CRLF row and the other control characters are visible.
        let crlf = display_line("end\r");
        assert_eq!(
            crlf.spans,
            [("end".to_owned(), false), ("^M".to_owned(), true)]
        );
        assert_eq!(crlf.cols, 5);
        assert_eq!(display_line("\u{1b}[0m").spans[0], ("^[".to_owned(), true));

        // Non-ASCII characters: one column each.
        assert_eq!(display_line("naïve").cols, 5);
        // Non-UTF-8 content: the replacement character.
        let lossy = ByteText::from_bytes(b"a\xffb".to_vec());
        assert_eq!(
            display_line(&lossy.to_string_lossy()).spans[0].0,
            "a\u{fffd}b"
        );
    }

    #[test]
    fn long_lines_are_cut() {
        let long = "x".repeat(MAX_LINE_COLS + 25);
        let shown = display_line(&long);
        assert_eq!(shown.cols, MAX_LINE_COLS);
        assert_eq!(shown.cut, 25);
        assert_eq!(display_line(&"y".repeat(MAX_LINE_COLS)).cut, 0);
    }

    #[test]
    fn line_numbers() {
        assert_eq!(number_width(0), 1);
        assert_eq!(number_width(9), 1);
        assert_eq!(number_width(10), 2);
        assert_eq!(number_width(12_345), 5);
        assert_eq!(line_number(Some(7), 3), "  7");
        assert_eq!(line_number(None, 3), "   ");
        assert_eq!(line_number(Some(1234), 3), "1234");
        assert_eq!(line_prefix(LineKind::Added), '+');
        assert_eq!(line_prefix(LineKind::Removed), '-');
        assert_eq!(line_prefix(LineKind::Context), ' ');
    }

    #[test]
    fn reload_asks_again_for_the_same_file() {
        let mut state = DiffState::default();
        assert_eq!(state.reload(), None, "nothing to reload");
        let first = state.open(target(1, "a")).unwrap();
        assert!(state.receive(first, Ok(Diff { files: Vec::new() })));
        let second = state.reload().unwrap();
        assert_ne!(first, second);
        assert!(
            matches!(state.pane(), Pane::Loaded(..)),
            "the old diff stays"
        );
        assert!(state.is_reloading());
        // The response of the old request no longer counts.
        assert!(!state.receive(first, Ok(Diff { files: Vec::new() })));
        assert!(state.receive(second, Ok(Diff { files: Vec::new() })));
        assert!(matches!(state.pane(), Pane::Loaded(..)));
        assert!(!state.is_reloading());
    }

    #[test]
    fn reload_after_an_error_shows_the_loading_state() {
        let mut state = DiffState::default();
        let first = state.open(target(1, "a")).unwrap();
        assert!(state.receive(first, Err("boom".into())));
        state.reload().unwrap();
        assert!(matches!(state.pane(), Pane::Loading(_)));
        assert!(!state.is_reloading());
    }

    #[test]
    fn a_new_answer_clears_the_line_selection() {
        let mut state = DiffState::default();
        assert!(state.loaded().is_none());
        let seq = state.open(target(1, "a.rs")).unwrap();
        assert!(
            state.loaded_mut().is_none(),
            "nothing to select while loading"
        );
        let one = || diff(vec![file(Change::Modified, "a.rs", "a.rs", vec![hunk()])]);
        assert!(state.receive(seq, Ok(one())));
        let (_, doc, selection) = state.loaded_mut().unwrap();
        let doc = doc.clone();
        selection.click(&doc, 3, false);
        assert_eq!(state.selection().map(LineSelection::len), Some(1));
        let again = state.reload().unwrap();
        assert_eq!(
            state.selection().map(LineSelection::len),
            Some(1),
            "reload: the selection stays while the old diff is shown"
        );
        assert!(state.receive(again, Ok(one())));
        assert_eq!(
            state.selection().map(LineSelection::len),
            Some(1),
            "same diff: selection kept"
        );
        let changed = state.reload().unwrap();
        let two = diff(vec![file(Change::Added, "a.rs", "a.rs", vec![hunk()])]);
        assert!(state.receive(changed, Ok(two)));
        assert!(
            state.selection().unwrap().is_empty(),
            "different diff: selection cleared"
        );
        let (_, doc, selection) = state.loaded_mut().unwrap();
        let doc = doc.clone();
        selection.click(&doc, 3, false);
        // Another file: new selection.
        state.open(target(1, "b.rs")).unwrap();
        assert!(state.selection().unwrap().is_empty());
    }

    #[test]
    fn each_source_asks_its_command() {
        let path = RepoPath::from("src/lib.rs");
        let commit = DiffTarget::commit(oid(3), path.clone());
        let (command, params) = commit.request();
        assert_eq!(command, "diff.commit");
        assert_eq!(
            params,
            serde_json::json!({ "id": oid(3).to_string(), "path": "src/lib.rs" })
        );
        for (source, expected) in [
            (DiffSource::Worktree, "diff.worktree"),
            (DiffSource::Index, "diff.index"),
        ] {
            let target = DiffTarget {
                source,
                path: path.clone(),
            };
            let (command, params) = target.request();
            assert_eq!(command, expected);
            assert_eq!(params, serde_json::json!({ "path": "src/lib.rs" }));
        }
    }

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "the expected ranges, not a range of values"
    )]
    fn char_ranges_become_display_columns() {
        // The tab takes the columns up to 4, the control character two.
        assert_eq!(columns("a\tb", &[2..3]), vec![4..5]);
        assert_eq!(columns("\u{1b}x", &[1..2]), vec![2..3]);
        assert_eq!(columns("abc", &[0..1, 3..9]), [0..1]);
    }

    #[test]
    fn split_rows_put_removed_lines_next_to_the_added_ones() {
        // a | b\tc -> B | end (added, no newline)
        let doc = DiffDoc::new(
            diff(vec![file(Change::Modified, "a.rs", "a.rs", vec![hunk()])]),
            &"a.rs".into(),
        );
        assert_eq!(
            doc.split,
            [
                SplitRow::File(0),
                SplitRow::Hunk(0, 0),
                SplitRow::Lines(0, 0, Some(0), Some(0)),
                SplitRow::Lines(0, 0, Some(1), Some(2)),
                SplitRow::Lines(0, 0, None, Some(3)),
            ]
        );
    }

    #[test]
    #[expect(
        clippy::single_range_in_vec_init,
        reason = "the expected ranges, not a range of values"
    )]
    fn paired_lines_mark_their_changed_words() {
        let hunk = Hunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            section: ByteText::from(""),
            lines: vec![
                line(LineKind::Removed, Some(1), None, "\tlet total = price * 2;"),
                line(LineKind::Added, None, Some(1), "\tlet total = price * qty;"),
            ],
        };
        let doc = DiffDoc::new(
            diff(vec![file(Change::Modified, "a.rs", "a.rs", vec![hunk])]),
            &"a.rs".into(),
        );
        // After the tab (4 columns): `let total = price * ` is 20 columns.
        assert_eq!(doc.emphasis[&(0, 0, 0)], [24..25]);
        assert_eq!(doc.emphasis[&(0, 0, 1)], [24..27]);
    }
}
