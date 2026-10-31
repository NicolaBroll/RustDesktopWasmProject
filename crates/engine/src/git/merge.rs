//! Merge (`merge.start`, `merge.abort`, `merge.continue`, S29) and the "in progress" state
//! (`repo.operation`): types and checks shared by the backends (`impl-docs/git/merge.md`).
//!
//! The command resolves the rev, decides between fast-forward and merge commit (with
//! [`GitRead::unmerged_count`](super::GitRead::unmerged_count)), checks the state
//! ([`check_merge_state`]) and writes the default message ([`merge_source`],
//! [`MergeSource::message`]); the backend executes ([`GitOperations::merge`](super::GitOperations::merge)).
//! **A merge stopped on conflict stays in progress** (like pull in merge mode; pull in rebase mode
//! is instead aborted): it is
//! concluded with `merge.continue` (which uses [`GitIndex::commit`](super::GitIndex::commit),
//! with `MERGE_HEAD` git creates the merge commit) or aborted with `merge.abort`
//! ([`GitOperations::merge_abort`](super::GitOperations::merge_abort), with a backup of index and
//! worktree first).

use super::{HEADS_PREFIX, REMOTES_PREFIX, check_no_operation};
use crate::{EngineError, Message};
use project_domain::{EntryKind, Oid, Operation, RefInfo, RefKind, RepoPath, Status};
use project_domain::{TAGS_PREFIX, branch_short, remote_short, tag_short};
use serde::{Deserialize, Serialize};

/// How to integrate the commit into HEAD ([`GitOperations::merge`](super::GitOperations::merge)).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// The branch advances to the commit (`git merge --ff-only`): HEAD is an ancestor of it.
    FastForward,
    /// A merge commit (parents: HEAD and the commit), even when a
    /// fast-forward would be possible if `no_ff` (`--no-ff`).
    Merge { no_ff: bool },
    /// The changes of the commit as **one ordinary commit** on HEAD (one parent): `git merge
    /// --squash`, then a commit with the request's message. The merged branch is not recorded
    /// as merged. A stop (conflicts, a hook that rejects the commit) leaves an
    /// [`Operation::Squash`] in progress, with the [`SquashRecord`].
    Squash,
}

impl MergeMode {
    /// The name in the result (`strategy`): `fast_forward` or `merge`.
    pub fn name(&self) -> &'static str {
        match self {
            MergeMode::FastForward => "fast_forward",
            MergeMode::Merge { .. } => "merge",
            MergeMode::Squash => "squash",
        }
    }
}

/// What `merge.start` asks of the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRequest {
    /// The branch of HEAD (short name): the merge does not start with detached HEAD.
    pub branch: String,
    /// The value of the branch read earlier: the backend refuses if it changed.
    pub expected: Oid,
    /// The commit to merge (the already resolved rev: git receives only the id).
    pub target: Oid,
    /// For messages: the name of the rev (`feature`, `origin/x`, `v1.0`) or the id.
    pub target_name: String,
    pub mode: MergeMode,
    /// The message of the merge commit (with [`MergeMode::Merge`]) or of the squash commit
    /// (with [`MergeMode::Squash`]).
    pub message: String,
}

/// The file, in the git folder of the worktree, with the [`SquashRecord`] of a squash merge
/// in progress.
pub const SQUASH_RECORD: &str = "project-squash.json";

/// What the app knows of a squash merge it started and that stopped: git leaves only
/// `SQUASH_MSG` (and the conflicts in the index), not an operation in progress, so the engine
/// keeps this record in [`SQUASH_RECORD`]. It counts ([`Operation::Squash`]) only while HEAD is
/// still on `branch` at `head` and `SQUASH_MSG` exists; otherwise it is stale (a commit or a
/// reset from the terminal) and the backend ignores and removes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SquashRecord {
    /// The branch of HEAD (short name).
    pub branch: String,
    /// The value of the branch (HEAD) before the squash: the parent of the squash commit.
    pub head: Oid,
    /// The squashed commit (the resolved rev).
    pub target: Oid,
    /// The name of the rev for messages and the journal (`feature`, `origin/x`, an id).
    pub source: String,
    /// The backup ref of the branch before the squash (`refs/project/backup/merge-...`).
    pub backup: String,
    /// The message of the squash commit (`merge.continue` without `message` uses it).
    pub message: String,
    /// `CliBackend`: a hash (FNV-1a 64, hex) of the `SQUASH_MSG` git wrote for this squash: the
    /// record counts only while the file is that one (not one of a squash started later from the
    /// terminal). `None` on the `MemoryBackend`.
    #[serde(default)]
    pub squash_msg: Option<String>,
}

impl SquashRecord {
    /// The record as saved in [`SQUASH_RECORD`].
    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec_pretty(self).expect("the record is serializable")
    }

    /// The record read from [`SQUASH_RECORD`]; `None` if it is not valid.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok()
    }

    /// The record of the squash started by `request` with the backup `backup`.
    pub fn of(request: &MergeRequest, backup: &str) -> Self {
        SquashRecord {
            branch: request.branch.clone(),
            head: request.expected,
            target: request.target,
            source: request.target_name.clone(),
            backup: backup.to_owned(),
            message: request.message.clone(),
            squash_msg: None,
        }
    }
}

/// The most squashed-commit subjects listed in the default squash message.
pub const SQUASH_SUBJECTS: usize = 50;

/// The default message of a squash (`merge.start` with `squash` and no `message`): `Squashed
/// branch 'x'` (`remote-tracking branch`, `tag`, `commit` as for [`MergeSource::message`], with
/// ` into <branch>` outside `main`/`master`), a blank line and the subjects of the squashed
/// commits, oldest first, as `* <subject>`: at most [`SQUASH_SUBJECTS`], then `…`. `subjects`
/// is newest first (as the log returns them), possibly with one more than the limit.
pub fn squash_message(source: &MergeSource, branch: &str, subjects: &[String]) -> String {
    let mut message = source.message(branch).replacen("Merge ", "Squashed ", 1);
    if subjects.is_empty() {
        return message;
    }
    message.push('\n');
    // The oldest are the last ones: with more than the limit the newest are left out.
    let shown = subjects.len().min(SQUASH_SUBJECTS);
    for subject in subjects[subjects.len() - shown..].iter().rev() {
        message.push_str(&format!("\n* {subject}"));
    }
    if subjects.len() > SQUASH_SUBJECTS {
        message.push_str("\n…");
    }
    message
}

/// Outcome of [`GitOperations::merge`](super::GitOperations::merge).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeOutcome {
    /// The value of the branch afterwards: the merge commit, the fast-forward commit or, if the merge
    /// is stopped, the previous one.
    pub after: Oid,
    /// The backup ref of the branch before the merge (`refs/project/backup/merge-...`).
    pub backup: String,
    /// True if the merge stopped (conflicts, or a `pre-merge-commit` hook that rejected):
    /// the repo is in a merge (`MERGE_HEAD`), to be concluded with `merge.continue` or aborted with
    /// `merge.abort`.
    pub in_progress: bool,
    /// The conflicted files (empty if the merge is concluded): those still unmerged and those
    /// git reported as conflicted (even if `rerere` already resolved them).
    pub conflicts: Vec<RepoPath>,
    /// The git text (stdout and stderr) of a stopped merge; empty otherwise.
    pub message: String,
    /// Warnings found after a successful merge (e.g. a hook that moved HEAD).
    pub warnings: Vec<Message>,
}

/// Outcome of [`GitOperations::merge_abort`](super::GitOperations::merge_abort).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeAborted {
    /// The backup ref of index and worktree before the abort
    /// (`refs/project/backup/merge-abort-...`): a commit with the worktree files (conflicts
    /// and resolutions included) whose parents are HEAD and a commit with the index, like a
    /// stash (recovered with `git stash apply <ref>` or file by file with `git show
    /// <ref>:<path>`).
    pub backup: String,
    /// The files saved in the backup because they differ from HEAD (in the index or the worktree).
    pub files: Vec<RepoPath>,
    /// Warnings after a successful abort.
    pub warnings: Vec<Message>,
}

/// The operation in progress and its data, for the UI banner (`repo.operation`). Read from
/// the files git leaves in its folder.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationState {
    /// Like [`Status::operation`].
    pub operation: Option<Operation>,
    /// The incoming commits: `MERGE_HEAD` (one or more), `CHERRY_PICK_HEAD`, `REVERT_HEAD`.
    /// Empty for rebase, bisect, `git am` and without an operation.
    pub incoming: Vec<Oid>,
    /// The message prepared by git (`MERGE_MSG`: merge, cherry-pick, revert), as it is in the
    /// file (with the `# Conflicts:` comment lines); `None` if absent.
    pub message: Option<String>,
    /// Rebase: the commit being replayed onto (`rebase-merge/onto` or `rebase-apply/onto`).
    pub onto: Option<Oid>,
    /// Rebase: the branch being rewritten (`head-name`, full name), `None` if the
    /// rebase started from detached HEAD.
    pub rebase_branch: Option<String>,
    /// True if there is a cherry-pick or revert sequence of several commits started from the terminal
    /// (`.git/sequencer`): the S31 commands leave it to the terminal (`check_picking`).
    #[serde(default)]
    pub sequencer: bool,
    /// Rebase: what the app knows of a rebase it started ([`RebaseRecord`](super::RebaseRecord));
    /// `None` for one started from the terminal.
    #[serde(default)]
    pub rebase: Option<super::RebaseRecord>,
    /// Squash: the [`SquashRecord`] of the squash merge in progress (`None` otherwise).
    #[serde(default)]
    pub squash: Option<SquashRecord>,
}

/// Where the commit to merge comes from, for the default message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeSourceKind {
    Branch,
    RemoteBranch,
    Tag,
    Commit,
}

/// The commit to merge with its name ([`merge_source`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeSource {
    pub kind: MergeSourceKind,
    /// Short name (`feature`, `origin/x`, `v1.0`) or, for a commit, the full id.
    pub name: String,
    /// The full name of the ref, if it is a ref.
    pub full: Option<String>,
}

impl MergeSource {
    /// The default message of the merge commit into `branch`, as `git merge` writes it:
    /// `Merge branch 'x'`, `Merge remote-tracking branch 'origin/x'`, `Merge tag 'v1'`, `Merge
    /// commit '<id>'`, with ` into <branch>` if the branch is not `main` or `master` (like git with
    /// the default `merge.suppressDest`).
    pub fn message(&self, branch: &str) -> String {
        let what = match self.kind {
            MergeSourceKind::Branch => "branch",
            MergeSourceKind::RemoteBranch => "remote-tracking branch",
            MergeSourceKind::Tag => "tag",
            MergeSourceKind::Commit => "commit",
        };
        let mut message = format!("Merge {what} '{}'", self.name);
        if branch != "main" && branch != "master" {
            message.push_str(&format!(" into {branch}"));
        }
        message
    }
}

/// The name of the rev `rev` that resolved to `target`: the first ref, in the order git
/// resolves a name (full `<rev>`, `refs/<rev>`, `refs/tags/<rev>`, `refs/heads/<rev>`,
/// `refs/remotes/<rev>`), that exists and points to `target`; otherwise the commit itself (an id,
/// `HEAD~2`, a ref pointing elsewhere).
pub fn merge_source(rev: &str, refs: &[RefInfo], target: &Oid) -> MergeSource {
    let candidates = [
        rev.to_owned(),
        format!("refs/{rev}"),
        format!("{TAGS_PREFIX}{rev}"),
        format!("{HEADS_PREFIX}{rev}"),
        format!("{REMOTES_PREFIX}{rev}"),
    ];
    for name in &candidates {
        let Some(r) = refs.iter().find(|r| r.name == *name) else {
            continue;
        };
        if r.target != *target {
            break;
        }
        let (kind, short) = match r.kind {
            RefKind::LocalBranch => (MergeSourceKind::Branch, branch_short(name)),
            RefKind::RemoteBranch => (MergeSourceKind::RemoteBranch, remote_short(name)),
            RefKind::Tag => (MergeSourceKind::Tag, tag_short(name)),
            RefKind::Stash => break,
        };
        return MergeSource {
            kind,
            name: short.to_owned(),
            full: Some(name.clone()),
        };
    }
    MergeSource {
        kind: MergeSourceKind::Commit,
        name: target.to_string(),
        full: None,
    }
}

/// The conflicted files of the status (`u` entries).
pub fn conflicted_paths(status: &Status) -> Vec<RepoPath> {
    status
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        .map(|e| e.path.clone())
        .collect()
}

fn quoted(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The checks before `merge.start`, the same for the backends, on the state read now:
///
/// - an operation in progress (even another merge) → [`EngineError::NotAllowed`];
/// - conflicted files → [`EngineError::NotAllowed`];
/// - for a merge commit, **any** change to tracked files (staged or not) →
///   [`EngineError::NotAllowed`] (a choice, like pull): a merge that stops on conflict
///   would mix local changes with those of the merge, and `git merge --abort` cannot
///   always rebuild them. Never `--autostash`. Untracked files are fine (git does not
///   overwrite them). Fast-forward accepts changes that do not touch the changed files: git
///   itself refuses, before writing, to overwrite the others.
pub fn check_merge_state(status: &Status, mode: &MergeMode) -> Result<(), EngineError> {
    check_no_operation("merge.start", msg!("engine-busy-merge"), status.operation)?;
    let unmerged = conflicted_paths(status);
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-merge-conflicts",
            files = quoted(&unmerged)
        )));
    }
    if matches!(mode, MergeMode::FastForward) {
        return Ok(());
    }
    let changed: Vec<RepoPath> = status
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
            ) && (e.index.is_change() || e.worktree.is_change())
        })
        .map(|e| e.path.clone())
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    Err(EngineError::NotAllowed(msg!(
        "engine-merge-local-changes",
        files = quoted(&changed)
    )))
}

/// The check common to `merge.abort` and `merge.continue`: a merge (or a squash merge started
/// by the app, [`Operation::Squash`]) must be in progress.
pub fn check_merging(command: &str, status: &Status) -> Result<(), EngineError> {
    if status.operation == Some(Operation::Squash) {
        return Ok(());
    }
    check_in_progress(command, Operation::Merge, status)
}

/// The name of an operation for messages (`a merge`, `a cherry-pick`, ...).
pub(crate) fn operation_name(op: Operation) -> Message {
    match op {
        Operation::Rebase => msg!("engine-op-rebase"),
        Operation::CherryPick => msg!("engine-op-cherry-pick"),
        Operation::Revert => msg!("engine-op-revert"),
        Operation::Bisect => msg!("engine-op-bisect"),
        Operation::Am => msg!("engine-op-am"),
        Operation::Merge => msg!("engine-op-merge"),
        Operation::Squash => msg!("engine-op-squash"),
    }
}

/// The operation `expected` must be in progress (for the commands that conclude or
/// abort it), otherwise [`EngineError::NotAllowed`].
pub(crate) fn check_in_progress(
    command: &str,
    expected: Operation,
    status: &Status,
) -> Result<(), EngineError> {
    match status.operation {
        Some(op) if op == expected => Ok(()),
        None => Err(EngineError::NotAllowed(msg!(
            "engine-merge-not-in-progress",
            operation = operation_name(expected),
            command = command
        ))),
        Some(other) => Err(EngineError::NotAllowed(msg!(
            "engine-merge-other-in-progress",
            other = operation_name(other),
            operation = operation_name(expected),
            command = command
        ))),
    }
}

/// The `MERGE_MSG` message without the conflicts block git appends at the end
/// (`# Conflicts:` and the `#\t<file>` lines after it, with the comment character of
/// `core.commentChar`): `git commit -F` does not strip comments with the `whitespace` cleanup, and
/// they would end up in the message. The other lines stay as they were.
pub fn clean_merge_message(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| {
        let mut chars = l.chars();
        matches!(chars.next(), Some(c) if !c.is_alphanumeric() && !c.is_whitespace())
            && chars.as_str() == " Conflicts:"
    });
    let kept = match start {
        Some(i) => {
            let marker = lines[i].chars().next().expect("non-empty line");
            let rest_ok = lines[i + 1..]
                .iter()
                .all(|l| l.starts_with(marker) || l.trim().is_empty());
            if rest_ok { &lines[..i] } else { &lines[..] }
        }
        None => &lines[..],
    };
    kept.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(name: &str, kind: RefKind, target: Oid) -> RefInfo {
        RefInfo {
            name: name.to_owned(),
            kind,
            target,
            tag_object: None,
            upstream: None,
        }
    }

    #[test]
    fn sources_and_messages() {
        let a = Oid::from_bytes([1; Oid::LEN]);
        let b = Oid::from_bytes([2; Oid::LEN]);
        let refs = [
            r("refs/heads/feature", RefKind::LocalBranch, a),
            r("refs/remotes/origin/x", RefKind::RemoteBranch, a),
            r("refs/tags/v1", RefKind::Tag, a),
            r("refs/heads/v1", RefKind::LocalBranch, b),
        ];
        let s = merge_source("feature", &refs, &a);
        assert_eq!(s.kind, MergeSourceKind::Branch);
        assert_eq!(s.message("main"), "Merge branch 'feature'");
        assert_eq!(s.message("dev"), "Merge branch 'feature' into dev");
        let s = merge_source("refs/heads/feature", &refs, &a);
        assert_eq!(
            (s.kind, s.name.as_str()),
            (MergeSourceKind::Branch, "feature")
        );
        let s = merge_source("origin/x", &refs, &a);
        assert_eq!(
            s.message("master"),
            "Merge remote-tracking branch 'origin/x'"
        );
        // The tag comes before the branch with the same name, as in git.
        let s = merge_source("v1", &refs, &a);
        assert_eq!(s.message("main"), "Merge tag 'v1'");
        let s = merge_source(&b.to_string()[..8], &refs, &b);
        assert_eq!(s.kind, MergeSourceKind::Commit);
        assert_eq!(s.message("main"), format!("Merge commit '{b}'"));
    }

    #[test]
    fn merge_msg_cleanup() {
        assert_eq!(
            clean_merge_message("Merge branch 'x'\n\n# Conflicts:\n#\ta.txt\n#\tb c.txt\n"),
            "Merge branch 'x'"
        );
        assert_eq!(
            clean_merge_message("Title\n\n#123 stays\n\n; Conflicts:\n;\tf\n"),
            "Title\n\n#123 stays"
        );
        // Text after the block: it is not git's block, everything stays.
        assert_eq!(
            clean_merge_message("T\n# Conflicts:\naltro\n"),
            "T\n# Conflicts:\naltro"
        );
        assert_eq!(clean_merge_message("Only title\n"), "Only title");
    }

    #[test]
    fn squash_messages() {
        let a = Oid::from_bytes([1; Oid::LEN]);
        let refs = [r("refs/heads/feature", RefKind::LocalBranch, a)];
        let s = merge_source("feature", &refs, &a);
        assert_eq!(squash_message(&s, "main", &[]), "Squashed branch 'feature'");
        let subjects = vec!["second".to_owned(), "first".to_owned()];
        assert_eq!(
            squash_message(&s, "dev", &subjects),
            "Squashed branch 'feature' into dev\n\n* first\n* second"
        );
        let c = merge_source(&a.to_string(), &[], &a);
        assert!(squash_message(&c, "main", &[]).starts_with("Squashed commit '"));
        // More than the limit: the oldest 50, then `…`.
        let many: Vec<String> = (0..=SQUASH_SUBJECTS)
            .rev()
            .map(|i| format!("c{i}"))
            .collect();
        let text = squash_message(&s, "main", &many);
        assert!(text.contains("\n* c0\n* c1\n"), "{text}");
        assert!(!text.contains("* c50"), "{text}");
        assert!(text.ends_with("* c49\n…"), "{text}");
    }
}
