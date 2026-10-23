//! Parser dei marcatori di conflitto nel file del worktree e risoluzione dei blocchi.
//!
//! [`parse_conflicts`] divide i byte del file in testo comune e blocchi di conflitto
//! ([`ConflictText`]); [`resolve_block`] e [`resolve_all`] sostituiscono uno o tutti i blocchi
//! con la versione scelta ([`Choice`]). Tutto lavora sui byte: niente si perde se il file non è
//! UTF-8.
//!
//! # Marcatori
//!
//! Un marcatore sta a inizio riga ed è fatto di esattamente `marker_size` caratteri uguali
//! (7 per default, attributo `conflict-marker-size`), seguiti da fine riga o da uno spazio e
//! un'etichetta:
//!
//! ```text
//! <<<<<<< nostra        inizio, etichetta del lato nostro (es. `HEAD`)
//! righe nostre
//! ||||||| base          solo con `merge.conflictStyle=diff3`/`zdiff3`
//! righe della base
//! =======
//! righe loro
//! >>>>>>> loro          fine, etichetta del lato loro (es. il branch unito)
//! ```
//!
//! - Una riga con più caratteri (`<<<<<<<<` con `marker_size` 7) o con altro dopo i caratteri
//!   senza spazio (`=======x`) è testo.
//! - A capo `\n` o `\r\n`, conservati com'erano; l'ultima riga può non avere a capo.
//! - Fuori da un blocco, una riga che sembra un marcatore di mezzo o di fine (`|||||||`,
//!   `=======`, `>>>>>>>`) è testo comune: può essere un titolo setext di Markdown o una riga
//!   qualsiasi di un file vero.
//! - Dentro un blocco i marcatori devono venire nell'ordine `<` (`|`) `=` `>`: un marcatore fuori
//!   ordine (un secondo `<<<<<<<`, `>>>>>>>` prima di `=======`, ...) o un blocco non chiuso a
//!   fine file danno [`DomainError::InvalidConflict`]. In quel caso l'editor mostra le tre
//!   versioni e il file com'è, senza blocchi.

use crate::DomainError;
use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Il file del worktree diviso in testo comune e blocchi di conflitto, nell'ordine del file.
///
/// Concatenare i segmenti, con ogni blocco sostituito dai byte del testo nel suo
/// [`ConflictBlock::range`], ridà il file.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ConflictText {
    pub segments: Vec<Segment>,
}

impl ConflictText {
    /// I blocchi di conflitto, nell'ordine del file. L'indice di un blocco qui è quello da
    /// passare a [`resolve_block`].
    pub fn blocks(&self) -> impl Iterator<Item = &ConflictBlock> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Conflict(block) => Some(block),
            Segment::Common(_) => None,
        })
    }

    /// Numero di blocchi di conflitto.
    pub fn block_count(&self) -> usize {
        self.blocks().count()
    }
}

/// Un pezzo del file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Segment {
    /// Byte fuori dai blocchi, a capo compresi. Mai vuoto.
    Common(Vec<u8>),
    Conflict(ConflictBlock),
}

/// Un blocco `<<<<<<<` ... `>>>>>>>`.
///
/// Le parti sono i byte delle righe fra i marcatori, a capo compresi (vuote se il lato non ha
/// righe); le etichette sono il testo dopo il marcatore senza lo spazio iniziale e senza l'a
/// capo, vuote se mancano.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictBlock {
    pub ours: Vec<u8>,
    /// Le righe fra `|||||||` e `=======`; `None` se il blocco non ha il marcatore della base
    /// (stile `merge`).
    pub base: Option<Vec<u8>>,
    pub theirs: Vec<u8>,
    pub ours_label: Vec<u8>,
    /// Vuota anche se il blocco non ha la base.
    pub base_label: Vec<u8>,
    pub theirs_label: Vec<u8>,
    /// Intervallo di byte del blocco nel testo letto, dal `<` iniziale all'a capo del marcatore
    /// `>>>>>>>` compreso (o alla fine del testo se quella riga non ha a capo).
    pub range: Range<usize>,
}

/// Con che cosa sostituire un blocco. In JSON: `"ours"`, `"theirs"`, `"base"`,
/// `"ours_then_theirs"`, `"theirs_then_ours"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    /// Le righe nostre.
    Ours,
    /// Le righe loro.
    Theirs,
    /// Le righe della base: solo per un blocco che la ha (stile `diff3`/`zdiff3`).
    Base,
    /// Le nostre e poi le loro.
    OursThenTheirs,
    /// Le loro e poi le nostre.
    TheirsThenOurs,
}

impl ConflictBlock {
    /// I byte che sostituiscono il blocco con la scelta data.
    ///
    /// [`Choice::Base`] su un blocco senza base dà [`DomainError::InvalidConflict`].
    pub fn resolved(&self, choice: Choice) -> Result<Vec<u8>, DomainError> {
        Ok(match choice {
            Choice::Ours => self.ours.clone(),
            Choice::Theirs => self.theirs.clone(),
            Choice::Base => self.base.clone().ok_or_else(|| invalid(
                "il blocco non ha la versione base (serve `merge.conflictStyle=diff3` o `zdiff3`)",
            ))?,
            Choice::OursThenTheirs => [self.ours.as_slice(), &self.theirs].concat(),
            Choice::TheirsThenOurs => [self.theirs.as_slice(), &self.ours].concat(),
        })
    }
}

/// Il tipo di marcatore, dal suo carattere.
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

/// Se la riga (senza a capo) è un marcatore, il suo tipo e l'etichetta.
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

/// La riga senza `\n` o `\r\n` finale.
fn strip_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn invalid(reason: impl Into<String>) -> DomainError {
    DomainError::InvalidConflict {
        reason: reason.into(),
    }
}

/// Dove si trova il parser dentro un blocco aperto.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Ours,
    Base,
    Theirs,
}

/// Divide il file del worktree in testo comune e blocchi di conflitto.
///
/// `marker_size` è la lunghezza dei marcatori (7, o l'attributo `conflict-marker-size` del
/// file); deve essere almeno 1. Un blocco non chiuso o con i marcatori fuori ordine dà
/// [`DomainError::InvalidConflict`] con il numero della riga (da 1). Un file senza blocchi dà
/// un solo [`Segment::Common`] (nessuno se è vuoto).
pub fn parse_conflicts(text: &[u8], marker_size: usize) -> Result<ConflictText, DomainError> {
    if marker_size == 0 {
        return Err(invalid("la lunghezza dei marcatori deve essere almeno 1"));
    }
    let mut segments = Vec::new();
    // Inizio del testo comune non ancora salvato.
    let mut common_start = 0;
    // Il blocco aperto, la parte in cui siamo e la riga del suo `<<<<<<<`.
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
            // Fuori da un blocco ogni altra riga, anche se sembra un marcatore, è testo.
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
                    Part::Ours => "`|||||||` o `=======`",
                    Part::Base => "`=======`",
                    Part::Theirs => "`>>>>>>>`",
                };
                return Err(invalid(format!(
                    "marcatore `{}` fuori ordine alla riga {number} nel blocco aperto alla riga \
                     {opened_at}: atteso {expected}",
                    kind.text()
                )));
            }
        };
        open = Some((block, next, opened_at));
    }
    if let Some((_, _, opened_at)) = open {
        return Err(invalid(format!(
            "il blocco aperto alla riga {opened_at} non è chiuso da `>>>>>>>`"
        )));
    }
    if common_start < text.len() {
        segments.push(Segment::Common(text[common_start..].to_vec()));
    }
    Ok(ConflictText { segments })
}

/// Il testo con il blocco numero `index` (da 0, in [`ConflictText::blocks`]) sostituito dalla
/// scelta; gli altri blocchi, risolti o no, restano come sono.
///
/// `text` deve essere quello letto da [`parse_conflicts`] per ottenere `parsed`: un indice
/// inesistente, o un intervallo che non sta nel testo, dà [`DomainError::InvalidConflict`].
pub fn resolve_block(
    text: &[u8],
    parsed: &ConflictText,
    index: usize,
    choice: Choice,
) -> Result<Vec<u8>, DomainError> {
    let count = parsed.block_count();
    let block = parsed.blocks().nth(index).ok_or_else(|| {
        invalid(format!(
            "il blocco {index} non esiste: il file ha {count} blocchi di conflitto"
        ))
    })?;
    replace(text, [(block, choice)])
}

/// Il testo con tutti i blocchi sostituiti dalla stessa scelta.
///
/// Come per [`resolve_block`], `text` deve essere quello letto da [`parse_conflicts`]. Con
/// [`Choice::Base`] serve che tutti i blocchi abbiano la base.
pub fn resolve_all(
    text: &[u8],
    parsed: &ConflictText,
    choice: Choice,
) -> Result<Vec<u8>, DomainError> {
    replace(text, parsed.blocks().map(|b| (b, choice)))
}

/// Copia `text` sostituendo gli intervalli dei blocchi dati (in ordine) con la loro scelta.
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
                "l'intervallo {start}..{end} del blocco non corrisponde al testo ({} byte): il \
                 file è cambiato dopo la lettura?",
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
