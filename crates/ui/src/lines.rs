//! Stage e unstage di hunk e singole righe dalla vista del diff (`index.stage_lines`,
//! `index.unstage_lines`): quali righe sono selezionate, se il file permette la selezione
//! parziale, i riferimenti da mandare al motore e lo stato dei pulsanti. Solo dati, senza egui:
//! si prova con test unitari.
//!
//! Le righe si indicano con l'indice della riga della vista ([`DiffDoc::rows`]): valgono per il
//! documento caricato, e la selezione si azzera a ogni nuovo documento (dopo una scrittura, una
//! ricarica o un altro file). Solo le righe aggiunte e tolte si selezionano, mai il contesto.

use crate::diff::{DiffDoc, DiffSource, DiffTarget, Row};
use crate::{Action, App, Drag, Request, lines};
use project_domain::{DomainError, FileDiff, LineRef, PatchDirection, RepoPath, build_patch};
use project_i18n::tr;
use std::collections::BTreeSet;

/// Se il diff mostrato permette lo stage (o l'unstage) di hunk e righe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Partial {
    /// Niente pulsanti: diff di un commit, o nessuna riga cambiata.
    Hidden,
    /// Il file non si può dividere in righe: il motivo, da mostrare.
    Unavailable(String),
    /// Hunk e righe si possono spostare in questa direzione.
    Available(PatchDirection),
}

/// La direzione per il diff di `source`: worktree → stage, indice → unstage.
pub fn direction(source: DiffSource) -> Option<PatchDirection> {
    match source {
        DiffSource::Worktree => Some(PatchDirection::Stage),
        DiffSource::Index => Some(PatchDirection::Unstage),
        DiffSource::Commit(_) => None,
    }
}

/// Decide se il diff `doc` di `source` permette la selezione parziale, con le stesse regole del
/// costruttore di patch del dominio ([`build_patch`] rifiuta binari, sottomoduli, link, conflitti,
/// copie e rinomine nello stage).
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

/// Il file a cui si riferiscono righe e hunk: l'unico del documento.
fn single_file(doc: &DiffDoc) -> Option<&FileDiff> {
    match &doc.diff.files[..] {
        [file] => Some(file),
        _ => None,
    }
}

/// Il riferimento della riga `row` della vista, se è una riga cambiata.
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

/// Vero se la riga `row` si può selezionare: una riga aggiunta o tolta.
pub fn selectable(doc: &DiffDoc, row: usize) -> bool {
    line_ref(doc, row).is_some()
}

/// Le righe selezionate di un diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineSelection {
    rows: BTreeSet<usize>,
    /// L'ultima riga cliccata senza Maiusc: un Maiusc+clic seleziona da qui.
    anchor: Option<usize>,
    /// Durante un trascinamento: `true` se seleziona, `false` se deseleziona.
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

    /// Clic su una riga: la seleziona o la deseleziona. Con Maiusc seleziona tutte le righe
    /// cambiate dall'ultima cliccata a questa, comprese. Le righe di contesto si ignorano.
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

    /// Inizio di un trascinamento sulla riga `row`: se la riga non era selezionata il
    /// trascinamento seleziona, altrimenti deseleziona.
    pub fn drag_start(&mut self, doc: &DiffDoc, row: usize) {
        if !selectable(doc, row) {
            return;
        }
        let select = !self.rows.contains(&row);
        self.drag = Some(select);
        self.anchor = Some(row);
        self.set_range(doc, row, row, select);
    }

    /// Il trascinamento è sulla riga `row`: le righe dall'inizio a qui prendono lo stato
    /// scelto all'inizio.
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

    /// I riferimenti delle righe selezionate, nell'ordine del diff.
    pub fn refs(&self, doc: &DiffDoc) -> Vec<LineRef> {
        self.rows.iter().filter_map(|&r| line_ref(doc, r)).collect()
    }
}

/// Cosa spostare: un hunk intero o le righe selezionate.
#[derive(Debug, Clone, Copy)]
pub enum Pick<'a> {
    /// L'hunk con questo indice nel file.
    Hunk(usize),
    Selected(&'a LineSelection),
}

/// Una scrittura di righe da mandare al motore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineWrite {
    pub direction: PatchDirection,
    pub path: RepoPath,
    /// Mai vuota.
    pub lines: Vec<LineRef>,
    /// Il digest del `FileDiff` visto (lo stesso del campo `digest` di `diff.worktree`/
    /// `diff.index`).
    pub digest: String,
    /// Descrizione per i messaggi, es. "Stage dell'hunk 1 di src/lib.rs".
    pub label: String,
}

impl LineWrite {
    /// L'id del comando del Registry.
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

/// La scrittura per `pick` nel diff `doc` di `target`; `None` se il diff non permette la
/// selezione parziale o non c'è niente da spostare (mai una lista di righe vuota).
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

/// Stato dei pulsanti della selezione parziale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineButtons {
    pub direction: PatchDirection,
    /// "Stage hunk"/"Unstage hunk" su ogni intestazione.
    pub hunks: bool,
    /// "Stage righe selezionate (N)".
    pub selected: bool,
    /// "Deseleziona".
    pub clear: bool,
    /// Righe selezionate.
    pub count: usize,
}

/// I pulsanti da mostrare, `None` se il diff non permette la selezione parziale. `can_write` è
/// falso durante una scrittura o con una conferma aperta.
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

/// Messaggio per una scrittura di righe rifiutata perché il file è cambiato.
pub const STALE_MESSAGE: &str =
    "il file è cambiato: rileggi il diff e rifai la selezione (il diff è stato riletto)";

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
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
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Manda lo stage o l'unstage di righe, come [`App::send_write`]: mai due scritture
    /// insieme, né con una conferma aperta.
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

    /// Due hunk: `ctx -1 +1 +2 ctx` e `-9 +9`.
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

    // Righe della vista di `two_hunks`: 0 file, 1 hunk, 2 ctx, 3 -2, 4 +2, 5 +3, 6 ctx,
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

        // Maiusc+clic: dall'ultima riga cliccata, saltando contesto e intestazioni.
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
        // Anche all'indietro, e senza ancora un Maiusc+clic è un clic normale.
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
        // Da una riga selezionata il trascinamento deseleziona.
        s.drag_start(&d, 4);
        s.drag_over(&d, 5);
        s.drag_end();
        assert_eq!(s.refs(&d), [LineRef::Old(2), LineRef::Old(9)]);
        // Senza trascinamento in corso non succede niente.
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
        assert_eq!(hunk.label, "Stage dell'hunk 2 di a.rs");
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
        assert!(plan(&d, &target, Pick::Selected(&s)).is_none(), "mai vuota");
        s.click(&d, 5, false);
        let index = DiffTarget {
            source: DiffSource::Index,
            path: "a.rs".into(),
        };
        let write = plan(&d, &index, Pick::Selected(&s)).unwrap();
        assert_eq!(write.command(), "index.unstage_lines");
        assert_eq!(write.lines, [LineRef::New(3)]);
        assert_eq!(write.label, "Unstage di 1 riga di a.rs");

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
            panic!("binario");
        };
        assert!(reason.contains("binario"), "{reason}");

        let mut link = two_hunks();
        link.new_mode = Some(0o120000);
        link.old_mode = Some(0o120000);
        assert!(matches!(
            partial(&doc(vec![link]), DiffSource::Index),
            Partial::Unavailable(r) if r.contains("link")
        ));

        // Una rinomina si può dividere solo nel diff dell'indice.
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

        // Cambio di tipo: due file sullo stesso percorso.
        let mut deleted = two_hunks();
        deleted.change = Change::Deleted;
        let typechange = doc(vec![deleted, two_hunks()]);
        assert!(matches!(
            partial(&typechange, DiffSource::Worktree),
            Partial::Unavailable(r) if r.contains("tipo")
        ));
        assert!(plan(&typechange, &worktree(), Pick::Hunk(0)).is_none());

        // Solo cambio di modo: nessuna riga, nessun pulsante.
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
        // Durante una scrittura si può solo deselezionare.
        let b = buttons(&available, &s, false).unwrap();
        assert!(!b.hunks && !b.selected && b.clear);
        assert_eq!(buttons(&Partial::Hidden, &s, true), None);
        assert_eq!(
            buttons(&Partial::Unavailable("binario".to_owned()), &s, true),
            None
        );
    }

    /// Il percorso completo con il repo di esempio, come lo fa la UI: `diff.worktree` di
    /// `src/lib.rs` letto come `Diff`, lo stage del primo hunk con il digest calcolato dalla UI,
    /// poi l'unstage dello stesso hunk dal diff dell'indice.
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
            "due hunk nel repo di esempio"
        );
        let write = plan(&d, &target, Pick::Hunk(0)).unwrap();
        // Il digest calcolato dalla UI è quello che il motore ha mandato.
        assert_eq!(Some(write.digest.clone()), digest);
        let out = registry
            .run(write.command(), &mut session, write.params())
            .unwrap();
        let lists =
            StageLists::new(&serde_json::from_value::<Status>(out["status"].clone()).unwrap());
        assert_eq!(follow(&lists, Side::Unstaged, &path), Follow::Reload);
        assert!(lists.contains(Side::Staged, &path));
        let (_, d, _) = read(&mut session, DiffSource::Worktree);
        assert_eq!(d.diff.files[0].hunks.len(), 1, "resta il secondo hunk");

        // Una selezione fatta su un diff vecchio è rifiutata.
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
