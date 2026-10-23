//! Editor dei conflitti (`conflict.show`, `conflict.resolve`, S30): tipi e controlli comuni ai
//! backend (`impl-docs/git/conflitti.md`).
//!
//! Il comando legge le versioni di un file in conflitto con
//! [`GitBackend::conflict_versions`](super::GitBackend::conflict_versions) e le mostra con un
//! [`worktree_digest`] dei byte del worktree; `conflict.resolve` ripassa quel digest e il
//! backend ([`GitBackend::resolve_conflict`](super::GitBackend::resolve_conflict)) scrive il
//! file solo se il worktree è ancora quello visto, dopo un backup dei suoi byte.

use crate::EngineError;
use project_domain::{Conflict, EntryKind, RepoPath, Status, UnmergedInfo};
use serde::Serialize;

/// Dimensione dei marcatori di conflitto se l'attributo `conflict-marker-size` non c'è.
pub const DEFAULT_MARKER_SIZE: usize = 7;

/// Modo di un sottomodulo nell'indice.
const SUBMODULE_MODE: u32 = 0o160000;
/// Tipo di oggetto di un link simbolico.
const SYMLINK_MODE: u32 = 0o120000;

/// Le versioni di un file in conflitto ([`GitBackend::conflict_versions`](super::GitBackend::conflict_versions)):
/// byte senza filtri degli stadi 1/2/3 dell'indice e del worktree, `None` se mancano.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictVersions {
    pub path: RepoPath,
    pub kind: Conflict,
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
    pub worktree: Option<Vec<u8>>,
    /// Dall'attributo `conflict-marker-size` ([`DEFAULT_MARKER_SIZE`] se manca o non è valido).
    pub marker_size: usize,
}

/// Una versione per `conflict.show`: `text` se i byte sono UTF-8 valido senza NUL, altrimenti
/// `None` e `binary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VersionText {
    pub text: Option<String>,
    pub binary: bool,
    pub size: usize,
}

impl VersionText {
    pub fn of(bytes: &[u8]) -> Self {
        let text = (!bytes.contains(&0))
            .then(|| std::str::from_utf8(bytes).ok())
            .flatten()
            .map(str::to_owned);
        Self {
            binary: text.is_none(),
            text,
            size: bytes.len(),
        }
    }
}

/// Cosa chiede `conflict.resolve` al backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictResolution {
    pub path: RepoPath,
    /// Il [`worktree_digest`] visto da `conflict.show`.
    pub digest: String,
    /// I byte da scrivere nel worktree; `None` cancella il file (`delete`).
    pub content: Option<Vec<u8>>,
}

/// Esito di [`GitBackend::resolve_conflict`](super::GitBackend::resolve_conflict).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictResolved {
    /// Il ref del backup dei byte del worktree (`refs/project/backup/conflict-...`), `None` se
    /// il file non c'era.
    pub backup: Option<String>,
    /// Il ref del backup dell'indice di `index.stage`, se c'è stato.
    pub index_backup: Option<String>,
}

/// Identifica i byte del worktree mostrati da `conflict.show`: FNV-1a a 128 bit (come
/// [`file_digest`](super::file_digest)), 32 cifre esadecimali. Il file assente ha un valore
/// suo: si calcola su un byte di tipo (`0` assente, `1` presente) seguito dai byte, così
/// nessun contenuto ha il digest del file assente.
pub fn worktree_digest(content: Option<&[u8]>) -> String {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let step = |h: u128, b: u8| (h ^ u128::from(b)).wrapping_mul(PRIME);
    let hash = match content {
        None => step(OFFSET, 0),
        Some(bytes) => bytes.iter().fold(step(OFFSET, 1), |h, &b| step(h, b)),
    };
    format!("{hash:032x}")
}

/// La dimensione dei marcatori dal valore dell'attributo `conflict-marker-size` (`None`,
/// `unspecified`, `set`, `unset` o un numero): un numero positivo, altrimenti
/// [`DEFAULT_MARKER_SIZE`] (come git, che legge il numero con `atoi`).
pub fn marker_size(value: Option<&str>) -> usize {
    value
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MARKER_SIZE)
}

/// Il conflitto di `path` nello stato: deve essere una voce `u` di un file (non un
/// sottomodulo, non un link nel worktree), altrimenti [`EngineError::NotAllowed`].
pub fn check_conflict<'a>(
    command: &str,
    status: &'a Status,
    path: &RepoPath,
) -> Result<&'a UnmergedInfo, EngineError> {
    let info = status
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Unmerged(info) if e.path == *path => Some(info),
            _ => None,
        })
        .ok_or_else(|| {
            EngineError::NotAllowed(format!(
                "`{path}` non è un file in conflitto: {command} lavora solo sui file non uniti \
                 di un merge (o rebase, cherry-pick...) fermo; niente modificato"
            ))
        })?;
    let modes = [
        info.mode_base,
        info.mode_ours,
        info.mode_theirs,
        info.mode_worktree,
    ];
    if info.submodule.is_some() || modes.contains(&SUBMODULE_MODE) {
        return Err(EngineError::NotAllowed(format!(
            "`{path}` è un sottomodulo in conflitto: l'editor dei conflitti lavora solo sui \
             file; risolverlo scegliendo il commit del sottomodulo e mettendolo in stage"
        )));
    }
    if info.mode_worktree & 0o170000 == SYMLINK_MODE {
        return Err(EngineError::not_a_file(path));
    }
    Ok(info)
}

/// Il digest visto (`expected`) è ancora quello del worktree (`content`), altrimenti
/// [`EngineError::StaleDiff`]. `note` si aggiunge al motivo (es. il backup già salvato).
pub fn check_digest(
    path: &RepoPath,
    expected: &str,
    content: Option<&[u8]>,
    note: &str,
) -> Result<(), EngineError> {
    if worktree_digest(content) == expected {
        return Ok(());
    }
    Err(EngineError::StaleDiff {
        path: path.to_string(),
        reason: format!("il file del worktree è cambiato dopo `conflict.show`, rileggerlo{note}"),
    })
}

impl EngineError {
    /// [`EngineError::NotAllowed`] per un percorso in conflitto che nel worktree è un link
    /// simbolico o una cartella.
    pub fn not_a_file(path: &RepoPath) -> Self {
        EngineError::NotAllowed(format!(
            "`{path}` nel worktree è un link simbolico o una cartella, non un file: l'editor \
             dei conflitti lavora solo sui file; niente modificato"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_distinguishes_missing_and_empty() {
        let missing = worktree_digest(None);
        let empty = worktree_digest(Some(b""));
        assert_ne!(missing, empty);
        assert_eq!(missing.len(), 32);
        assert_eq!(empty, worktree_digest(Some(b"")));
        assert_ne!(worktree_digest(Some(b"a")), worktree_digest(Some(b"b")));
    }

    #[test]
    fn marker_size_from_attribute() {
        assert_eq!(marker_size(None), 7);
        assert_eq!(marker_size(Some("unspecified")), 7);
        assert_eq!(marker_size(Some("set")), 7);
        assert_eq!(marker_size(Some("0")), 7);
        assert_eq!(marker_size(Some("-3")), 7);
        assert_eq!(marker_size(Some("12")), 12);
    }

    #[test]
    fn text_or_binary() {
        assert_eq!(
            VersionText::of(b"ciao\n"),
            VersionText {
                text: Some("ciao\n".into()),
                binary: false,
                size: 5
            }
        );
        assert!(VersionText::of(b"a\0b").binary);
        assert!(VersionText::of(&[0xff, 0xfe]).text.is_none());
    }
}
