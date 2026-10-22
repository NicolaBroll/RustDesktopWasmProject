//! Vista del diff di un file: quale diff chiedere (`diff.commit`, `diff.worktree` o
//! `diff.index`), quale risposta mostrare e
//! come trasformarla in righe da disegnare. Solo dati, senza egui: si prova con test unitari.
//!
//! Regole delle richieste (come il pannello dettagli):
//! - si chiede un diff quando l'utente apre un file, non a ogni frame;
//! - ogni richiesta ha un numero; una risposta con un numero che non è quello del diff aperto
//!   (l'utente ha aperto un altro file, o ha chiuso la vista) si scarta.

use crate::format::change_name;
use project_domain::{Change, Diff, FileDiff, LineKind, Oid, RepoPath};

/// Da dove viene il diff di un file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffSource {
    /// Il commit rispetto al primo genitore (`diff.commit`).
    Commit(Oid),
    /// Le modifiche non in stage, worktree rispetto all'indice (`diff.worktree`).
    Worktree,
    /// Le modifiche in stage, indice rispetto a HEAD (`diff.index`).
    Index,
}

/// Il file di cui mostrare il diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTarget {
    pub source: DiffSource,
    /// Il percorso nuovo del file (per rinomine e copie quello di arrivo).
    pub path: RepoPath,
}

impl DiffTarget {
    pub fn commit(commit: Oid, path: RepoPath) -> Self {
        Self {
            source: DiffSource::Commit(commit),
            path,
        }
    }

    /// Il comando e i parametri che chiedono questo diff.
    pub fn request(&self) -> (&'static str, serde_json::Value) {
        let path = serde_json::to_value(&self.path).unwrap_or_default();
        match self.source {
            DiffSource::Commit(id) => (
                "diff.commit",
                serde_json::json!({ "id": id.to_string(), "path": path }),
            ),
            DiffSource::Worktree => ("diff.worktree", serde_json::json!({ "path": path })),
            DiffSource::Index => ("diff.index", serde_json::json!({ "path": path })),
        }
    }
}

/// Cosa sa la vista del diff aperto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane<'a> {
    /// Nessun diff aperto: al centro c'è il grafo.
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
}

#[derive(Debug, Default)]
pub struct DiffState {
    open: Option<Open>,
    next: u64,
}

impl DiffState {
    /// Apre il diff di `target` e restituisce il numero della richiesta da mandare, se serve:
    /// non serve se quel diff è già aperto (caricato o in arrivo).
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
        });
        Some(seq)
    }

    /// Dopo un errore richiede lo stesso diff: restituisce il numero della nuova richiesta.
    pub fn retry(&mut self) -> Option<u64> {
        let target = self.open.as_ref()?.target.clone();
        matches!(self.open.as_ref()?.answer, Some(Err(_)))
            .then(|| self.open(target))
            .flatten()
    }

    /// Richiede il diff aperto anche se è già caricato (il file è cambiato): restituisce il
    /// numero della nuova richiesta. Fino alla risposta la vista mostra il caricamento.
    pub fn reload(&mut self) -> Option<u64> {
        let open = self.open.as_mut()?;
        open.seq = self.next;
        open.answer = None;
        self.next += 1;
        Some(open.seq)
    }

    /// Chiude la vista: le risposte in arrivo si scarteranno.
    pub fn close(&mut self) {
        self.open = None;
    }

    pub fn target(&self) -> Option<&DiffTarget> {
        self.open.as_ref().map(|o| &o.target)
    }

    /// La risposta della richiesta `seq`. Si tiene solo se è quella del diff aperto:
    /// restituisce `false` se è stata scartata.
    pub fn receive(&mut self, seq: u64, result: Result<Diff, String>) -> bool {
        let Some(open) = &mut self.open else {
            return false;
        };
        if open.seq != seq || open.answer.is_some() {
            return false;
        }
        open.answer = Some(result.map(|diff| DiffDoc::new(diff, &open.target.path)));
        true
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

/// Una riga della vista: indici nel [`Diff`] del documento, così migliaia di righe non
/// copiano il contenuto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// Titolo del file `files[f]`.
    File(usize),
    /// Informazione sotto il titolo (modi, binario, file vuoto...).
    Note(usize, String),
    /// Intestazione dell'hunk `files[f].hunks[h]`.
    Hunk(usize, usize),
    /// Riga `files[f].hunks[h].lines[l]`.
    Line(usize, usize, usize),
    /// `\ No newline at end of file` dopo la riga precedente.
    NoNewline,
    /// Spazio tra un file e il seguente.
    Gap,
}

/// Un diff pronto da disegnare: le righe in ordine, le larghezze delle colonne.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffDoc {
    pub diff: Diff,
    pub rows: Vec<Row>,
    /// Cifre delle colonne dei numeri di riga.
    pub number_width: usize,
    /// Colonne di testo della riga più larga (dopo tab ed eventuale taglio).
    pub max_cols: usize,
}

impl DiffDoc {
    /// `diff` è la risposta di `diff.commit` per `path`: il motore restituisce anche i file
    /// che hanno `path` come origine (es. una copia da `path`); si tengono quelli che arrivano a
    /// `path` (due per un cambio di tipo), oppure tutti se nessuno ci arriva.
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
        Self {
            diff,
            rows,
            number_width: number_width(max_number),
            max_cols,
        }
    }

    /// Righe aggiunte e tolte di tutti i file.
    pub fn line_counts(&self) -> (usize, usize) {
        self.diff
            .files
            .iter()
            .map(FileDiff::line_counts)
            .fold((0, 0), |(a, d), (fa, fd)| (a + fa, d + fd))
    }
}

/// Titolo del file: il percorso, oppure `vecchio -> nuovo` per rinomine e copie (ASCII: i font
/// di default non hanno le frecce).
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

/// Tipo di modifica, somiglianza e righe cambiate, es. `rinominato 92%  +1 -1`.
pub fn file_summary(file: &FileDiff) -> String {
    let mut parts = vec![change_name(file.change).to_owned()];
    if let Some(similarity) = file.similarity {
        parts.push(format!("{similarity}%"));
    }
    if file.is_submodule() {
        parts.push("[sottomodulo]".to_owned());
    }
    let (added, removed) = file.line_counts();
    if added + removed > 0 {
        parts.push(format!("+{added} -{removed}"));
    }
    parts.join("  ")
}

/// Le righe di informazione sotto il titolo: modi cambiati e, se il file non ha hunk, perché.
pub fn file_notes(file: &FileDiff) -> Vec<String> {
    let mut notes = Vec::new();
    match (file.old_mode, file.new_mode) {
        (Some(old), Some(new)) if old != new => notes.push(format!("modo {old:o} -> {new:o}")),
        (None, Some(new)) if file.change == Change::Added => {
            notes.push(format!("nuovo file, modo {new:o}"));
        }
        (Some(old), None) if file.change == Change::Deleted => {
            notes.push(format!("file eliminato, modo {old:o}"));
        }
        _ => {}
    }
    if file.is_submodule() {
        notes.push("sottomodulo: il diff mostra il commit a cui punta".to_owned());
    }
    if !file.hunks.is_empty() {
        return notes;
    }
    let reason = if file.change == Change::Unmerged {
        "file in conflitto: il diff non è disponibile finché il conflitto non è risolto"
    } else if file.binary {
        "file binario: il contenuto non si mostra"
    } else {
        match file.change {
            Change::Renamed => "rinomina esatta: contenuto invariato",
            Change::Copied => "copia esatta: contenuto invariato",
            Change::Added => "file vuoto",
            Change::Deleted => "file vuoto eliminato",
            _ if file.old_mode != file.new_mode => "solo cambio di modo: contenuto invariato",
            _ => "nessuna modifica al contenuto",
        }
    };
    notes.push(reason.to_owned());
    notes
}

/// Spazi di un tab.
pub const TAB_WIDTH: usize = 4;
/// Colonne mostrate al massimo per riga: oltre la riga si taglia e lo dice.
pub const MAX_LINE_COLS: usize = 1000;

/// Il contenuto di una riga com'è da disegnare.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayLine {
    /// Pezzi di testo; `true` per i segni messi al posto dei caratteri di controllo (`^M` per
    /// `\r`), da disegnare più tenui.
    pub spans: Vec<(String, bool)>,
    /// Colonne occupate (un carattere, una colonna).
    pub cols: usize,
    /// Caratteri tagliati in fondo perché la riga supera [`MAX_LINE_COLS`].
    pub cut: usize,
}

/// Rende leggibile una riga: tab espansi fino al multiplo di [`TAB_WIDTH`] seguente, `\r` e
/// gli altri caratteri di controllo come `^M`, `^[`...; oltre [`MAX_LINE_COLS`] colonne la
/// riga si taglia.
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

/// Cifre che servono per scrivere `max` (almeno 1).
pub fn number_width(max: u32) -> usize {
    max.max(1).ilog10() as usize + 1
}

/// Numero di riga allineato a destra su `width` cifre, o spazi se la riga non c'è in quel lato.
pub fn line_number(number: Option<u32>, width: usize) -> String {
    match number {
        Some(n) => format!("{n:>width$}"),
        None => " ".repeat(width),
    }
}

/// Il segno della riga come lo scrive git: ` `, `+`, `-`.
pub fn line_prefix(kind: LineKind) -> char {
    char::from(kind.prefix())
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
        let mut last = line(LineKind::Added, None, Some(10), "fine");
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
        let seq = s.open(target(1, "a.rs")).expect("prima richiesta");
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
        // Una seconda risposta con lo stesso numero non sostituisce la prima.
        assert!(!s.receive(seq, Err("tardi".to_owned())));
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
        // Stesso percorso, commit diverso: è un altro diff.
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
        assert_eq!(s.retry(), None, "niente da riprovare mentre carica");
        assert!(s.receive(seq, Err("commit sconosciuto".to_owned())));
        assert_eq!(
            s.pane(),
            Pane::Failed(&target(1, "a.rs"), "commit sconosciuto")
        );
        let again = s.retry().expect("nuova richiesta");
        assert_ne!(again, seq);
        assert_eq!(s.pane(), Pane::Loading(&target(1, "a.rs")));
        // Anche riaprire lo stesso file dopo un errore lo richiede.
        assert!(s.receive(again, Err("ancora".to_owned())));
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
        assert_eq!(doc.number_width, 2, "fino alla riga 11");
        // `b\tc`: il tab porta `c` alla colonna 4.
        assert_eq!(doc.max_cols, 5);
        assert_eq!(doc.line_counts(), (2, 1));
        let header = String::from_utf8(doc.diff.files[0].hunks[0].header()).unwrap();
        assert_eq!(header, "@@ -8,2 +8,3 @@ fn main() {");
    }

    #[test]
    fn only_files_that_end_at_the_path_are_kept() {
        // Il motore restituisce anche la copia che parte da `a.rs`.
        let copy = file(Change::Copied, "a.rs", "b.rs", vec![hunk()]);
        let modified = file(Change::Modified, "a.rs", "a.rs", vec![hunk()]);
        let doc = DiffDoc::new(diff(vec![modified.clone(), copy.clone()]), &"a.rs".into());
        assert_eq!(doc.diff.files, vec![modified]);
        // Un cambio di tipo: due file sullo stesso percorso, separati.
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
        assert_eq!(file_summary(&renamed), "rinominato  92%  +2 -1");
        assert!(file_notes(&renamed).is_empty());

        renamed.hunks.clear();
        renamed.old_mode = None;
        renamed.new_mode = None;
        assert_eq!(
            file_notes(&renamed),
            ["rinomina esatta: contenuto invariato"]
        );

        let mut mode = file(Change::Modified, "run.sh", "run.sh", Vec::new());
        mode.new_mode = Some(0o100755);
        assert_eq!(file_title(&mode), "run.sh");
        assert_eq!(
            file_notes(&mode),
            [
                "modo 100644 -> 100755",
                "solo cambio di modo: contenuto invariato"
            ]
        );

        let mut empty = file(Change::Deleted, ".gitkeep", ".gitkeep", Vec::new());
        empty.new_mode = None;
        assert_eq!(
            file_notes(&empty),
            ["file eliminato, modo 100644", "file vuoto eliminato"]
        );

        let mut binary = file(Change::Added, "logo.png", "logo.png", Vec::new());
        binary.old_mode = None;
        binary.binary = true;
        assert_eq!(
            file_notes(&binary),
            [
                "nuovo file, modo 100644",
                "file binario: il contenuto non si mostra"
            ]
        );

        let mut sub = file(Change::Modified, "vendor/lib", "vendor/lib", vec![hunk()]);
        sub.old_mode = Some(0o160000);
        sub.new_mode = Some(0o160000);
        assert!(file_summary(&sub).contains("[sottomodulo]"));
        assert_eq!(file_notes(&sub).len(), 1);

        let mut conflict = file(Change::Unmerged, "x", "x", Vec::new());
        conflict.old_mode = None;
        conflict.new_mode = None;
        assert!(file_notes(&conflict)[0].starts_with("file in conflitto"));
    }

    #[test]
    fn tabs_and_control_characters_are_readable() {
        let plain = display_line("let x = 1;");
        assert_eq!(plain.spans, [("let x = 1;".to_owned(), false)]);
        assert_eq!((plain.cols, plain.cut), (10, 0));

        // Tab fino al multiplo di 4 seguente, dalla colonna in cui si trova.
        assert_eq!(display_line("\tx").spans, [("    x".to_owned(), false)]);
        assert_eq!(display_line("ab\tx").spans, [("ab  x".to_owned(), false)]);
        assert_eq!(display_line("abcd\tx").cols, 9);

        // Il `\r` di una riga CRLF e gli altri caratteri di controllo si vedono.
        let crlf = display_line("fine\r");
        assert_eq!(
            crlf.spans,
            [("fine".to_owned(), false), ("^M".to_owned(), true)]
        );
        assert_eq!(crlf.cols, 6);
        assert_eq!(display_line("\u{1b}[0m").spans[0], ("^[".to_owned(), true));

        // Caratteri non ASCII: una colonna ciascuno.
        assert_eq!(display_line("città").cols, 5);
        // Contenuto non UTF-8: il carattere sostitutivo.
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
        assert_eq!(state.reload(), None, "niente da ricaricare");
        let first = state.open(target(1, "a")).unwrap();
        assert!(state.receive(first, Ok(Diff { files: Vec::new() })));
        let second = state.reload().unwrap();
        assert_ne!(first, second);
        assert!(matches!(state.pane(), Pane::Loading(_)));
        // La risposta della richiesta vecchia non vale più.
        assert!(!state.receive(first, Ok(Diff { files: Vec::new() })));
        assert!(state.receive(second, Ok(Diff { files: Vec::new() })));
        assert!(matches!(state.pane(), Pane::Loaded(..)));
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
}
