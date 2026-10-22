//! Costruttore di patch per lo stage e l'unstage di singole righe.
//!
//! Dato un [`FileDiff`] letto con [`super::parse_diff`] e le righe scelte dall'utente,
//! [`build_patch`] restituisce la patch da passare a `git apply` con gli argomenti di
//! [`APPLY_ARGS`] (patch su stdin). La patch è un [`FileDiff`]: [`FileDiff::to_patch`] dà i byte.
//!
//! # Le due direzioni
//!
//! `git apply --cached` confronta il **preimage** della patch (righe di contesto e righe `-`)
//! con il contenuto del file nell'indice, e se coincide lo sostituisce con il **postimage**
//! (contesto e righe `+`). In entrambe le direzioni la patch è costruita *in avanti* rispetto
//! all'indice, cioè il suo preimage è il contenuto attuale dell'indice, e si applica con gli
//! stessi argomenti (niente `--reverse`):
//!
//! - [`PatchDirection::Stage`], sul diff worktree → indice (`diff.worktree`, `git diff`): il
//!   lato vecchio del diff è l'indice, quindi è il preimage. Righe aggiunte selezionate: `+`;
//!   aggiunte non selezionate: tolte; rimosse selezionate: `-`; rimosse non selezionate:
//!   contesto (sono nell'indice e ci restano).
//! - [`PatchDirection::Unstage`], sul diff HEAD → indice (`diff.index`, `git diff --cached`): qui
//!   l'indice è il lato **nuovo** del diff. Il costruttore inverte il diff (indice → HEAD) e poi
//!   applica la stessa regola, così la regola si rovescia rispetto alle righe mostrate: righe
//!   aggiunte selezionate (in stage, da togliere): `-`; aggiunte non selezionate: contesto (sono
//!   nell'indice e ci restano); rimosse selezionate (da rimettere): `+`; rimosse non selezionate:
//!   tolte (non sono nell'indice e non ci tornano).
//!
//! In tutte e due le direzioni l'ordine nel risultato è quello del diff mostrato, con un
//! accorgimento: in ogni gruppo di modifiche fra due righe di contesto la prima riga tolta si
//! accoppia alla prima aggiunta, la seconda alla seconda, e così via (git scrive tutte le tolte
//! e poi tutte le aggiunte). Così di `-x -y +x' +y'` lo stage della sola `x → x'` dà `x' y`, non
//! `y x'`. Ne segue che fare l'unstage di una selezione dà lo stesso contenuto che fare lo stage
//! delle righe non selezionate a partire da HEAD.
//!
//! # Selezione
//!
//! Una riga cambiata si indica con il suo numero di riga nel lato in cui esiste, come lo mostra
//! il diff letto ([`Line::old_line`] / [`Line::new_line`]): [`LineRef::Old`] per una riga tolta,
//! [`LineRef::New`] per una aggiunta. In JSON `{"old": 3}` e `{"new": 5}`. Nel diff worktree
//! `old` è il numero nell'indice e `new` nel worktree; nel diff dell'indice `old` è in HEAD e
//! `new` nell'indice. Un hunk intero è l'insieme delle sue righe cambiate
//! ([`Hunk::line_refs`]); le righe di contesto non si selezionano.
//!
//! - Selezione vuota: `Ok(None)`, nessuna patch.
//! - Un riferimento che non corrisponde a una riga cambiata del diff (numero inesistente, riga
//!   di contesto, lato sbagliato): [`DomainError::InvalidLineSelection`] e nessuna patch. Se il
//!   file è cambiato da quando la selezione è stata fatta i numeri possono non tornare più:
//!   meglio un errore che una patch su righe diverse. Riferimenti ripetuti valgono una volta.
//!
//! # Casi particolari
//!
//! - **Fine file senza a capo**: una riga senza a capo può essere solo l'ultima. Se nel
//!   risultato una riga senza a capo non è più l'ultima (es. si tiene la vecchia ultima riga e
//!   si aggiunge una riga dopo), prende l'a capo: se era nel preimage diventa la coppia
//!   `-riga` + `\ No newline at end of file` e `+riga`.
//! - **File nuovo** (nel diff worktree solo per un file aggiunto con `git add -N`, che git
//!   mostra come nuovo; nel diff dell'indice per l'unstage di un file eliminato in stage): la
//!   patch crea il file con le sole righe selezionate (`new file mode`). `git apply --cached`
//!   accetta di creare un file che nell'indice è solo un segnaposto `intent-to-add`; un file
//!   non tracciato non compare in `git diff` e va messo in stage intero.
//! - **File eliminato** (nel diff worktree: eliminato dal worktree; nel diff dell'indice: un
//!   file nuovo in stage di cui si fa l'unstage): con tutte le righe selezionate la patch elimina
//!   il file dall'indice (`deleted file mode`); con una selezione parziale il file resta con le
//!   righe non selezionate e la patch è una modifica.
//! - **Rinomina**: le patch per righe non rinominano mai. Nel diff dell'indice (rinomina in
//!   stage) la patch modifica il file con il percorso nuovo, quello che sta nell'indice: il
//!   vecchio percorso resta fuori dall'indice; per annullare la rinomina si usa l'unstage del
//!   file intero. Nel diff worktree una rinomina (con `git add -N`) è rifiutata: si mette in
//!   stage il file intero.
//! - **Cambio di modo**: non fa parte delle righe e non entra mai nella patch, che lascia il
//!   modo dell'indice com'è; si mette in stage con il file intero. Un cambio di solo modo non ha
//!   righe da selezionare.
//! - **Intestazioni**: la patch non ha la riga `index` (gli id non sarebbero più quelli del
//!   contenuto) né `similarity`/`rename`; i nomi sono scritti da [`FileDiff::write_header`],
//!   tra virgolette dove servono, con i byte esatti. I conteggi degli hunk sono ricalcolati, così
//!   la patch è valida anche senza `--recount`. Il contenuto (CRLF, byte non UTF-8) è copiato
//!   byte per byte.
//! - **Rifiutati** con [`DomainError::UnsupportedPatch`]: file binari, sottomoduli, link
//!   simbolici, file in conflitto, copie, rinomine nel diff worktree. Il controllo viene prima
//!   di quello della selezione.

use super::{Change, FileDiff, Hunk, Line, LineKind, RepoPath};
use crate::DomainError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Argomenti di `git apply` per applicare una patch di [`build_patch`] all'indice, letta da
/// stdin; valgono per entrambe le direzioni.
///
/// - `--cached`: solo l'indice, mai il worktree;
/// - `--recount`: conteggi delle righe ricalcolati dal contenuto (quelli scritti sono già
///   corretti, è una sicurezza in più);
/// - `--whitespace=nowarn`: con `apply.whitespace=error` nella configurazione una riga con spazi
///   finali farebbe fallire la patch; qui si copia il contenuto dell'utente così com'è;
/// - `--no-ignore-whitespace`: con `apply.ignoreWhitespace=change` il preimage potrebbe
///   coincidere con un indice diverso negli spazi.
///
/// Prima di scrivere si può provare la patch con `--check` in più.
///
/// Attenzione: `git apply` cerca il preimage di un hunk anche lontano dalla posizione scritta,
/// e un hunk senza contesto in coda lo ancora alla fine del file. È sicuro solo con patch da un
/// diff letto con `DIFF_ARGS` (contesto fisso `--unified=3`, anche con `diff.context=0` nella
/// configurazione) e applicate allo stesso indice da cui il diff è stato calcolato. I comandi
/// `index.stage_lines`/`index.unstage_lines` per questo non usano `git apply`: calcolano il
/// risultato con un applicatore rigoroso nel motore e lo scrivono nell'indice.
pub const APPLY_ARGS: &[&str] = &[
    "--cached",
    "--recount",
    "--whitespace=nowarn",
    "--no-ignore-whitespace",
];

const SYMLINK: u32 = 0o120000;

/// In che direzione si spostano le righe selezionate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PatchDirection {
    /// Dal worktree all'indice: il diff è quello del worktree (`diff.worktree`).
    Stage,
    /// Dall'indice indietro verso HEAD: il diff è quello dell'indice (`diff.index`).
    Unstage,
}

/// Una riga cambiata di un diff, indicata con il numero di riga nel lato in cui esiste.
///
/// In JSON: `{"old": <n>}` o `{"new": <n>}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineRef {
    /// Una riga tolta, con il suo numero nel file vecchio ([`Line::old_line`]).
    Old(u32),
    /// Una riga aggiunta, con il suo numero nel file nuovo ([`Line::new_line`]).
    New(u32),
}

impl Line {
    /// Il riferimento con cui selezionare questa riga; `None` per una riga di contesto.
    pub fn line_ref(&self) -> Option<LineRef> {
        match self.kind {
            LineKind::Context => None,
            LineKind::Removed => self.old_line.map(LineRef::Old),
            LineKind::Added => self.new_line.map(LineRef::New),
        }
    }
}

impl Hunk {
    /// I riferimenti di tutte le righe cambiate dell'hunk, in ordine: la selezione dell'hunk
    /// intero.
    pub fn line_refs(&self) -> Vec<LineRef> {
        self.lines.iter().filter_map(Line::line_ref).collect()
    }
}

impl FileDiff {
    /// I riferimenti di tutte le righe cambiate del file, in ordine.
    pub fn line_refs(&self) -> Vec<LineRef> {
        self.hunks.iter().flat_map(Hunk::line_refs).collect()
    }
}

/// Costruisce la patch che sposta nell'indice (o toglie dall'indice) solo le righe `lines` di
/// `file`, da applicare con `git apply` e gli argomenti di [`APPLY_ARGS`].
///
/// `file` è un file di `diff.worktree` per [`PatchDirection::Stage`], di `diff.index` per
/// [`PatchDirection::Unstage`]; `lines` usa i numeri di riga di `file` (vedi [`LineRef`]).
/// Restituisce `Ok(None)` se `lines` è vuota. Vedi la documentazione del modulo per la
/// semantica, i casi particolari e quelli rifiutati.
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

/// Il diff con le righe di ogni gruppo di modifiche (fra due righe di contesto) alternate: la
/// prima tolta, la prima aggiunta, la seconda tolta, la seconda aggiunta... con la selezione
/// riordinata allo stesso modo. Ogni lato resta nel suo ordine, quindi il diff vuol dire la
/// stessa cosa; serve a decidere dove finiscono, nel risultato, le righe che restano rispetto
/// a quelle che entrano: una modifica `-x -y +x' +y'` di cui si sceglie solo `x → x'` dà
/// `x' y`, non `y x'`.
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

/// Svuota un gruppo di modifiche in `out`, alternando tolte e aggiunte.
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
        Some("il file è in conflitto: va risolto e messo in stage intero")
    } else if file.binary {
        Some("il file è binario: non ha righe da selezionare, si mette in stage intero")
    } else if file.is_submodule() {
        Some("è un sottomodulo: non ha righe da selezionare, si mette in stage intero")
    } else if file.old_mode == Some(SYMLINK) || file.new_mode == Some(SYMLINK) {
        Some("è un link simbolico: il suo contenuto si mette in stage solo intero")
    } else if file.change == Change::Copied {
        Some("è una copia: si mette in stage solo il file intero")
    } else if file.change == Change::Renamed && direction == PatchDirection::Stage {
        Some(
            "è una rinomina non ancora in stage: si mette in stage il file intero, con i due percorsi",
        )
    } else if !matches!(
        file.change,
        Change::Added | Change::Deleted | Change::Modified | Change::Renamed
    ) {
        Some("tipo di modifica non previsto in un diff")
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

/// Per ogni hunk e riga di `file`, vero se la riga è selezionata.
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
                LineRef::Old(n) => format!("tolta n. {n}"),
                LineRef::New(n) => format!("aggiunta n. {n}"),
            })
            .collect();
        return Err(DomainError::InvalidLineSelection {
            path: file.path().to_string_lossy().into_owned(),
            reason: format!(
                "nel diff non c'è una riga {} (il file è forse cambiato nel frattempo?)",
                list.join(", ")
            ),
        });
    }
    Ok(selected)
}

/// Il diff nell'altro verso: righe tolte e aggiunte scambiate, come i lati e i percorsi.
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

/// La regola dello stage su `base` (il cui lato vecchio è l'indice): `None` se nessuna riga
/// selezionata cambia qualcosa.
fn select(base: &FileDiff, selected: &[Vec<bool>]) -> Option<FileDiff> {
    // Righe del risultato, nell'ordine del diff: contesto, tolte non selezionate, aggiunte
    // selezionate. `later[h][i]`: dopo la riga `i` dell'hunk `h` c'è un'altra riga del risultato.
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
                        // Ultima riga del preimage senza a capo, seguita da altre righe nel
                        // risultato: nel risultato prende l'a capo.
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

/// Righe del lato vecchio (`skip` = [`LineKind::Added`]) o nuovo (`skip` =
/// [`LineKind::Removed`]).
fn count(lines: &[Line], skip: LineKind) -> u32 {
    let n = lines.iter().filter(|l| l.kind != skip).count();
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// In ogni gruppo di righe cambiate fra due righe di contesto, prima le tolte e poi le
/// aggiunte, come scrive git. L'ordine di ciascun lato non cambia, quindi nemmeno il significato.
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

/// Assegna i numeri di riga dei due lati a partire dall'intestazione dell'hunk.
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
