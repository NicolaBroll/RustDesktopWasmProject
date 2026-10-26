//! L2 — data model and domain logic.
//!
//! The [`git`] module holds the git client types and the parsers for git's output (see
//! `impl-docs/ARCHITECTURE-git.md` §3), also exported from the crate root. No
//! dependency on UI, platform or I/O: the parsers work on in-memory bytes.

pub mod git;

pub use git::{
    APPLY_ARGS, Blame, BlameCommit, BlameLine, ByteText, Change, ChangedFile, Choice, Commit,
    CommitDetails, Conflict, ConflictBlock, ConflictText, DIFF_ARGS, DIFF_TREE_ARGS, Diff,
    EntryKind, FILE_LOG_ARGS, FileDiff, FileLogEntry, GraphLayout, GraphRow, GraphSegment,
    HEADS_PREFIX, Head, Hunk, LOG_ARGS, LOG_FIELDS, LOG_FORMAT, Line, LineKind, LineRef,
    NO_NEWLINE_MARKER, Oid, Operation, PatchDirection, PlanError, PlanErrorReason, REFS_ARGS,
    REFS_FIELDS, REFS_FORMAT, REMOTES_PREFIX, RebaseAction, RebasePlan, RebaseStep, RebaseTodo,
    RebasedCommit, RefInfo, RefKind, RepoPath, STATUS_ARGS, Segment, Signature, Status,
    StatusEntry, SubmoduleStatus, TAGS_PREFIX, TrackedInfo, UnmergedInfo, Upstream, branch_short,
    build_patch, layout_graph, layout_graph_nodes, parse_blame, parse_conflicts, parse_diff,
    parse_diff_tree, parse_file_log, parse_log, parse_refs, parse_status, remote_short,
    resolve_all, resolve_block, tag_short,
};

/// Domain errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("invalid git identifier `{input}`: {reason}")]
    InvalidOid { input: String, reason: String },
    /// The output of a git command does not have the format the parser expects.
    #[error("invalid output of `git {command}`: {reason}")]
    InvalidGitOutput {
        /// The git subcommand, e.g. `log`.
        command: &'static str,
        reason: String,
    },
    /// The file does not lend itself to a per-line patch (binary, submodule, conflicted, ...).
    #[error("cannot stage or unstage single lines of `{path}`: {reason}")]
    UnsupportedPatch {
        /// The path, for display.
        path: String,
        reason: String,
    },
    /// The selected lines do not match changed lines of the diff.
    #[error("invalid line selection for `{path}`: {reason}")]
    InvalidLineSelection {
        /// The path, for display.
        path: String,
        reason: String,
    },
    /// A file's conflict markers do not form valid blocks (unclosed block,
    /// markers out of order), or a resolution does not apply to the block.
    #[error("invalid conflict: {reason}")]
    InvalidConflict { reason: String },
}
