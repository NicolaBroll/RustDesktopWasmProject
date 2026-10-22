//! Parser dell'output di `git for-each-ref` nel formato a campi terminati da NUL.
//!
//! # Formato
//!
//! Ogni ref è scritto come [`REFS_FIELDS`] campi, ciascuno **terminato** da un NUL; dopo il
//! formato git aggiunge sempre un a capo (`for-each-ref` non ha `-z`):
//!
//! ```text
//! %(refname) \0 %(objectname) \0 %(*objectname) \0 %(upstream) \0 %(upstream:track,nobracket) \0 %(symref) \0 \n
//! ```
//!
//! Il parser divide l'input **solo sui NUL**: l'a capo di git resta all'inizio del primo campo
//! del record successivo (e da solo dopo l'ultimo NUL) e viene tolto. Così il formato non è
//! ambiguo anche se un campo contiene un a capo. Comportamento verificato con git 2.43:
//!
//! - un nome di ref non può contenere NUL, a capo o altri caratteri di controllo
//!   (`git check-ref-format` li rifiuta), ma può contenere byte non ASCII e anche non UTF-8;
//! - `%(upstream)` invece viene da `branch.<b>.merge` nella config e **può** contenere un a capo
//!   (i valori di config non possono contenere NUL): per questo non si divide sulle righe;
//! - un repo senza ref dà un output vuoto (zero byte);
//! - `%(*objectname)` è l'oggetto puntato da un tag annotato, vuoto per gli altri ref; git 2.43
//!   toglie un solo livello, quindi per un tag di un tag è l'id del tag interno;
//! - `%(upstream)` è il nome completo (es. `refs/remotes/origin/main`, o `refs/heads/main` per un
//!   upstream locale con `branch.<b>.remote = .`), vuoto se il branch non ne ha uno;
//! - `%(upstream:track,nobracket)` è vuoto (in pari, o senza upstream), `ahead <n>`,
//!   `behind <m>`, `ahead <n>, behind <m>` oppure `gone` se il ref dell'upstream non esiste più;
//! - `%(symref)` è il ref puntato da un ref simbolico (es. `refs/remotes/origin/HEAD`), vuoto
//!   per gli altri; un ref simbolico rotto non compare affatto.

use std::collections::HashMap;

use super::log::parse_digits;
use super::{Oid, RefInfo, RefKind, Upstream};
use crate::DomainError;

/// Numero di campi per ref nel formato [`REFS_FORMAT`].
pub const REFS_FIELDS: usize = 6;

macro_rules! refs_format {
    () => {
        "%(refname)%00%(objectname)%00%(*objectname)%00%(upstream)%00%(upstream:track,nobracket)%00%(symref)%00"
    };
}

/// La stringa di formato letta da [`parse_refs`]. È già contenuta in [`REFS_ARGS`].
pub const REFS_FORMAT: &str = refs_format!();

/// Argomenti di `git for-each-ref` per produrre l'input di [`parse_refs`]: il formato
/// ([`REFS_FORMAT`]) e i pattern dei ref letti (branch locali, remoti, tag e `refs/stash`).
/// Chi chiama può aggiungere filtri come `--merged` o `--points-at`, ma non `--format`,
/// `--shell`/`--perl`/`--python`/`--tcl` (cambiano il formato) né altri pattern: i ref fuori da
/// questi tipi verrebbero comunque ignorati.
///
/// L'ordine è quello di default di git, per nome del ref (byte per byte).
pub const REFS_ARGS: &[&str] = &[
    concat!("--format=", refs_format!()),
    "refs/heads/",
    "refs/remotes/",
    "refs/tags/",
    "refs/stash",
];

/// Nomi dei campi, per i messaggi di errore.
const FIELD_NAMES: [&str; REFS_FIELDS] = [
    "nome (%(refname))",
    "id (%(objectname))",
    "id puntato dal tag (%(*objectname))",
    "upstream (%(upstream))",
    "ahead/behind (%(upstream:track,nobracket))",
    "ref simbolico (%(symref))",
];

/// Legge l'output di `git for-each-ref` prodotto con [`REFS_ARGS`].
///
/// I ref restano nell'ordine di git. Regole:
///
/// - il tipo viene dal prefisso: `refs/heads/` → [`RefKind::LocalBranch`], `refs/remotes/` →
///   [`RefKind::RemoteBranch`], `refs/tags/` → [`RefKind::Tag`], esattamente `refs/stash` →
///   [`RefKind::Stash`]. Gli altri ref (es. `refs/notes/*`, `refs/stash/x`) sono **ignorati**:
///   i pattern di [`REFS_ARGS`] non li fanno uscire, ma se ci sono non sono un errore;
/// - i ref simbolici (`%(symref)` non vuoto, es. `refs/remotes/origin/HEAD`) sono **ignorati**:
///   duplicano il ref a cui puntano, che è già nella lista;
/// - i ref con un nome non UTF-8 sono **ignorati**: [`RefInfo::name`] è una `String` e un nome
///   convertito con perdita indicherebbe a git un ref diverso;
/// - per un tag annotato [`RefInfo::target`] è l'oggetto puntato dal tag e
///   [`RefInfo::tag_object`] l'id del tag. Se il tag punta a un altro tag annotato presente
///   nella lista si risale fino in fondo; un tag su un albero o un blob ha come `target` quel
///   oggetto, che non è un commit;
/// - l'upstream ha il nome completo; ahead/behind sono `0`/`0` se in pari e `None` se git
///   risponde `gone`. Un nome di upstream non UTF-8 è convertito con `from_utf8_lossy` (solo da
///   mostrare: per i comandi usare `<branch>@{upstream}`).
///
/// Un input che non termina con NUL e a capo, con un numero di campi che non è multiplo di
/// [`REFS_FIELDS`] o con campi non validi dà [`DomainError::InvalidGitOutput`], con il numero
/// del record (da 1) coinvolto.
pub fn parse_refs(input: &[u8]) -> Result<Vec<RefInfo>, DomainError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let body = input.strip_suffix(b"\0\n").ok_or_else(|| {
        invalid(
            "l'output non termina con NUL e a capo: è troncato o non è stato prodotto con \
             il formato di REFS_ARGS"
                .to_owned(),
        )
    })?;
    let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    let (records, rest) = fields.as_chunks::<REFS_FIELDS>();
    if !rest.is_empty() {
        let n = records.len() + 1;
        let have = rest.len();
        return Err(invalid(format!(
            "record n. {n}: {have} campi invece di {REFS_FIELDS} (manca {})",
            FIELD_NAMES[have]
        )));
    }
    let mut refs = Vec::new();
    for (i, record) in records.iter().enumerate() {
        let n = i + 1;
        if let Some(info) = parse_record(record, i == 0)
            .map_err(|reason| invalid(format!("record n. {n}: {reason}")))?
        {
            refs.push(info);
        }
    }
    peel_nested_tags(&mut refs);
    Ok(refs)
}

/// Legge un record; `None` se il ref va ignorato.
fn parse_record(f: &[&[u8]; REFS_FIELDS], first: bool) -> Result<Option<RefInfo>, String> {
    let name = if first {
        f[0]
    } else {
        f[0].strip_prefix(b"\n")
            .ok_or_else(|| format!("manca l'a capo prima di {}", FIELD_NAMES[0]))?
    };
    if name.is_empty() {
        return Err(format!("{} vuoto", FIELD_NAMES[0]));
    }
    let object = parse_oid(f[1], 1)?;
    let peeled = if f[2].is_empty() {
        None
    } else {
        Some(parse_oid(f[2], 2)?)
    };
    let upstream = parse_upstream(f[3], f[4])?;
    if !f[5].is_empty() {
        return Ok(None);
    }
    let Ok(name) = std::str::from_utf8(name) else {
        return Ok(None);
    };
    let Some(kind) = kind_of(name) else {
        return Ok(None);
    };
    let (target, tag_object) = match peeled {
        Some(peeled) => (peeled, Some(object)),
        None => (object, None),
    };
    Ok(Some(RefInfo {
        name: name.to_owned(),
        kind,
        target,
        tag_object,
        upstream,
    }))
}

fn kind_of(name: &str) -> Option<RefKind> {
    if name == "refs/stash" {
        Some(RefKind::Stash)
    } else if name.starts_with("refs/heads/") {
        Some(RefKind::LocalBranch)
    } else if name.starts_with("refs/remotes/") {
        Some(RefKind::RemoteBranch)
    } else if name.starts_with("refs/tags/") {
        Some(RefKind::Tag)
    } else {
        None
    }
}

fn parse_oid(field: &[u8], i: usize) -> Result<Oid, String> {
    Oid::from_hex(field).map_err(|e| format!("{}: {e}", FIELD_NAMES[i]))
}

fn parse_upstream(name: &[u8], track: &[u8]) -> Result<Option<Upstream>, String> {
    if name.is_empty() {
        return if track.is_empty() {
            Ok(None)
        } else {
            Err(format!(
                "{} è `{}` ma {} è vuoto",
                FIELD_NAMES[4],
                show(track),
                FIELD_NAMES[3]
            ))
        };
    }
    let (ahead, behind) = parse_track(track)
        .ok_or_else(|| format!("{} non valido: `{}`", FIELD_NAMES[4], show(track)))?;
    Ok(Some(Upstream {
        name: show(name),
        ahead,
        behind,
    }))
}

/// `""` → `0`/`0`, `gone` → `None`/`None`, altrimenti `ahead <n>`, `behind <m>` o
/// `ahead <n>, behind <m>`.
pub(super) fn parse_track(track: &[u8]) -> Option<(Option<u32>, Option<u32>)> {
    match track {
        b"" => return Some((Some(0), Some(0))),
        b"gone" => return Some((None, None)),
        _ => {}
    }
    let count = |s: &[u8]| parse_digits(s).and_then(|n| u32::try_from(n).ok());
    let (ahead, rest) = match track.strip_prefix(b"ahead ") {
        Some(rest) => match rest.iter().position(|&b| b == b',') {
            Some(i) => (count(&rest[..i])?, rest[i..].strip_prefix(b", ")?),
            None => return Some((Some(count(rest)?), Some(0))),
        },
        None => (0, track),
    };
    let behind = count(rest.strip_prefix(b"behind ")?)?;
    Some((Some(ahead), Some(behind)))
}

/// Per i tag di tag: se `target` è l'oggetto di un altro tag annotato della lista, lo sostituisce
/// con il suo `target`, fino in fondo.
fn peel_nested_tags(refs: &mut [RefInfo]) {
    let peeled: HashMap<Oid, Oid> = refs
        .iter()
        .filter_map(|r| r.tag_object.map(|tag| (tag, r.target)))
        .collect();
    for r in refs.iter_mut().filter(|r| r.tag_object.is_some()) {
        // Gli id sono hash del contenuto, quindi non ci sono cicli; il limite protegge comunque
        // da un input costruito apposta.
        for _ in 0..peeled.len() {
            match peeled.get(&r.target) {
                Some(&next) if next != r.target => r.target = next,
                _ => break,
            }
        }
    }
}

fn show(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "for-each-ref",
        reason,
    }
}
