//! Parser dell'output di `git log` nel formato a campi fissi separati da NUL.
//!
//! # Formato
//!
//! Ogni commit è scritto come [`LOG_FIELDS`] campi, ciascuno **terminato** da un NUL:
//!
//! ```text
//! %H \0 %P \0 %an \0 %ae \0 %ad \0 %cn \0 %ce \0 %cd \0 %s \0 %b \0
//! ```
//!
//! Comportamento verificato con git 2.43:
//!
//! - nel formato i primi nove campi sono separati da `%x00`; il decimo NUL lo mette `-z`, che con
//!   `tformat:` (il default di `--format=`) **termina** ogni record, ultimo compreso. L'output
//!   quindi finisce sempre con un NUL, e un log senza commit è vuoto (zero byte). Con `format:`
//!   invece `-z` separa i record e l'ultimo non ha il NUL finale: per questo [`LOG_ARGS`] usa
//!   `tformat:` in modo esplicito;
//! - `%P` sono i genitori separati da uno spazio; è vuoto per un commit radice;
//! - `%ad` e `%cd` con `--date=raw` sono `<secondi> <+hhmm|-hhmm>`: a differenza di `%at`/`%ct`
//!   portano anche il fuso orario di chi ha firmato;
//! - `%s` è il primo paragrafo del messaggio con le righe unite da uno spazio (come in
//!   `git log --oneline`); `%b` è il resto, senza le righe vuote iniziali. `%b` è vuoto se il
//!   messaggio ha solo l'oggetto, altrimenti termina con `\n` (e con `--cleanup=verbatim` può
//!   terminare con più `\n`): il parser toglie gli a capo finali;
//! - un messaggio, un nome o un'email non possono contenere NUL, quindi contare i campi è
//!   univoco anche quando il corpo è vuoto o contiene righe vuote;
//! - `%an`/`%ae` sono i valori scritti nel commit, senza `.mailmap` (quelli sarebbero `%aN`/`%aE`);
//! - con `--encoding=UTF-8` git converte in UTF-8 i commit che dichiarano un'altra codifica
//!   (intestazione `encoding`); i commit senza intestazione passano così come sono e possono
//!   contenere byte non UTF-8, che qui diventano `U+FFFD` con `from_utf8_lossy`.

use super::{Commit, Oid, Signature};
use crate::DomainError;

/// Numero di campi per commit nel formato [`LOG_FORMAT`].
pub const LOG_FIELDS: usize = 10;

macro_rules! log_format {
    () => {
        "%H%x00%P%x00%an%x00%ae%x00%ad%x00%cn%x00%ce%x00%cd%x00%s%x00%b"
    };
}

/// La stringa di formato letta da [`parse_log`]. Da sola non basta: va usata con gli altri
/// argomenti di [`LOG_ARGS`], che la contiene già.
pub const LOG_FORMAT: &str = log_format!();

/// Argomenti obbligatori di `git log` per produrre l'input di [`parse_log`]. Chi chiama aggiunge
/// solo l'ordine (es. `--topo-order`), i limiti (es. `--max-count`) e il range; non vanno
/// aggiunte opzioni che cambiano l'output, come `--graph`, `--stat` o `--patch`.
///
/// - `-z`: NUL alla fine di ogni record;
/// - `--date=raw`: `%ad`/`%cd` con secondi e fuso orario;
/// - `--encoding=UTF-8`: messaggi in UTF-8 qualunque sia `i18n.logOutputEncoding`;
/// - `--no-show-signature`: niente output di gpg in mezzo ai record con `log.showSignature`;
/// - `--no-color`: niente sequenze di colore;
/// - `--format=tformat:...`: [`LOG_FORMAT`] con il NUL finale anche sull'ultimo record.
pub const LOG_ARGS: &[&str] = &[
    "-z",
    "--date=raw",
    "--encoding=UTF-8",
    "--no-show-signature",
    "--no-color",
    concat!("--format=tformat:", log_format!()),
];

/// Nomi dei campi, per i messaggi di errore.
const FIELD_NAMES: [&str; LOG_FIELDS] = [
    "id (%H)",
    "genitori (%P)",
    "nome dell'autore (%an)",
    "email dell'autore (%ae)",
    "data dell'autore (%ad)",
    "nome del committer (%cn)",
    "email del committer (%ce)",
    "data del committer (%cd)",
    "oggetto (%s)",
    "corpo (%b)",
];

/// Legge l'output di `git log` prodotto con [`LOG_ARGS`], nell'ordine in cui git lo ha scritto.
///
/// Un input vuoto dà un vettore vuoto. Un input che non termina con NUL, che ha un numero di
/// campi non multiplo di [`LOG_FIELDS`] o con id, genitori o date non validi dà
/// [`DomainError::InvalidGitOutput`], con il numero del commit (da 1) e il campo coinvolto.
pub fn parse_log(input: &[u8]) -> Result<Vec<Commit>, DomainError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = input.strip_suffix(b"\0") else {
        return Err(invalid(
            "l'output non termina con NUL: è troncato o non è stato prodotto con `-z` e \
             `--format=tformat:`"
                .to_owned(),
        ));
    };
    let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    let (records, rest) = fields.as_chunks::<LOG_FIELDS>();
    if !rest.is_empty() {
        return Err(invalid(format!(
            "il commit n. {} ha {} campi invece di {LOG_FIELDS}: il formato non è quello di \
             `LOG_FORMAT`",
            records.len() + 1,
            rest.len()
        )));
    }
    records
        .iter()
        .enumerate()
        .map(|(i, record)| parse_record(i + 1, record))
        .collect()
}

fn parse_record(n: usize, f: &[&[u8]; LOG_FIELDS]) -> Result<Commit, DomainError> {
    let field_err = |idx: usize, reason: String| {
        invalid(format!(
            "commit n. {n}, campo {}: {reason}",
            FIELD_NAMES[idx]
        ))
    };
    let oid = |idx: usize, hex: &[u8]| {
        Oid::from_hex(hex).map_err(|e| match e {
            DomainError::InvalidOid { input, reason } => {
                field_err(idx, format!("`{input}` non è un id valido ({reason})"))
            }
            other => other,
        })
    };
    let date = |idx: usize| parse_raw_date(f[idx]).map_err(|reason| field_err(idx, reason));

    let id = oid(0, f[0])?;
    let parents = f[1]
        .split(|&b| b == b' ')
        .filter(|p| !p.is_empty())
        .map(|p| oid(1, p))
        .collect::<Result<Vec<_>, _>>()?;
    let (author_time, author_offset) = date(4)?;
    let (committer_time, committer_offset) = date(7)?;
    Ok(Commit {
        id,
        parents,
        author: Signature {
            name: text(f[2]),
            email: text(f[3]),
            time: author_time,
            offset_min: author_offset,
        },
        committer: Signature {
            name: text(f[5]),
            email: text(f[6]),
            time: committer_time,
            offset_min: committer_offset,
        },
        summary: text(f[8]),
        body: text(trim_trailing_newlines(f[9])),
    })
}

/// Legge una data `--date=raw`: `<secondi> <segno><hh><mm>`, es. `1700000000 -0530`.
/// Restituisce i secondi e il fuso in minuti (`-330` nell'esempio).
fn parse_raw_date(raw: &[u8]) -> Result<(i64, i32), String> {
    let shown = || String::from_utf8_lossy(raw).into_owned();
    let bad = |what: &str| format!("data `{}` non valida: {what}", shown());
    let Some(space) = raw.iter().position(|&b| b == b' ') else {
        return Err(bad(
            "manca il fuso orario (formato atteso `<secondi> <+hhmm>`)",
        ));
    };
    let (secs, tz) = (&raw[..space], &raw[space + 1..]);
    let time = parse_digits(secs)
        .and_then(|v| i64::try_from(v).ok())
        .ok_or_else(|| bad("i secondi devono essere un numero intero non negativo"))?;
    let sign = match tz.first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Err(bad("il fuso orario deve iniziare con `+` o `-`")),
    };
    let hhmm = &tz[1..];
    if hhmm.len() != 4 {
        return Err(bad("il fuso orario deve avere quattro cifre (`+hhmm`)"));
    }
    let (hh, mm) = hhmm.split_at(2);
    let hours = parse_digits(hh).ok_or_else(|| bad("ore del fuso non numeriche"))?;
    let minutes = parse_digits(mm).ok_or_else(|| bad("minuti del fuso non numerici"))?;
    // Al massimo 99 * 60 + 99: sta sempre in un i32.
    let offset = i32::try_from(hours * 60 + minutes).unwrap_or(i32::MAX);
    Ok((time, sign * offset))
}

/// Cifre decimali ASCII (almeno una) in un `u64`; `None` se vuoto, non numerico o troppo grande.
fn parse_digits(s: &[u8]) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    s.iter().try_fold(0u64, |acc, &b| {
        if b.is_ascii_digit() {
            acc.checked_mul(10)?.checked_add(u64::from(b - b'0'))
        } else {
            None
        }
    })
}

fn trim_trailing_newlines(s: &[u8]) -> &[u8] {
    let end = s.iter().rposition(|&b| b != b'\n').map_or(0, |i| i + 1);
    &s[..end]
}

fn text(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "log",
        reason,
    }
}
