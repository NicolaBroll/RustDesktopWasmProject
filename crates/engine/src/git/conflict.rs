//! Conflict editor (`conflict.show`, `conflict.resolve`, S30): types and checks shared by the
//! backends (`impl-docs/git/conflicts.md`).
//!
//! The command reads the versions of a conflicted file with
//! [`GitBackend::conflict_versions`](super::GitBackend::conflict_versions) and shows them with a
//! [`worktree_digest`] of the worktree bytes; `conflict.resolve` passes that digest back and the
//! backend ([`GitBackend::resolve_conflict`](super::GitBackend::resolve_conflict)) writes the
//! file only if the worktree is still the one that was seen, after a backup of its bytes.

use crate::EngineError;
use project_domain::{Conflict, EntryKind, RepoPath, Status, UnmergedInfo};
use serde::{Deserialize, Serialize};

/// Size of the conflict markers if the `conflict-marker-size` attribute is not set.
pub const DEFAULT_MARKER_SIZE: usize = 7;

/// Mode of a submodule in the index.
const SUBMODULE_MODE: u32 = 0o160000;
/// Object type of a symbolic link.
const SYMLINK_MODE: u32 = 0o120000;

/// The versions of a conflicted file ([`GitBackend::conflict_versions`](super::GitBackend::conflict_versions)):
/// unfiltered bytes of stages 1/2/3 of the index and of the worktree, `None` if missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictVersions {
    pub path: RepoPath,
    pub kind: Conflict,
    pub base: Option<Vec<u8>>,
    pub ours: Option<Vec<u8>>,
    pub theirs: Option<Vec<u8>>,
    pub worktree: Option<Vec<u8>>,
    /// From the `conflict-marker-size` attribute ([`DEFAULT_MARKER_SIZE`] if missing or not valid).
    pub marker_size: usize,
}

/// A version for `conflict.show`: `text` if the bytes are valid UTF-8 without NUL, otherwise
/// `None` and `binary`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionText {
    /// The content, if it is valid UTF-8 without NUL.
    pub text: Option<String>,
    /// The content is not text (`text` is `None`).
    pub binary: bool,
    /// The size in bytes.
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

/// What `conflict.resolve` asks of the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictResolution {
    pub path: RepoPath,
    /// The [`worktree_digest`] seen by `conflict.show`.
    pub digest: String,
    /// The bytes to write in the worktree; `None` deletes the file (`delete`).
    pub content: Option<Vec<u8>>,
}

/// Outcome of [`GitBackend::resolve_conflict`](super::GitBackend::resolve_conflict).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConflictResolved {
    /// The backup ref of the worktree bytes (`refs/project/backup/conflict-...`), `None` if
    /// the file was not there.
    pub backup: Option<String>,
    /// The backup ref of the index from `index.stage`, if there was one.
    pub index_backup: Option<String>,
}

/// Identifies the worktree bytes shown by `conflict.show`: 128-bit FNV-1a (like
/// [`file_digest`](super::file_digest)), 32 hex digits. The missing file has a value
/// of its own: computed over a type byte (`0` missing, `1` present) followed by the bytes, so
/// no content has the digest of the missing file.
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

/// The marker size from the value of the `conflict-marker-size` attribute (`None`,
/// `unspecified`, `set`, `unset` or a number): a positive number, otherwise
/// [`DEFAULT_MARKER_SIZE`] (like git, which reads the number with `atoi`).
pub fn marker_size(value: Option<&str>) -> usize {
    value
        .and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MARKER_SIZE)
}

/// The conflict of `path` in the status: it must be a `u` entry of a file (not a
/// submodule, not a link in the worktree), otherwise [`EngineError::NotAllowed`].
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
            EngineError::NotAllowed(msg!(
                "engine-conflict-not-conflicted",
                path = path,
                command = command
            ))
        })?;
    let modes = [
        info.mode_base,
        info.mode_ours,
        info.mode_theirs,
        info.mode_worktree,
    ];
    if info.submodule.is_some() || modes.contains(&SUBMODULE_MODE) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-conflict-submodule",
            path = path
        )));
    }
    if info.mode_worktree & 0o170000 == SYMLINK_MODE {
        return Err(EngineError::not_a_file(path));
    }
    Ok(info)
}

/// The seen digest (`expected`) is still that of the worktree (`content`), otherwise
/// [`EngineError::StaleDiff`].
pub fn check_digest(
    path: &RepoPath,
    expected: &str,
    content: Option<&[u8]>,
) -> Result<(), EngineError> {
    if worktree_digest(content) == expected {
        return Ok(());
    }
    Err(EngineError::StaleDiff {
        path: path.to_string(),
        reason: msg!("engine-conflict-worktree-changed"),
    })
}

impl EngineError {
    /// [`EngineError::NotAllowed`] for a conflicted path that in the worktree is a symbolic
    /// link or a folder.
    pub fn not_a_file(path: &RepoPath) -> Self {
        EngineError::NotAllowed(msg!("engine-conflict-not-a-file", path = path))
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
            VersionText::of(b"hello\n"),
            VersionText {
                text: Some("hello\n".into()),
                binary: false,
                size: 6
            }
        );
        assert!(VersionText::of(b"a\0b").binary);
        assert!(VersionText::of(&[0xff, 0xfe]).text.is_none());
    }
}
