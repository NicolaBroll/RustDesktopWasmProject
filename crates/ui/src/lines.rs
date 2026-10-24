//! Stage and unstage of hunks and single lines from the diff view (`index.stage_lines`,
//! `index.unstage_lines`): which lines are selected, whether the file allows partial
//! selection, the references to send to the engine and the state of the buttons. Data only, no egui:
//! tested with unit tests.
//!
//! Lines are identified by the index of the view row ([`DiffDoc::rows`]): they are valid for the
//! loaded document, and the selection is cleared on every new document (after a write, a
//! reload or another file). Only added and removed lines can be selected, never context.

use crate::diff::{DiffDoc, DiffSource, DiffTarget, Row};
use crate::{Action, Drag, Request, Tab, lines};
use project_domain::{DomainError, FileDiff, LineRef, PatchDirection, RepoPath, build_patch};
use project_i18n::tr;
use std::collections::BTreeSet;

/// Whether the shown diff allows staging (or unstaging) hunks and lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Partial {
    /// No buttons: diff of a commit, or no changed line.
    Hidden,
    /// The file cannot be split into lines: the reason, to show.
    Unavailable(String),
    /// Hunks and lines can be moved in this direction.
    Available(PatchDirection),
}

/// The direction for the diff of `source`: worktree → stage, index → unstage.
pub fn direction(source: DiffSource) -> Option<PatchDirection> {
    match source {
        DiffSource::Worktree => Some(PatchDirection::Stage),
        DiffSource::Index => Some(PatchDirection::Unstage),
        DiffSource::Commit(_) => None,
    }
}

/// Decides whether the diff `doc` of `source` allows partial selection, with the same rules as the
/// domain's patch builder ([`build_patch`] refuses binaries, submodules, links, conflicts,
/// copies and renames in the stage).
pub fn partial(doc: &DiffDoc, source: DiffSource) -> Partial {
    let Some(direction) = direction(source) else {
        return Partial::Hidden;
    };
    let file = match &doc.diff.files[..] {
        [] => return Partial::Hidden,
        [file] => file,
        _ => {
            return Partial::Unavailable(tr!("diff-lines-type-changed"));
        }
    };
    match build_patch(file, &[], direction) {
        Err(DomainError::UnsupportedPatch { reason, .. }) => Partial::Unavailable(reason),
        Err(e) => Partial::Unavailable(e.to_string()),
        Ok(_) if file.line_refs().is_empty() => Partial::Hidden,
        Ok(_) => Partial::Available(direction),
    }
}

/// The file that lines and hunks refer to: the only one of the document.
fn single_file(doc: &DiffDoc) -> Option<&FileDiff> {
    match &doc.diff.files[..] {
        [file] => Some(file),
        _ => None,
    }
}

/// The reference of the view row `row`, if it is a changed row.
pub fn line_ref(doc: &DiffDoc, row: usize) -> Option<LineRef> {
    match doc.rows.get(row)? {
        Row::Line(f, h, l) => doc
            .diff
            .files
            .get(*f)?
            .hunks
            .get(*h)?
            .lines
            .get(*l)?
            .line_ref(),
        _ => None,
    }
}

/// True if row `row` can be selected: an added or removed row.
pub fn selectable(doc: &DiffDoc, row: usize) -> bool {
    line_ref(doc, row).is_some()
}

/// The selected lines of a diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineSelection {
    rows: BTreeSet<usize>,
    /// The last row clicked without Shift: a Shift+click selects from here.
    anchor: Option<usize>,
    /// During a drag: `true` if it selects, `false` if it deselects.
    drag: Option<bool>,
}

impl LineSelection {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn contains(&self, row: usize) -> bool {
        self.rows.contains(&row)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Click on a row: selects or deselects it. With Shift it selects all the rows
    /// changed from the last clicked to this one, inclusive. Context rows are ignored.
    pub fn click(&mut self, doc: &DiffDoc, row: usize, shift: bool) {
        if !selectable(doc, row) {
            return;
        }
        match self.anchor {
            Some(anchor) if shift => self.set_range(doc, anchor, row, true),
            _ => {
                if !self.rows.remove(&row) {
                    self.rows.insert(row);
                }
                self.anchor = Some(row);
            }
        }
    }

    /// Start of a drag on row `row`: if the row was not selected the
    /// drag selects, otherwise it deselects.
    pub fn drag_start(&mut self, doc: &DiffDoc, row: usize) {
        if !selectable(doc, row) {
            return;
        }
        let select = !self.rows.contains(&row);
        self.drag = Some(select);
        self.anchor = Some(row);
        self.set_range(doc, row, row, select);
    }

    /// The drag is on row `row`: the rows from the start to here take the state
    /// chosen at the start.
    pub fn drag_over(&mut self, doc: &DiffDoc, row: usize) {
        if let (Some(select), Some(anchor)) = (self.drag, self.anchor) {
            self.set_range(doc, anchor, row, select);
        }
    }

    pub fn drag_end(&mut self) {
        self.drag = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn set_range(&mut self, doc: &DiffDoc, a: usize, b: usize, select: bool) {
        let (from, to) = (a.min(b), a.max(b).min(doc.rows.len().saturating_sub(1)));
        for row in (from..=to).filter(|&r| selectable(doc, r)) {
            if select {
                self.rows.insert(row);
            } else {
                self.rows.remove(&row);
            }
        }
    }

    /// The references of the selected lines, in diff order.
    pub fn refs(&self, doc: &DiffDoc) -> Vec<LineRef> {
        self.rows.iter().filter_map(|&r| line_ref(doc, r)).collect()
    }
}

/// What to move: a whole hunk or the selected lines.
#[derive(Debug, Clone, Copy)]
pub enum Pick<'a> {
    /// The hunk with this index in the file.
    Hunk(usize),
    Selected(&'a LineSelection),
}

/// A line write to send to the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineWrite {
    pub direction: PatchDirection,
    pub path: RepoPath,
    /// Never empty.
    pub lines: Vec<LineRef>,
    /// The digest of the `FileDiff` seen (the same as the `digest` field of `diff.worktree`/
    /// `diff.index`).
    pub digest: String,
    /// Description for messages, e.g. "Stage hunk 1 of src/lib.rs".
    pub label: String,
}

impl LineWrite {
    /// The id of the Registry command.
    pub fn command(&self) -> &'static str {
        match self.direction {
            PatchDirection::Stage => "index.stage_lines",
            PatchDirection::Unstage => "index.unstage_lines",
        }
    }

    pub fn params(&self) -> serde_json::Value {
        serde_json::json!({
            "path": self.path,
            "lines": self.lines,
            "digest": self.digest,
        })
    }
}

/// The write for `pick` in the diff `doc` of `target`; `None` if the diff does not allow
/// partial selection or there is nothing to move (never an empty list of lines).
pub fn plan(doc: &DiffDoc, target: &DiffTarget, pick: Pick<'_>) -> Option<LineWrite> {
    let Partial::Available(direction) = partial(doc, target.source) else {
        return None;
    };
    let file = single_file(doc)?;
    let path = target.path.to_string_lossy();
    let (lines, label) = match pick {
        Pick::Hunk(h) => {
            let hunk = h + 1;
            let label = match direction {
                PatchDirection::Stage => tr!(
                    "diff-lines-label-stage-hunk",
                    hunk = hunk,
                    path = path.as_ref()
                ),
                PatchDirection::Unstage => {
                    tr!(
                        "diff-lines-label-unstage-hunk",
                        hunk = hunk,
                        path = path.as_ref()
                    )
                }
            };
            (file.hunks.get(h)?.line_refs(), label)
        }
        Pick::Selected(selection) => {
            let lines = selection.refs(doc);
            let n = lines.len();
            let label = match direction {
                PatchDirection::Stage => {
                    tr!("diff-lines-label-stage-lines", n = n, path = path.as_ref())
                }
                PatchDirection::Unstage => {
                    tr!(
                        "diff-lines-label-unstage-lines",
                        n = n,
                        path = path.as_ref()
                    )
                }
            };
            (lines, label)
        }
    };
    if lines.is_empty() {
        return None;
    }
    Some(LineWrite {
        direction,
        path: target.path.clone(),
        label,
        lines,
        digest: project_engine::file_digest(file),
    })
}

/// State of the partial selection buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineButtons {
    pub direction: PatchDirection,
    /// "Stage hunk"/"Unstage hunk" on every header.
    pub hunks: bool,
    /// "Stage selected lines (N)".
    pub selected: bool,
    /// "Deselect".
    pub clear: bool,
    /// Selected lines.
    pub count: usize,
}

/// The buttons to show, `None` if the diff does not allow partial selection. `can_write` is
/// false during a write or with a confirmation open.
pub fn buttons(
    partial: &Partial,
    selection: &LineSelection,
    can_write: bool,
) -> Option<LineButtons> {
    let Partial::Available(direction) = partial else {
        return None;
    };
    let count = selection.len();
    Some(LineButtons {
        direction: *direction,
        hunks: can_write,
        selected: can_write && count > 0,
        clear: count > 0,
        count,
    })
}

impl Tab {
    /// The actions of this function, dispatched by [`Tab::apply`].
    pub(crate) fn apply_lines(&mut self, action: Action) {
        match action {
            Action::SelectLine { row, shift } => {
                if let Some((_, doc, selection)) = self.selectable_diff() {
                    selection.click(doc, row, shift);
                }
            }
            Action::DragLines(step) => {
                if let Some((_, doc, selection)) = self.selectable_diff() {
                    match step {
                        Drag::Start(row) => selection.drag_start(doc, row),
                        Drag::Over(row) => selection.drag_over(doc, row),
                        Drag::End => selection.drag_end(),
                    }
                }
            }
            Action::ClearLines => {
                if let Some((_, _, selection)) =
                    self.repo.as_mut().and_then(|r| r.diff.loaded_mut())
                {
                    selection.clear();
                }
            }
            Action::WriteHunk(h) => {
                let write = self.repo.as_ref().and_then(|r| {
                    let (target, doc, _) = r.diff.loaded()?;
                    lines::plan(doc, target, Pick::Hunk(h))
                });
                if let Some(write) = write {
                    self.send_line_write(write);
                }
            }
            Action::WriteSelectedLines => {
                let write = self.repo.as_ref().and_then(|r| {
                    let (target, doc, selection) = r.diff.loaded()?;
                    lines::plan(doc, target, Pick::Selected(selection))
                });
                if let Some(write) = write {
                    self.send_line_write(write);
                }
            }
            _ => unreachable!("action dispatched to the wrong function"),
        }
    }

    /// Sends the stage or unstage of lines, like [`Tab::send_write`]: never two writes
    /// together, nor with a confirmation open.
    pub(crate) fn send_line_write(&mut self, write: LineWrite) {
        if self.executor.writing().is_some() || write.lines.is_empty() {
            return;
        }
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        if repo.modal_open() {
            return;
        }
        repo.write_error = None;
        repo.write_label = Some(write.label.clone());
        self.submit(write.command(), write.params(), Request::Write(generation));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{ByteText, Change, Diff, Hunk, Line, LineKind, Oid};

    fn line(kind: LineKind, old: Option<u32>, new: Option<u32>) -> Line {
        Line {
            kind,
            old_line: old,
            new_line: new,
            content: ByteText::from("x"),
            no_newline: false,
        }
    }

    fn file(hunks: Vec<Hunk>) -> FileDiff {
        FileDiff {
            old_path: "a.rs".into(),
            new_path: "a.rs".into(),
            change: Change::Modified,
            similarity: None,
            old_mode: Some(0o100644),
            new_mode: Some(0o100644),
            old_oid: Some(Oid::from_bytes([1; 20])),
            new_oid: Some(Oid::from_bytes([2; 20])),
            binary: false,
            hunks,
        }
    }

    /// Two hunks: `ctx -1 +1 +2 ctx` and `-9 +9`.
    fn two_hunks() -> FileDiff {
        let first = Hunk {
            old_start: 1,
            old_count: 3,
            new_start: 1,
            new_count: 4,
            section: ByteText::from(""),
            lines: vec![
                line(LineKind::Context, Some(1), Some(1)),
                line(LineKind::Removed, Some(2), None),
                line(LineKind::Added, None, Some(2)),
                line(LineKind::Added, None, Some(3)),
                line(LineKind::Context, Some(3), Some(4)),
            ],
        };
        let second = Hunk {
            old_start: 9,
            old_count: 1,
            new_start: 10,
            new_count: 1,
            section: ByteText::from(""),
            lines: vec![
                line(LineKind::Removed, Some(9), None),
                line(LineKind::Added, None, Some(10)),
            ],
        };
        file(vec![first, second])
    }

    fn doc(files: Vec<FileDiff>) -> DiffDoc {
        DiffDoc::new(Diff { files }, &"a.rs".into())
    }

    fn worktree() -> DiffTarget {
        DiffTarget {
            source: DiffSource::Worktree,
            path: "a.rs".into(),
        }
    }

    // Rows of the `two_hunks` view: 0 file, 1 hunk, 2 ctx, 3 -2, 4 +2, 5 +3, 6 ctx,
    // 7 hunk, 8 -9, 9 +10.

    #[test]
    fn only_changed_lines_are_selectable() {
        let d = doc(vec![two_hunks()]);
        let selectable: Vec<usize> = (0..d.rows.len()).filter(|&r| selectable(&d, r)).collect();
        assert_eq!(selectable, [3, 4, 5, 8, 9]);
        let mut s = LineSelection::default();
        for row in [0, 1, 2, 6, 7, 99] {
            s.click(&d, row, false);
        }
        assert!(s.is_empty());
    }

    #[test]
    fn click_toggles_and_shift_selects_a_range() {
        let d = doc(vec![two_hunks()]);
        let mut s = LineSelection::default();
        s.click(&d, 4, false);
        assert!(s.contains(4));
        assert_eq!(s.refs(&d), [LineRef::New(2)]);
        s.click(&d, 4, false);
        assert!(s.is_empty());

        // Shift+click: from the last clicked row, skipping context and headers.
        s.click(&d, 3, false);
        s.click(&d, 9, true);
        assert_eq!(
            s.refs(&d),
            [
                LineRef::Old(2),
                LineRef::New(2),
                LineRef::New(3),
                LineRef::Old(9),
                LineRef::New(10)
            ]
        );
        // Also backwards, and without a previous Shift+click it is a normal click.
        let mut s = LineSelection::default();
        s.click(&d, 5, true);
        assert_eq!(s.refs(&d), [LineRef::New(3)]);
        s.click(&d, 3, true);
        assert_eq!(s.len(), 3);
        s.clear();
        assert!(s.is_empty());
    }

    #[test]
    fn dragging_selects_or_deselects_from_the_start() {
        let d = doc(vec![two_hunks()]);
        let mut s = LineSelection::default();
        s.drag_start(&d, 3);
        assert!(s.is_dragging());
        s.drag_over(&d, 8);
        s.drag_end();
        assert_eq!(s.len(), 4);
        assert!(!s.contains(9));
        // From a selected row the drag deselects.
        s.drag_start(&d, 4);
        s.drag_over(&d, 5);
        s.drag_end();
        assert_eq!(s.refs(&d), [LineRef::Old(2), LineRef::Old(9)]);
        // Without a drag in progress nothing happens.
        s.drag_over(&d, 9);
        assert!(!s.contains(9));
    }

    #[test]
    fn hunks_and_selections_become_line_refs() {
        let d = doc(vec![two_hunks()]);
        let target = worktree();
        let hunk = plan(&d, &target, Pick::Hunk(1)).unwrap();
        assert_eq!(hunk.lines, [LineRef::Old(9), LineRef::New(10)]);
        assert_eq!(hunk.command(), "index.stage_lines");
        assert_eq!(hunk.label, "Stage hunk 2 of a.rs");
        assert_eq!(hunk.digest, project_engine::file_digest(&d.diff.files[0]));
        assert_eq!(hunk.digest.len(), 32);
        assert_eq!(
            hunk.params(),
            serde_json::json!({
                "path": "a.rs",
                "lines": [{ "old": 9 }, { "new": 10 }],
                "digest": hunk.digest,
            })
        );
        assert!(plan(&d, &target, Pick::Hunk(5)).is_none());

        let mut s = LineSelection::default();
        assert!(
            plan(&d, &target, Pick::Selected(&s)).is_none(),
            "never empty"
        );
        s.click(&d, 5, false);
        let index = DiffTarget {
            source: DiffSource::Index,
            path: "a.rs".into(),
        };
        let write = plan(&d, &index, Pick::Selected(&s)).unwrap();
        assert_eq!(write.command(), "index.unstage_lines");
        assert_eq!(write.lines, [LineRef::New(3)]);
        assert_eq!(write.label, "Unstage 1 line of a.rs");

        let commit = DiffTarget::commit(Oid::from_bytes([3; 20]), "a.rs".into());
        assert!(plan(&d, &commit, Pick::Hunk(0)).is_none());
    }

    #[test]
    fn unsupported_files_explain_why() {
        let d = doc(vec![two_hunks()]);
        assert_eq!(
            partial(&d, DiffSource::Worktree),
            Partial::Available(PatchDirection::Stage)
        );
        assert_eq!(
            partial(&d, DiffSource::Index),
            Partial::Available(PatchDirection::Unstage)
        );
        assert_eq!(
            partial(&d, DiffSource::Commit(Oid::from_bytes([3; 20]))),
            Partial::Hidden
        );

        let mut binary = file(Vec::new());
        binary.binary = true;
        let Partial::Unavailable(reason) = partial(&doc(vec![binary]), DiffSource::Worktree) else {
            panic!("binary");
        };
        assert!(reason.contains("binary"), "{reason}");

        let mut link = two_hunks();
        link.new_mode = Some(0o120000);
        link.old_mode = Some(0o120000);
        assert!(matches!(
            partial(&doc(vec![link]), DiffSource::Index),
            Partial::Unavailable(r) if r.contains("link")
        ));

        // A rename can be split only in the index diff.
        let mut renamed = two_hunks();
        renamed.change = Change::Renamed;
        renamed.old_path = "b.rs".into();
        let d = doc(vec![renamed]);
        assert!(matches!(
            partial(&d, DiffSource::Worktree),
            Partial::Unavailable(_)
        ));
        assert!(matches!(
            partial(&d, DiffSource::Index),
            Partial::Available(_)
        ));

        // Type change: two files on the same path.
        let mut deleted = two_hunks();
        deleted.change = Change::Deleted;
        let typechange = doc(vec![deleted, two_hunks()]);
        assert!(matches!(
            partial(&typechange, DiffSource::Worktree),
            Partial::Unavailable(r) if r.contains("type")
        ));
        assert!(plan(&typechange, &worktree(), Pick::Hunk(0)).is_none());

        // Mode change only: no lines, no buttons.
        let mut mode = file(Vec::new());
        mode.new_mode = Some(0o100755);
        assert_eq!(
            partial(&doc(vec![mode]), DiffSource::Worktree),
            Partial::Hidden
        );
        assert_eq!(
            partial(&doc(Vec::new()), DiffSource::Worktree),
            Partial::Hidden
        );
    }

    #[test]
    fn buttons_follow_selection_and_writes() {
        let d = doc(vec![two_hunks()]);
        let available = partial(&d, DiffSource::Worktree);
        let mut s = LineSelection::default();
        let b = buttons(&available, &s, true).unwrap();
        assert!(b.hunks && !b.selected && !b.clear);
        assert_eq!(b.count, 0);
        s.click(&d, 3, false);
        s.click(&d, 4, false);
        let b = buttons(&available, &s, true).unwrap();
        assert!(b.hunks && b.selected && b.clear);
        assert_eq!(b.count, 2);
        // During a write one can only deselect.
        let b = buttons(&available, &s, false).unwrap();
        assert!(!b.hunks && !b.selected && b.clear);
        assert_eq!(buttons(&Partial::Hidden, &s, true), None);
        assert_eq!(
            buttons(&Partial::Unavailable("binary".to_owned()), &s, true),
            None
        );
    }

    /// The full path with the sample repo, as the UI does it: `diff.worktree` of
    /// `src/lib.rs` read as `Diff`, the stage of the first hunk with the digest computed by the UI,
    /// then the unstage of the same hunk from the index diff.
    #[test]
    fn staging_a_hunk_with_the_sample_repo() {
        use crate::stage::{Follow, Side, StageLists, follow};
        use project_domain::Status;
        use project_engine::{MemoryOpener, Registry, Session};
        use serde_json::json;
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        registry
            .run(
                "repo.open",
                &mut session,
                json!({ "path": MemoryOpener::SAMPLE_PATH }),
            )
            .unwrap();
        let path = RepoPath::from("src/lib.rs");
        let read = |session: &mut Session, source: DiffSource| {
            let target = DiffTarget {
                source,
                path: path.clone(),
            };
            let (command, params) = target.request();
            let value = registry.run(command, session, params).unwrap();
            let digest = value["files"][0]["digest"].as_str().map(str::to_owned);
            let diff: Diff = serde_json::from_value(value).unwrap();
            (target, DiffDoc::new(diff, &path), digest)
        };

        let (target, d, digest) = read(&mut session, DiffSource::Worktree);
        assert_eq!(
            d.diff.files[0].hunks.len(),
            2,
            "two hunks in the sample repo"
        );
        let write = plan(&d, &target, Pick::Hunk(0)).unwrap();
        // The digest computed by the UI is the one the engine sent.
        assert_eq!(Some(write.digest.clone()), digest);
        let out = registry
            .run(write.command(), &mut session, write.params())
            .unwrap();
        let lists =
            StageLists::new(&serde_json::from_value::<Status>(out["status"].clone()).unwrap());
        assert_eq!(follow(&lists, Side::Unstaged, &path), Follow::Reload);
        assert!(lists.contains(Side::Staged, &path));
        let (_, d, _) = read(&mut session, DiffSource::Worktree);
        assert_eq!(d.diff.files[0].hunks.len(), 1, "the second hunk remains");

        // A selection made on an old diff is refused.
        let stale = registry.run(write.command(), &mut session, write.params());
        assert!(matches!(
            stale,
            Err(project_engine::EngineError::StaleDiff { .. })
        ));

        let (target, d, _) = read(&mut session, DiffSource::Index);
        let mut s = LineSelection::default();
        let first = (0..d.rows.len()).find(|&r| selectable(&d, r)).unwrap();
        s.click(&d, first, false);
        let write = plan(&d, &target, Pick::Selected(&s)).unwrap();
        assert_eq!(write.command(), "index.unstage_lines");
        registry
            .run(write.command(), &mut session, write.params())
            .unwrap();
    }
}
