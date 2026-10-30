//! Repository maintenance (`repo.optimize`, S36): the commit-graph
//! (`impl-docs/git/reading.md`).
//!
//! Without a commit-graph `git log --date-order` (the order of `repo.log`) has to parse every
//! reachable commit before printing the first one: on the Linux kernel (1.5M commits) the first
//! page takes ~16 s, with the graph ~40 ms. `git commit-graph write --reachable` writes the
//! file `objects/info/commit-graph`, a cache that git rebuilds from the objects: writing it
//! never loses data and moves no ref.

use crate::EngineError;
use serde::{Deserialize, Serialize};

/// Whether git reads a commit-graph for the repo ([`GitMaintenance::commit_graph`](super::GitMaintenance::commit_graph)).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitGraphState {
    /// There is a commit-graph (`objects/info/commit-graph` or a split chain
    /// `objects/info/commit-graphs/commit-graph-chain`) in the repo's object directory or in
    /// one of its alternates.
    pub present: bool,
    /// `core.commitGraph` is not `false`: git reads the graph if there is one.
    pub enabled: bool,
    /// Shallow repo (`git rev-parse --is-shallow-repository`): git neither reads nor writes a
    /// commit-graph there (`git commit-graph write` succeeds and writes nothing).
    #[serde(default)]
    pub shallow: bool,
    /// Replace refs (`refs/replace/`, with `core.useReplaceRefs` on) or a grafts file: git
    /// neither reads nor writes a commit-graph, like in a shallow repo.
    #[serde(default)]
    pub replaced: bool,
}

impl CommitGraphState {
    /// Git uses a commit-graph for this repo: present, enabled, not shallow and not replaced.
    pub fn in_use(&self) -> bool {
        self.present && self.enabled && !self.shallow && !self.replaced
    }
}

/// [`EngineError::NotAllowed`] of `repo.optimize` with `core.commitGraph=false`: git would not
/// read the graph, nothing is written.
pub fn commit_graph_disabled() -> EngineError {
    EngineError::NotAllowed(msg!("engine-commit-graph-disabled"))
}

/// [`EngineError::NotAllowed`] of `repo.optimize` in a shallow repo: git would not write the graph
/// (nor read it), nothing is written.
pub fn commit_graph_shallow() -> EngineError {
    EngineError::NotAllowed(msg!("engine-commit-graph-shallow"))
}

/// [`EngineError::NotAllowed`] of `repo.optimize` with replace refs or grafts: git would not write
/// the graph (nor read it), nothing is written.
pub fn commit_graph_replaced() -> EngineError {
    EngineError::NotAllowed(msg!("engine-commit-graph-replaced"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_use_needs_present_and_enabled() {
        let state = |present, enabled, shallow| CommitGraphState {
            present,
            enabled,
            shallow,
            replaced: false,
        };
        assert!(state(true, true, false).in_use());
        assert!(!state(true, false, false).in_use());
        assert!(!state(false, true, false).in_use());
        assert!(!state(true, true, true).in_use());
        let replaced = CommitGraphState {
            replaced: true,
            ..state(true, true, false)
        };
        assert!(!replaced.in_use());
    }
}
