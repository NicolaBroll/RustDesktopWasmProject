//! Git LFS (S63, `impl-docs/git/reading.md` "Git LFS"): which files are stored in LFS and whether
//! a repo that uses LFS can be worked on. Reads only: the app never runs `git lfs` commands, the
//! LFS work is done by the user's `filter.lfs.*` and git-lfs hooks, as in a terminal.

use serde::{Deserialize, Serialize};

/// The value of the `filter` attribute of the files stored in Git LFS (`*.bin filter=lfs`).
pub const LFS_FILTER: &[u8] = b"lfs";

/// LFS in the open repo ([`GitRead::lfs_state`](super::GitRead::lfs_state), `repo.lfs_status`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LfsState {
    /// At least one file of the index has the `filter` attribute `lfs`.
    pub used: bool,
    /// git-lfs is installed: `git lfs version` succeeds.
    pub installed: bool,
}

impl LfsState {
    /// The repo uses LFS but git-lfs is missing: LFS files stay as pointer text in the
    /// worktree, and committing them again would store that text (the notice of the UI).
    pub fn missing_git_lfs(&self) -> bool {
        self.used && !self.installed
    }
}
