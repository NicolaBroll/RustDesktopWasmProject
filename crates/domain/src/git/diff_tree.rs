//! Parser dell'output di `git diff-tree` nel formato `--raw -z`: i file cambiati da un commit.
//!
//! # Formato
//!
//! Con [`DIFF_TREE_ARGS`] ogni file cambiato è un record di due o tre campi, ciascuno
//! **terminato** da un NUL:
//!
//! ```text
//! :<modo vecchio> <modo nuovo> <id vecchio> <id nuovo> <stato>[<punteggio>] \0 <percorso> \0
//! :<modo vecchio> <modo nuovo> <id vecchio> <id nuovo> R<punteggio> \0 <origine> \0 <destinazione> \0
//! ```
//!
//! Comportamento verificato con git 2.43:
//!
//! - i modi sono in ottale a sei cifre, `000000` per il lato che non esiste (file aggiunto o
//!   eliminato); gli id sono completi (con `--no-abbrev`), tutti zeri per il lato che non esiste;
//! - lo stato è una lettera: `A` aggiunto, `D` eliminato, `M` modificato (anche solo nel modo, es.
//!   reso eseguibile), `T` cambiato il tipo (es. da file a link simbolico), `R` rinominato e `C`
//!   copiato. `R` e `C` sono seguiti dalla somiglianza in percentuale e hanno due percorsi:
//!   prima l'origine, poi la destinazione. Le copie compaiono solo con `-C`, che
//!   [`DIFF_TREE_ARGS`] non usa; il parser le legge comunque;
//! - con `-z` i percorsi non sono mai tra virgolette né con escape: possono contenere spazi, a
//!   capo e byte non UTF-8, e restano byte esatti in [`RepoPath`];
//! - `-r` scende nelle sottocartelle: un record per file, mai per cartella. Un sottomodulo è un
//!   record con modo `160000`;
//! - un commit che non cambia niente (es. `--allow-empty`) dà un output vuoto;
//! - senza `--no-commit-id`, con un solo commit come argomento, git scrive prima l'id del commit
//!   seguito da un NUL: [`DIFF_TREE_ARGS`] lo esclude.

use super::log::parse_digits;
use super::status::{mode, oid, show};
use super::{Change, Commit, Oid, RepoPath};
use crate::DomainError;
use serde::{Deserialize, Serialize};

/// Argomenti obbligatori di `git diff-tree` per produrre l'input di [`parse_diff_tree`].
///
/// Chi chiama aggiunge le due revisioni da confrontare (`<genitore> <commit>`), oppure
/// `--root <commit>` per un commit senza genitori (confronto con l'albero vuoto); non vanno
/// aggiunte opzioni che cambiano il formato, come `--name-status`, `--stat` o `--patch`.
///
/// - `-z`: record terminati da NUL e percorsi senza virgolette né escape;
/// - `-r`: un record per file anche dentro le sottocartelle;
/// - `--raw`: modi, id e stato per ogni file;
/// - `--no-abbrev`: id completi;
/// - `-M`: rinomine riconosciute (`diff-tree` non legge `diff.renames` dalla configurazione);
/// - `--no-commit-id`: niente id del commit in testa all'output.
pub const DIFF_TREE_ARGS: &[&str] = &["-z", "-r", "--raw", "--no-abbrev", "-M", "--no-commit-id"];

/// Un file cambiato da un commit rispetto al genitore con cui è confrontato.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// Percorso nel commit (per un file eliminato, quello che aveva prima; per una rinomina o
    /// una copia, la destinazione).
    pub path: RepoPath,
    /// Il tipo di modifica: solo [`Change::Added`], [`Change::Deleted`], [`Change::Modified`],
    /// [`Change::TypeChanged`], [`Change::Renamed`] o [`Change::Copied`].
    pub change: Change,
    /// Solo per rinomine e copie: il percorso di partenza.
    pub original: Option<RepoPath>,
    /// Somiglianza in percentuale (0–100) per rinomine e copie; `None` per gli altri stati.
    pub score: Option<u8>,
    /// Modo prima del commit, in ottale (es. `0o100644`, `0o100755`, `0o120000` per un link,
    /// `0o160000` per un sottomodulo); `0` se il file non esisteva.
    pub old_mode: u32,
    /// Modo dopo il commit; `0` se il file è stato eliminato.
    pub new_mode: u32,
    /// Id del contenuto prima del commit; tutti zeri se il file non esisteva.
    pub old_oid: Oid,
    /// Id del contenuto dopo il commit; tutti zeri se il file è stato eliminato.
    pub new_oid: Oid,
}

impl ChangedFile {
    /// Percorso prima del commit: l'origine per rinomine e copie, altrimenti [`Self::path`].
    pub fn old_path(&self) -> &RepoPath {
        self.original.as_ref().unwrap_or(&self.path)
    }

    /// Vero se il percorso è un sottomodulo (modo `160000`) prima o dopo il commit.
    pub fn is_submodule(&self) -> bool {
        const GITLINK: u32 = 0o160000;
        self.old_mode == GITLINK || self.new_mode == GITLINK
    }
}

/// Dettagli di un commit: metadati e file cambiati (risultato di `commit.show`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitDetails {
    pub commit: Commit,
    /// Il commit con cui sono confrontati i file: il **primo genitore** (anche per un merge,
    /// così i file sono quelli portati dal merge nel branch su cui è stato fatto); `None` per un
    /// commit radice, confrontato con l'albero vuoto.
    pub compared_to: Option<Oid>,
    /// File cambiati, nell'ordine di git (per percorso, byte per byte).
    pub files: Vec<ChangedFile>,
}

/// Legge l'output di `git diff-tree` prodotto con [`DIFF_TREE_ARGS`].
///
/// Un input vuoto dà un vettore vuoto. Un input che non termina con NUL, con un record
/// incompleto, uno stato sconosciuto o campi non validi dà [`DomainError::InvalidGitOutput`],
/// con il numero del record (da 1) coinvolto.
pub fn parse_diff_tree(input: &[u8]) -> Result<Vec<ChangedFile>, DomainError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let Some(body) = input.strip_suffix(b"\0") else {
        return Err(invalid(
            "l'output non termina con NUL: è troncato o non è stato prodotto con `-z`".to_owned(),
        ));
    };
    let mut tokens = body.split(|&b| b == 0);
    let mut files = Vec::new();
    while let Some(header) = tokens.next() {
        let n = files.len() + 1;
        let err = |reason: String| invalid(format!("record n. {n}: {reason}"));
        let mut next_path = |what: &str| match tokens.next() {
            Some([]) => Err(err(format!("{what} vuoto"))),
            Some(p) => Ok(RepoPath::from(p)),
            None => Err(err(format!("manca {what}"))),
        };
        let raw = parse_header(header).map_err(err)?;
        let first = next_path("il percorso")?;
        let (path, original) = if matches!(raw.change, Change::Renamed | Change::Copied) {
            (next_path("il percorso di destinazione")?, Some(first))
        } else {
            (first, None)
        };
        files.push(ChangedFile {
            path,
            change: raw.change,
            original,
            score: raw.score,
            old_mode: raw.old_mode,
            new_mode: raw.new_mode,
            old_oid: raw.old_oid,
            new_oid: raw.new_oid,
        });
    }
    Ok(files)
}

struct RawHeader {
    old_mode: u32,
    new_mode: u32,
    old_oid: Oid,
    new_oid: Oid,
    change: Change,
    score: Option<u8>,
}

/// `:<mH> <mN> <hH> <hN> <stato>`.
fn parse_header(header: &[u8]) -> Result<RawHeader, String> {
    let Some(rest) = header.strip_prefix(b":") else {
        return Err(format!(
            "`{}` non è un record `--raw` (deve iniziare con `:`)",
            show(header)
        ));
    };
    let f: Vec<&[u8]> = rest.split(|&b| b == b' ').collect();
    let [old_mode, new_mode, old_oid, new_oid, status] = f[..] else {
        return Err(format!(
            "intestazione `{}` con {} campi invece di 5",
            show(header),
            f.len()
        ));
    };
    let (change, score) = match status {
        [letter, score @ ..] => {
            let change = match letter {
                b'A' => Change::Added,
                b'D' => Change::Deleted,
                b'M' => Change::Modified,
                b'T' => Change::TypeChanged,
                b'R' => Change::Renamed,
                b'C' => Change::Copied,
                _ => return Err(format!("stato `{}` sconosciuto", show(status))),
            };
            let needs_score = matches!(change, Change::Renamed | Change::Copied);
            let score = match (needs_score, score) {
                (false, []) => None,
                // `M` può avere una dissimilarità solo con `-B`, che non usiamo.
                (false, _) => return Err(format!("stato `{}` inatteso", show(status))),
                (true, digits) => Some(
                    parse_digits(digits)
                        .and_then(|v| u8::try_from(v).ok())
                        .filter(|v| *v <= 100)
                        .ok_or_else(|| {
                            format!(
                                "`{}` non è un punteggio valido (atteso `{}<0-100>`)",
                                show(status),
                                char::from(*letter)
                            )
                        })?,
                ),
            };
            (change, score)
        }
        [] => return Err("stato vuoto".to_owned()),
    };
    Ok(RawHeader {
        old_mode: mode(old_mode)?,
        new_mode: mode(new_mode)?,
        old_oid: oid(old_oid, "id vecchio")?,
        new_oid: oid(new_oid, "id nuovo")?,
        change,
        score,
    })
}

fn invalid(reason: String) -> DomainError {
    DomainError::InvalidGitOutput {
        command: "diff-tree",
        reason,
    }
}
