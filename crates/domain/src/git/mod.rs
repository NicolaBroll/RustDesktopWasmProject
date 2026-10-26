//! Git client types: identifiers, commits, refs, HEAD, graph lanes.
//!
//! Only data and pure functions: the parsers of git's output work on `&[u8]`
//! (the log, [`parse_log`], the status, [`parse_status`], the refs, [`parse_refs`], the files changed
//! by a commit, [`parse_diff_tree`], and the diff, [`parse_diff`]); [`build_patch`] builds the
//! patches for staging single lines; [`parse_conflicts`] reads the conflict markers of a
//! file and [`resolve_block`]/[`resolve_all`] resolve them; [`RebasePlan`] is the plan of an
//! interactive rebase; [`parse_blame`] reads `git blame --porcelain` and [`parse_file_log`] the
//! history of one file.

mod blame;
mod conflict;
mod diff;
mod diff_tree;
mod file_log;
mod graph;
mod log;
mod oid;
mod patch;
mod path;
mod quote;
mod rebase;
mod refs;
mod status;
mod text;

#[cfg(test)]
mod blame_tests;
#[cfg(test)]
mod conflict_tests;
#[cfg(test)]
mod diff_tests;
#[cfg(test)]
mod diff_tree_tests;
#[cfg(test)]
mod file_log_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod log_tests;
#[cfg(test)]
mod patch_tests;
#[cfg(test)]
mod rebase_tests;
#[cfg(test)]
mod refs_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod tests;

pub use blame::{Blame, BlameCommit, BlameLine, parse_blame};
pub use conflict::{
    Choice, ConflictBlock, ConflictText, Segment, parse_conflicts, resolve_all, resolve_block,
};
pub use diff::{DIFF_ARGS, Diff, FileDiff, Hunk, Line, LineKind, NO_NEWLINE_MARKER, parse_diff};
pub use diff_tree::{ChangedFile, CommitDetails, DIFF_TREE_ARGS, parse_diff_tree};
pub use file_log::{FILE_LOG_ARGS, FileLogEntry, parse_file_log};
pub use graph::{GraphLayout, GraphRow, GraphSegment, layout_graph, layout_graph_nodes};
pub use log::{LOG_ARGS, LOG_FIELDS, LOG_FORMAT, parse_log};
pub use oid::Oid;
pub use patch::{APPLY_ARGS, LineRef, PatchDirection, build_patch};
pub use path::RepoPath;
pub use rebase::{
    PlanError, PlanErrorReason, RebaseAction, RebasePlan, RebaseStep, RebaseTodo, RebasedCommit,
};
pub use refs::{
    HEADS_PREFIX, REFS_ARGS, REFS_FIELDS, REFS_FORMAT, REMOTES_PREFIX, TAGS_PREFIX, branch_short,
    parse_refs, remote_short, tag_short,
};
pub use status::{STATUS_ARGS, parse_status};
pub use text::ByteText;

use serde::{Deserialize, Serialize};

/// Author or committer of a commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// Name as written in the commit (converted with `from_utf8_lossy` if not UTF-8).
    pub name: String,
    pub email: String,
    /// Seconds since the Unix epoch (UTC).
    pub time: i64,
    /// Time zone of the signer, in minutes relative to UTC (e.g. `120` for +0200).
    pub offset_min: i32,
}

/// A commit read from the log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub id: Oid,
    /// Parents in order: empty for a root commit, two or more for a merge.
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    /// Subject: the first paragraph of the message with lines joined by a space (git's `%s`).
    pub summary: String,
    /// Rest of the message after the first paragraph (git's `%b`), without trailing newlines;
    /// may be empty.
    pub body: String,
}

impl Commit {
    /// True if the commit has more than one parent.
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// Kind of a ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefKind {
    /// `refs/heads/*`
    LocalBranch,
    /// `refs/remotes/*`
    RemoteBranch,
    /// `refs/tags/*`
    Tag,
    /// `refs/stash`
    Stash,
}

/// Remote branch tracked by a local branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    /// Name of the upstream in the form given by git: full in [`RefInfo`]
    /// (e.g. `refs/remotes/origin/main`), short in [`Status`] (e.g. `origin/main`, as in
    /// `# branch.upstream` of `git status`).
    pub name: String,
    /// Local commits not present in the upstream. `None` if git does not know (e.g. upstream gone).
    pub ahead: Option<u32>,
    /// Upstream commits not present locally. `None` if git does not know.
    pub behind: Option<u32>,
}

/// A ref (branch, tag, stash) and the commit it points to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefInfo {
    /// Full name, e.g. `refs/heads/main`.
    pub name: String,
    pub kind: RefKind,
    /// The commit the ref points to. For an annotated tag it is the object the tag points to (usually
    /// a commit; a tag on a tree or a blob points to that), not the tag.
    pub target: Oid,
    /// Only for annotated tags: the id of the tag object, i.e. the value of the ref (needed, for example,
    /// as the expected value in `git update-ref -d <ref> <old>`). `None` for lightweight tags and
    /// other refs.
    pub tag_object: Option<Oid>,
    /// Only for local branches that have one.
    pub upstream: Option<Upstream>,
}

impl RefInfo {
    /// Name without the kind prefix (`refs/heads/`, `refs/remotes/`, `refs/tags/`), for the UI.
    /// If the prefix is absent it returns the full name.
    pub fn short_name(&self) -> &str {
        let prefix = match self.kind {
            RefKind::LocalBranch => HEADS_PREFIX,
            RefKind::RemoteBranch => REMOTES_PREFIX,
            RefKind::Tag => TAGS_PREFIX,
            RefKind::Stash => "refs/",
        };
        self.name.strip_prefix(prefix).unwrap_or(&self.name)
    }
}

/// Where HEAD points.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Head {
    /// On a branch. `name` is the short name (e.g. `main`, as in `git status --branch`);
    /// `target` is `None` if the branch has no commits yet (empty repo).
    Branch { name: String, target: Option<Oid> },
    /// HEAD detached on a commit.
    Detached(Oid),
}

impl Head {
    /// The commit of HEAD, if it exists.
    pub fn target(&self) -> Option<Oid> {
        match self {
            Head::Branch { target, .. } => *target,
            Head::Detached(oid) => Some(*oid),
        }
    }
}

/// The repo state: HEAD, upstream of the current branch and modified files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub head: Head,
    /// Upstream of the current branch, with the short name (e.g. `origin/main`). `None` if the branch
    /// has none or HEAD is detached. `ahead`/`behind` are `None` if the upstream's ref
    /// no longer exists.
    pub upstream: Option<Upstream>,
    /// One entry per file, in git's order. Empty if the worktree is clean.
    pub entries: Vec<StatusEntry>,
    /// Operation in progress. [`parse_status`] always leaves it `None` (the output of
    /// `git status --porcelain=v2` does not report it): the backend fills it by looking in `.git`
    /// (`MERGE_HEAD`, `rebase-merge/`, `CHERRY_PICK_HEAD`, ...).
    pub operation: Option<Operation>,
}

impl Status {
    /// True if there is nothing to stage or commit: no entries except ignored
    /// files.
    pub fn is_clean(&self) -> bool {
        self.entries
            .iter()
            .all(|e| matches!(e.kind, EntryKind::Ignored))
    }

    /// True if at least one file is conflicted.
    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(StatusEntry::is_conflict)
    }
}

/// Operation in progress in the repo, to be finished or aborted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
    /// `git am` stopped halfway (`.git/rebase-apply/applying`). Added after the others.
    Am,
}

/// A file in the repo state.
///
/// `index` and `worktree` are git's `X` and `Y` letters: the staged change (HEAD → index)
/// and the unstaged one (index → worktree). For untracked and ignored files both are
/// [`Change::Untracked`] or [`Change::Ignored`], like `??` and `!!` of `git status -s`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusEntry {
    /// Current path (for a rename, the new one).
    pub path: RepoPath,
    pub index: Change,
    pub worktree: Change,
    pub kind: EntryKind,
}

impl StatusEntry {
    /// True if the file has staged changes (excluding conflicts).
    pub fn is_staged(&self) -> bool {
        !self.is_conflict() && self.index.is_change()
    }

    /// True if the file has worktree changes not yet staged (excluding conflicts, untracked
    /// and ignored files). Includes files added with `git add -N` (`.A`).
    pub fn is_unstaged(&self) -> bool {
        !self.is_conflict() && self.worktree.is_change()
    }

    /// True if the file is conflicted (record `u`).
    pub fn is_conflict(&self) -> bool {
        matches!(self.kind, EntryKind::Unmerged(_))
    }

    /// True if the file is untracked.
    pub fn is_untracked(&self) -> bool {
        matches!(self.kind, EntryKind::Untracked)
    }

    /// Original path of a rename or copy.
    pub fn original_path(&self) -> Option<&RepoPath> {
        match &self.kind {
            EntryKind::Renamed { original, .. } | EntryKind::Copied { original, .. } => {
                Some(original)
            }
            _ => None,
        }
    }

    /// Submodule state, if the path is a submodule.
    pub fn submodule(&self) -> Option<SubmoduleStatus> {
        match &self.kind {
            EntryKind::Ordinary(info)
            | EntryKind::Renamed { info, .. }
            | EntryKind::Copied { info, .. } => info.submodule,
            EntryKind::Unmerged(info) => info.submodule,
            EntryKind::Untracked | EntryKind::Ignored => None,
        }
    }
}

/// One `X` or `Y` letter of git's status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Change {
    /// `.`
    Unmodified,
    /// `M`
    Modified,
    /// `T`: the type changed (e.g. from file to symbolic link).
    TypeChanged,
    /// `A`
    Added,
    /// `D`
    Deleted,
    /// `R`
    Renamed,
    /// `C`
    Copied,
    /// `U`: conflicted.
    Unmerged,
    /// `?`
    Untracked,
    /// `!`
    Ignored,
}

impl Change {
    /// The letter used by git.
    pub fn letter(self) -> char {
        match self {
            Change::Unmodified => '.',
            Change::Modified => 'M',
            Change::TypeChanged => 'T',
            Change::Added => 'A',
            Change::Deleted => 'D',
            Change::Renamed => 'R',
            Change::Copied => 'C',
            Change::Unmerged => 'U',
            Change::Untracked => '?',
            Change::Ignored => '!',
        }
    }

    /// True for an actual modification: neither `.`, nor `U`, nor `?`, nor `!`.
    pub fn is_change(self) -> bool {
        !matches!(
            self,
            Change::Unmodified | Change::Unmerged | Change::Untracked | Change::Ignored
        )
    }
}

/// The entry kind, with the data specific to each record of `git status --porcelain=v2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    /// Record `1`: modified tracked file.
    Ordinary(TrackedInfo),
    /// Record `2` with `R`: renamed from `original`; `score` is the similarity (0–100).
    Renamed {
        info: TrackedInfo,
        original: RepoPath,
        score: u8,
    },
    /// Record `2` with `C`: copied from `original` (only with `status.renames=copies`).
    Copied {
        info: TrackedInfo,
        original: RepoPath,
        score: u8,
    },
    /// Record `u`: conflicted.
    Unmerged(UnmergedInfo),
    /// Record `?`.
    Untracked,
    /// Record `!` (only with `--ignored`).
    Ignored,
}

/// Modes and ids of a tracked file. Modes are octal numbers (e.g. `0o100644`, `0o120000` for a
/// link, `0o160000` for a submodule, `0` if the file does not exist); ids are zero if the object
/// does not exist (e.g. `oid_head` of an added file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedInfo {
    /// `Some` if the path is a submodule.
    pub submodule: Option<SubmoduleStatus>,
    pub mode_head: u32,
    pub mode_index: u32,
    pub mode_worktree: u32,
    pub oid_head: Oid,
    pub oid_index: Oid,
}

/// Data of a conflicted file: stages 1 (base), 2 (ours) and 3 (theirs) of the index.
/// A missing stage has mode `0` and zero id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmergedInfo {
    pub conflict: Conflict,
    pub submodule: Option<SubmoduleStatus>,
    pub mode_base: u32,
    pub mode_ours: u32,
    pub mode_theirs: u32,
    pub mode_worktree: u32,
    pub oid_base: Oid,
    pub oid_ours: Oid,
    pub oid_theirs: Oid,
}

/// Kind of conflict, from the XY code of a `u` record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Conflict {
    /// `DD`
    BothDeleted,
    /// `AU`
    AddedByUs,
    /// `UD`
    DeletedByThem,
    /// `UA`
    AddedByThem,
    /// `DU`
    DeletedByUs,
    /// `AA`
    BothAdded,
    /// `UU`
    BothModified,
}

/// State of a submodule (field `S<c><m><u>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubmoduleStatus {
    /// `C`: the submodule commit changed.
    pub commit_changed: bool,
    /// `M`: the submodule has changes to tracked files.
    pub modified: bool,
    /// `U`: the submodule has untracked files.
    pub untracked: bool,
}
