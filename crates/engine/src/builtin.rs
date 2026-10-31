//! Catalog of the built-in commands.

pub(crate) mod branch;
pub(crate) mod clone;
pub(crate) mod commit;
pub(crate) mod conflict;
pub(crate) mod discard;
pub(crate) mod edit;
pub(crate) mod file;
pub(crate) mod merge;
pub(crate) mod pick;
pub(crate) mod pull;
pub(crate) mod push;
pub(crate) mod rebase;
pub(crate) mod remote;
pub(crate) mod remote_branch;
pub(crate) mod remote_tag;
pub(crate) mod remotes;
pub(crate) mod reset;
pub(crate) mod search;
pub(crate) mod stage;
pub(crate) mod stash;
pub(crate) mod tag;

use crate::{
    Access::{Read, Write},
    DiffSpec, EngineError, Journal, LogQuery, Message, Registry, Session, commit_graph_disabled,
    commit_graph_replaced, commit_graph_shallow, file_digest,
};
use project_domain::{Commit, Diff, FileDiff, Head, Oid, RepoPath};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Commits returned by `repo.log` when `limit` is missing.
pub const DEFAULT_LOG_LIMIT: usize = 200;

impl Registry {
    /// Registry with all the built-in commands, in the order of the catalog.
    pub fn builtin() -> Self {
        let mut r = Self::default();
        r.add_repo();
        r.add_changes();
        r.add_refs();
        r.add_remotes();
        r.add_stashes();
        r.add_merges();
        r.add_rebase();
        r.add_edit();
        r
    }

    /// The repo, its history and the diffs.
    fn add_repo(&mut self) {
        self.add(
            "repo.open",
            "Open repository",
            r#"{"path": string}"#,
            Read,
            repo_open,
        )
        .takes_paths_outside_repo();
        self.add(
            "repo.clone",
            "Clone a repository into an absent or empty folder and open it",
            r#"{"url": string, "path": string, "branch"?: string, "depth"?: number (> 0)}"#,
            Write,
            clone::repo_clone,
        )
        .takes_paths_outside_repo();
        self.add(
            "repo.init",
            "New empty repository in an absent or empty folder, then open it",
            r#"{"path": string, "branch"?: string (without: init.defaultBranch)}"#,
            Write,
            clone::repo_init,
        )
        .takes_paths_outside_repo();
        self.add("repo.status", "Repository status", "{}", Read, repo_status);
        self.add(
            "repo.refs",
            "Branches, tags and stashes",
            "{}",
            Read,
            repo_refs,
        );
        self.add(
            "repo.operation",
            "Operation in progress (merge, rebase...) and conflicted files",
            "{}",
            Read,
            merge::repo_operation,
        );
        self.add(
            "repo.log",
            "Commit history",
            r#"{"skip"?: number, "limit"?: number (200), "rev"?: string, "all"?: bool, "stashes"?: bool (with all: also the stash bases)}"#,
            Read,
            repo_log,
        );
        self.add(
            "repo.search",
            "Commits whose message, author or id contains a text (literal, case-insensitive), in the order of repo.log",
            r#"{"query": string, "in"?: ["message" | "author" | "id"] (all three), "path"?: path (only commits touching it, no rename following), "all"?: bool, "stashes"?: bool (as in repo.log), "skip"?: number, "limit"?: number (200)} -> {"ids", "total", "has_more"}"#,
            Read,
            search::repo_search,
        );
        self.add(
            "repo.optimize",
            "Write the commit-graph (fast history on huge repos)",
            "{}",
            Write,
            repo_optimize,
        );
        self.add(
            "commit.show",
            "Commit details",
            r#"{"id": string}"#,
            Read,
            commit_show,
        );
        self.add(
            "file.blame",
            "Who last changed each line of a file",
            r#"{"path": path, "rev"?: string (without: the worktree, uncommitted lines have commit null)}"#,
            Read,
            file::file_blame,
        );
        self.add(
            "file.log",
            "Commits that change a file",
            r#"{"path": path, "rev"?: string, "skip"?: number, "limit"?: number (200), "follow"?: bool (true: across renames)}"#,
            Read,
            file::file_log,
        );
        self.add(
            "diff.worktree",
            "Diff of the unstaged changes",
            r#"{"path"?: path}"#,
            Read,
            diff_worktree,
        );
        self.add(
            "diff.index",
            "Diff of the staged changes",
            r#"{"path"?: path}"#,
            Read,
            diff_index,
        );
        self.add(
            "diff.commit",
            "Commit diff",
            r#"{"id": string, "path"?: path}"#,
            Read,
            diff_commit,
        );
        self.add(
            "diff.range",
            "Diff between two revisions",
            r#"{"from": string, "to": string, "merge_base"?: bool, "path"?: path}"#,
            Read,
            diff_range,
        );
    }

    /// Stage, commit and discard.
    fn add_changes(&mut self) {
        self.add(
            "index.stage",
            "Stage files",
            r#"{"paths": [path, ...]}"#,
            Write,
            stage::index_stage,
        );
        self.add(
            "index.unstage",
            "Unstage files",
            r#"{"paths": [path, ...]}"#,
            Write,
            stage::index_unstage,
        );
        self.add(
            "index.stage_lines",
            "Stage the chosen lines",
            r#"{"path": path, "lines": [{"old": n} | {"new": n}, ...], "digest": string (from diff.worktree)}"#,
            Write,
            stage::index_stage_lines,
        );
        self.add(
            "index.unstage_lines",
            "Unstage the chosen lines",
            r#"{"path": path, "lines": [{"old": n} | {"new": n}, ...], "digest": string (from diff.index)}"#,
            Write,
            stage::index_unstage_lines,
        );
        self.add(
            "commit.create",
            "Commit the staged changes",
            r#"{"message": string, "amend"?: bool, "confirm"?: bool (amend of a published commit)}"#,
            Write,
            commit::commit_create,
        );
        self.add(
            "worktree.discard",
            "Discard the unstaged changes",
            r#"{"paths": [path, ...], "confirm": true}"#,
            Write,
            discard::worktree_discard,
        );
    }

    /// Branches, reset and tags.
    fn add_refs(&mut self) {
        self.add(
            "branch.create",
            "Create a branch",
            r#"{"name": string, "start"?: string (rev, HEAD)}"#,
            Write,
            branch::branch_create,
        );
        self.add(
            "branch.checkout",
            "Switch to a branch",
            r#"{"name": string}"#,
            Write,
            branch::branch_checkout,
        );
        self.add(
            "branch.delete",
            "Delete a branch",
            r#"{"name": string, "confirm"?: bool (unmerged branch), "expected"?: full oid (the branch must point there)}"#,
            Write,
            branch::branch_delete,
        );
        self.add(
            "branch.rename",
            "Rename a branch",
            r#"{"name": string, "new_name": string}"#,
            Write,
            branch::branch_rename,
        );
        self.add(
            "branch.reset",
            "Reset the current branch to a commit",
            r#"{"target": string (rev), "mode": "soft"|"mixed"|"hard", "confirm"?: bool (hard with local changes), "expected"?: full oid (HEAD must be there)}"#,
            Write,
            reset::branch_reset,
        );
        self.add(
            "branch.reset_preview",
            "What a reset would do (files a hard overwrites)",
            r#"{"target": string (rev), "mode": "soft"|"mixed"|"hard"}"#,
            Read,
            reset::branch_reset_preview,
        );
        self.add(
            "tag.create",
            "Create a tag",
            r#"{"name": string, "target"?: string (rev, HEAD), "message"?: string (annotated tag; without: lightweight)}"#,
            Write,
            tag::tag_create,
        );
        self.add(
            "tag.delete",
            "Delete a tag",
            r#"{"name": string, "confirm": true (always required; the tag is saved in a backup ref first), "expected"?: string (full id of the value shown in the confirmation)}"#,
            Write,
            tag::tag_delete,
        );
    }

    /// Fetch, pull, push and the remotes.
    fn add_remotes(&mut self) {
        self.add(
            "remote.fetch",
            "Download from the remote (fetch)",
            r#"{"remote"?: string (configured remote; upstream, origin or the only one), "branch"?: string (only that branch), "prune"?: bool (also remove the remote branches gone from the remote; not with branch)}"#,
            Write,
            remote::remote_fetch,
        );
        self.add(
            "remote.pull",
            "Download and integrate the upstream (pull)",
            r#"{"remote"?: string (the upstream's), "branch"?: string (local branch; HEAD), "strategy"?: "merge" | "rebase" | "ff_only" (instead of the configuration)}"#,
            Write,
            pull::remote_pull,
        );
        self.add(
            "remote.push",
            "Publish the branch to the remote (push)",
            r#"{"remote"?: string (upstream, origin or the only one), "branch"?: string (local branch; HEAD), "force"?: bool (force-with-lease), "confirm"?: bool (force that removes commits from the remote)}"#,
            Write,
            push::remote_push,
        );
        self.add(
            "remote.delete_branch",
            "Delete a branch from the remote",
            r#"{"remote": string, "branch": string (name on the remote), "confirm": true, "expected": string (full id of the remote-tracking branch shown in the confirmation)}"#,
            Write,
            remote_branch::remote_delete_branch,
        );
        self.add(
            "remote.push_tag",
            "Publish a tag to the remote",
            r#"{"remote": string, "name": string (local tag; never overwrites a tag on the remote)}"#,
            Write,
            remote_tag::remote_push_tag,
        );
        self.add(
            "remote.delete_tag",
            "Delete a tag from the remote",
            r#"{"remote": string, "name": string (tag on the remote), "confirm": true, "expected": string (full id of the remote tag shown in the confirmation)}"#,
            Write,
            remote_tag::remote_delete_tag,
        );
        self.add(
            "remote.list",
            "Configured remotes",
            "{}",
            Read,
            remotes::remote_list,
        );
        self.add(
            "remote.add",
            "Add a remote",
            r#"{"name": string, "url": string, "fetch"?: bool (then download)}"#,
            Write,
            remotes::remote_add,
        );
        self.add(
            "remote.remove",
            "Remove a remote",
            r#"{"name": string, "confirm": true}"#,
            Write,
            remotes::remote_remove,
        );
    }

    /// Stashes.
    fn add_stashes(&mut self) {
        self.add("stash.list", "Stashes", "{}", Read, stash::stash_list);
        self.add(
            "stash.push",
            "Stash the changes",
            r#"{"message"?: string (one line), "include_untracked"?: bool, "keep_index"?: bool, "paths"?: [path, ...]}"#,
            Write,
            stash::stash_push,
        );
        self.add(
            "stash.apply",
            "Apply a stash",
            r#"{"index"?: number (stash@{n}, 0), "expected"?: full oid (the stash seen), "restore_index"?: bool}"#,
            Write,
            stash::stash_apply,
        );
        self.add(
            "stash.pop",
            "Apply a stash and remove it from the list",
            r#"{"index"?: number (stash@{n}, 0), "expected"?: full oid (the stash seen), "restore_index"?: bool}"#,
            Write,
            stash::stash_pop,
        );
        self.add(
            "stash.drop",
            "Drop a stash",
            r#"{"index"?: number (stash@{n}, 0), "expected": full oid (the stash to drop; required with confirm), "confirm": true}"#,
            Write,
            stash::stash_drop,
        );
    }

    /// Merge, cherry-pick and revert.
    fn add_merges(&mut self) {
        self.add(
            "merge.start",
            "Merge a branch into the current branch",
            r#"{"rev": string (branch, remote branch, tag or id), "no_ff"?: bool (always a merge commit), "ff_only"?: bool (fast-forward only), "message"?: string (of the merge commit)}"#,
            Write,
            merge::merge_start,
        );
        self.add(
            "merge.continue",
            "Finish the merge in progress",
            r#"{"message"?: string (otherwise the one prepared by git)}"#,
            Write,
            merge::merge_continue,
        );
        self.add(
            "merge.abort",
            "Abort the merge in progress",
            r#"{"confirm": true}"#,
            Write,
            merge::merge_abort,
        );
        self.add(
            "commit.cherry_pick",
            "Apply a commit onto the current branch (cherry-pick)",
            r#"{"rev": string (id, branch or tag of the commit), "mainline"?: number (for a merge commit: the parent, from 1)}"#,
            Write,
            pick::commit_cherry_pick,
        );
        self.add(
            "cherry_pick.continue",
            "Finish the cherry-pick in progress",
            r#"{"message"?: string (otherwise the one prepared by git)}"#,
            Write,
            pick::cherry_pick_continue,
        );
        self.add(
            "cherry_pick.abort",
            "Abort the cherry-pick in progress",
            r#"{"confirm": true}"#,
            Write,
            pick::cherry_pick_abort,
        );
        self.add(
            "commit.revert",
            "Undo a commit with a new commit (revert)",
            r#"{"rev": string (id, branch or tag of the commit), "mainline"?: number (for a merge commit: the parent, from 1)}"#,
            Write,
            pick::commit_revert,
        );
        self.add(
            "revert.continue",
            "Finish the revert in progress",
            r#"{"message"?: string (otherwise the one prepared by git)}"#,
            Write,
            pick::revert_continue,
        );
        self.add(
            "revert.abort",
            "Abort the revert in progress",
            r#"{"confirm": true}"#,
            Write,
            pick::revert_abort,
        );
    }

    /// Interactive rebase and conflict resolution.
    fn add_rebase(&mut self) {
        self.add(
            "rebase.plan",
            "Default plan of an interactive rebase of the current branch",
            r#"{"base": string (the commit to rebuild on, excluded: id, branch or tag)}"#,
            Read,
            rebase::rebase_plan,
        );
        self.add(
            "rebase.start",
            "Rewrite the commits of the current branch (interactive rebase)",
            r#"{"base": string, "steps": [{"action": "pick" | "reword" | "squash" | "fixup" | "drop", "commit": full oid, "message"?: string}] (oldest first, every commit of base..HEAD once), "confirm"?: bool (rewrites published commits)}"#,
            Write,
            rebase::rebase_start,
        );
        self.add(
            "rebase.continue",
            "Continue the rebase in progress",
            "{}",
            Write,
            rebase::rebase_continue,
        );
        self.add(
            "rebase.abort",
            "Abort the rebase in progress",
            r#"{"confirm": true}"#,
            Write,
            rebase::rebase_abort,
        );
        self.add(
            "conflict.show",
            "Versions of a conflicted file (base, ours, theirs, worktree)",
            r#"{"path": path}"#,
            Read,
            conflict::conflict_show,
        );
        self.add(
            "conflict.resolve",
            "Resolve a conflicted file and stage it",
            r#"{"path": path, "digest": string (from conflict.show), "content"?: string (the resolved text), "delete"?: bool (deletes the file; instead of content)}"#,
            Write,
            conflict::conflict_resolve,
        );
    }

    /// Undo, redo and the journal.
    fn add_edit(&mut self) {
        self.add(
            "edit.undo",
            "Undo the last operation",
            "{}",
            Write,
            edit::edit_undo,
        );
        self.add(
            "edit.redo",
            "Redo the undone operation",
            "{}",
            Write,
            edit::edit_redo,
        );
        self.add(
            "edit.history",
            "Operation journal",
            "{}",
            Read,
            edit::edit_history,
        );
    }
}

type Out = Result<Value, EngineError>;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct NoParams {}

/// Parameters of `repo.open`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RepoOpenParams {
    /// The folder of the repo (also a subfolder of the worktree).
    pub path: String,
}

fn to_json(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("the domain types are serializable")
}

/// Result of `repo.open` and `repo.init` (and the first fields of `repo.clone`): the repo just
/// opened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoOpenResult {
    /// The worktree root, absolute ([`GitRead::root`](crate::GitRead::root)), so the same
    /// repo opened from a subfolder or a relative path gives the same path; the given path if
    /// the backend has none.
    pub path: String,
    /// HEAD of the repo.
    pub head: Head,
    /// git reads a commit-graph for the repo
    /// ([`CommitGraphState::in_use`](crate::CommitGraphState::in_use)); without it `repo.log`
    /// is slow on huge histories and the UI can offer `repo.optimize`. A failed check gives
    /// `false` (the repo opens anyway).
    pub commit_graph: bool,
}

/// Result of `repo.optimize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoOptimizeResult {
    /// The state afterwards, as in [`RepoOpenResult::commit_graph`].
    pub commit_graph: bool,
}

/// Result of `repo.log`: a page of the history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoLogResult {
    /// The commits of the page, in log order.
    pub commits: Vec<Commit>,
    /// After the last returned commit there are others (the next page starts at `skip +
    /// limit`).
    pub has_more: bool,
}

/// Result of `diff.worktree` and `diff.index`: a [`Diff`] whose files also carry their
/// `digest`. Whoever reads it as a [`Diff`] ignores the digests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffWithDigests {
    /// One element per file, in git's order.
    pub files: Vec<FileDiffWithDigest>,
}

/// A file of [`DiffWithDigests`]: the fields of [`FileDiff`] and `digest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileDiffWithDigest {
    /// The changes to the file.
    #[serde(flatten)]
    pub file: FileDiff,
    /// [`file_digest`] of the file, to pass to `index.stage_lines`/`index.unstage_lines`.
    pub digest: String,
}

/// The repo at `path`, opened: [`RepoOpenResult`].
fn repo_open(s: &mut Session, RepoOpenParams { path }: RepoOpenParams) -> Out {
    s.open(&path)?;
    Ok(to_json(opened(s, path)?))
}

/// The result of `repo.open` for the repo just opened at `path` (also `repo.clone`, `repo.init`).
fn opened(s: &Session, path: String) -> Result<RepoOpenResult, EngineError> {
    let repo = s.repo()?;
    let path = repo.root().unwrap_or(path);
    let head = repo.head()?;
    let commit_graph = repo.commit_graph().is_ok_and(|g| g.in_use());
    Ok(RepoOpenResult {
        path,
        head,
        commit_graph,
    })
}

/// Writes the commit-graph of all reachable commits (`git commit-graph write --reachable`), so
/// that `repo.log` (`--date-order`) no longer parses the whole history before the first page.
///
/// A write (it adds or replaces `objects/info/commit-graph`), but it cannot lose data: the file
/// is a cache that git derives from the objects and that `git gc` itself rewrites. So no
/// `confirm`, no backup, and no [`JournalEntry`](crate::JournalEntry): no ref, index or worktree file changes.
/// Slow (~19 s on the Linux kernel): progress and cancellation from [`Session::job`]. Refused
/// with `core.commitGraph=false` ([`commit_graph_disabled`]), in a shallow repo
/// ([`commit_graph_shallow`]) and with replace refs or grafts ([`commit_graph_replaced`]): git
/// would not read it.
///
/// Result: [`RepoOptimizeResult`], the state afterwards (as in `repo.open`).
fn repo_optimize(s: &mut Session, NoParams {}: NoParams) -> Out {
    const ID: &str = "repo.optimize";
    let job = s.job().clone();
    let repo = s.repo_mut()?;
    let before = repo.commit_graph()?;
    if !before.enabled {
        return Err(commit_graph_disabled());
    }
    if before.shallow {
        return Err(commit_graph_shallow());
    }
    if before.replaced {
        return Err(commit_graph_replaced());
    }
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command: ID });
    }
    repo.write_commit_graph(&job)?;
    let after = repo.commit_graph()?;
    Ok(to_json(RepoOptimizeResult {
        commit_graph: after.in_use(),
    }))
}

/// The [`Status`](project_domain::Status) of the open repo.
fn repo_status(s: &mut Session, NoParams {}: NoParams) -> Out {
    Ok(to_json(s.repo()?.status()?))
}

/// The [`RefInfo`](project_domain::RefInfo) of the open repo, in name order.
fn repo_refs(s: &mut Session, NoParams {}: NoParams) -> Out {
    Ok(to_json(s.repo()?.refs()?))
}

/// Parameters of `commit.show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitShowParams {
    /// The commit: an id (also abbreviated) or a ref name.
    pub id: String,
}

/// The [`CommitDetails`](project_domain::CommitDetails) of commit `id` (also abbreviated, or a
/// ref name): `{"commit", "compared_to", "files"}`, with the files compared to the first
/// parent.
fn commit_show(s: &mut Session, CommitShowParams { id }: CommitShowParams) -> Out {
    Ok(to_json(s.repo()?.show_commit(&id)?))
}

/// Parameters of `diff.worktree` and `diff.index`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffParams {
    /// Only this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<RepoPath>,
}

/// Parameters of `diff.commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffCommitParams {
    /// The commit, as in `commit.show`.
    pub id: String,
    /// Only this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<RepoPath>,
}

/// Parameters of `diff.range`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DiffRangeParams {
    /// The old side: any revision that leads to a commit (id, branch, tag, `HEAD~2`...).
    pub from: String,
    /// The new side, as `from`.
    pub to: String,
    /// True: what `to` brings since it diverged from `from` (`git diff from...to`, against
    /// their merge base) instead of `to` against `from`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub merge_base: bool,
    /// Only this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<RepoPath>,
}

/// Result of `diff.range`: the [`Diff`] (`files`, as in `diff.commit`) and the commits compared.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffRangeResult {
    /// `from` resolved to a full commit id.
    pub from: Oid,
    /// `to` resolved to a full commit id.
    pub to: Oid,
    /// With `merge_base`: the merge base the diff is against (the old side). Absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<Oid>,
    /// The changed files.
    #[serde(flatten)]
    pub diff: Diff,
}

/// Like [`check_path`], for an optional path.
fn check_opt_path(
    id: &'static str,
    path: Option<RepoPath>,
) -> Result<Option<RepoPath>, EngineError> {
    path.map(|p| check_path(id, p)).transpose()
}

/// A file path as git writes it: relative to the root, with `/` as separator, without
/// empty components, `.` or `..` and without NUL. Otherwise [`EngineError::InvalidParams`].
fn check_path(id: &'static str, path: RepoPath) -> Result<RepoPath, EngineError> {
    let bytes = path.as_bytes();
    let reason = if bytes.is_empty() {
        msg!("engine-path-empty", path = path)
    } else if bytes.contains(&0) {
        msg!("engine-path-nul", path = path)
    } else if bytes
        .split(|&b| b == b'/')
        .any(|c| c.is_empty() || c == b"." || c == b"..")
    {
        msg!("engine-path-not-relative", path = path)
    } else {
        return Ok(path);
    };
    Err(EngineError::InvalidParams { id, reason })
}

/// The JSON of a [`Diff`] with, in addition, in every file, the `digest` field ([`file_digest`]) to
/// pass to `index.stage_lines`/`index.unstage_lines`: [`DiffWithDigests`].
fn diff_with_digests(diff: Diff) -> Value {
    let files = diff
        .files
        .into_iter()
        .map(|file| FileDiffWithDigest {
            digest: file_digest(&file),
            file,
        })
        .collect();
    to_json(DiffWithDigests { files })
}

/// The [`Diff`] of the worktree against the index (unstaged changes): `{"files": [...]}`,
/// each file with its `digest`. Untracked files do not appear.
fn diff_worktree(s: &mut Session, DiffParams { path }: DiffParams) -> Out {
    let path = check_opt_path("diff.worktree", path)?;
    Ok(diff_with_digests(
        s.repo()?.diff(&DiffSpec::Worktree { path })?,
    ))
}

/// The [`Diff`] of the index against HEAD (staged changes), each file with its
/// `digest`.
fn diff_index(s: &mut Session, DiffParams { path }: DiffParams) -> Out {
    let path = check_opt_path("diff.index", path)?;
    Ok(diff_with_digests(
        s.repo()?.diff(&DiffSpec::Index { path })?,
    ))
}

/// The [`Diff`] of commit `id` against the first parent (like
/// `commit.show`).
fn diff_commit(s: &mut Session, DiffCommitParams { id, path }: DiffCommitParams) -> Out {
    let path = check_opt_path("diff.commit", path)?;
    Ok(to_json(
        s.repo()?.diff(&DiffSpec::Commit { rev: id, path })?,
    ))
}

/// The [`Diff`] of `to` against `from`, or with `merge_base` against their merge base
/// (`from...to`): [`DiffRangeResult`]. A revision that does not lead to a commit, or two commits
/// without a common ancestor with `merge_base`, → [`EngineError::InvalidParams`].
fn diff_range(s: &mut Session, params: DiffRangeParams) -> Out {
    const ID: &str = "diff.range";
    let DiffRangeParams {
        from,
        to,
        merge_base,
        path,
    } = params;
    let path = check_opt_path(ID, path)?;
    let repo = s.repo()?;
    let commit = |rev: &str| {
        repo.commit_id(rev)?
            .ok_or_else(|| EngineError::InvalidParams {
                id: ID,
                reason: msg!("engine-range-not-commit", rev = rev),
            })
    };
    let (from_id, to_id) = (commit(&from)?, commit(&to)?);
    let base = if merge_base {
        let base =
            repo.common_base(&from_id, &to_id)?
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: msg!("engine-range-no-base", from = from, to = to),
                })?;
        Some(base)
    } else {
        None
    };
    let diff = repo.diff(&DiffSpec::Range {
        from: base.unwrap_or(from_id),
        to: to_id,
        path,
    })?;
    Ok(to_json(DiffRangeResult {
        from: from_id,
        to: to_id,
        base,
        diff,
    }))
}

/// The paths of a command that writes the index or the worktree (`index.stage`,
/// `index.unstage`, `worktree.discard`, `stash.push`), validated as in `diff.*` and without
/// duplicates (in the given order). An empty list is an error: for git "no path" would mean
/// "everything", and it is better that it cannot be asked for by mistake.
fn check_path_list(id: &'static str, paths: Vec<RepoPath>) -> Result<Vec<RepoPath>, EngineError> {
    if paths.is_empty() {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-paths-empty"),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let path = check_path(id, path)?;
        if seen.insert(path.clone()) {
            out.push(path);
        }
    }
    Ok(out)
}

/// The state of the operation journal after a command, flattened into the results of the
/// commands that write (`#[serde(flatten)]`): whether `edit.undo`/`edit.redo` have anything to do
/// and the description of the entry they would touch.
///
/// JSON: `{"can_undo", "can_redo", "next_undo": Message | null, "next_redo": Message | null}`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalState {
    /// `edit.undo` has an entry to undo.
    pub can_undo: bool,
    /// `edit.redo` has an entry to redo.
    pub can_redo: bool,
    /// The description of the entry `edit.undo` would undo (`null` if none).
    #[serde(default)]
    pub next_undo: Option<Message>,
    /// The description of the entry `edit.redo` would redo (`null` if none).
    #[serde(default)]
    pub next_redo: Option<Message>,
}

impl JournalState {
    /// The state of `journal`.
    pub(crate) fn of(journal: &Journal) -> Self {
        Self {
            can_undo: journal.can_undo(),
            can_redo: journal.can_redo(),
            next_undo: journal.done().last().map(|e| e.description.clone()),
            next_redo: journal.undone().last().map(|e| e.description.clone()),
        }
    }
}

/// What a command re-reads after a successful write, as the pair of fields `<key>` and
/// `<key>_error` of a typed result: the value, or `None` and the error text. The operation is
/// done anyway and must not look failed (or be repeated).
fn reread<T>(result: Result<T, EngineError>) -> (Option<T>, Option<String>) {
    match result {
        Ok(value) => (Some(value), None),
        Err(e) => (None, Some(e.to_string())),
    }
}

/// A page of the log: [`RepoLogResult`].
fn repo_log(s: &mut Session, query: LogQuery) -> Out {
    let limit = query.limit.unwrap_or(DEFAULT_LOG_LIMIT);
    // One more commit to know whether the page is the last.
    let probe = LogQuery {
        limit: Some(limit.saturating_add(1)),
        ..query
    };
    let mut commits = s.repo()?.log(&probe)?;
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    Ok(to_json(RepoLogResult { commits, has_more }))
}

#[cfg(test)]
#[path = "builtin/snapshot_tests.rs"]
mod snapshot_tests;

#[cfg(test)]
#[path = "builtin/range_tests.rs"]
mod range_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitRead, MemoryBackend, MemoryOpener};
    use project_domain::{CommitDetails, RefInfo, Status};
    use serde_json::json;
    use std::sync::Arc;

    fn session() -> Session {
        Session::new(Arc::new(MemoryOpener::sample()))
    }

    fn opened() -> (Registry, Session) {
        let r = Registry::builtin();
        let mut s = session();
        r.run(
            "repo.open",
            &mut s,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
        (r, s)
    }

    fn log_page(r: &Registry, s: &mut Session, params: Value) -> (Vec<Commit>, bool) {
        let out = r.run("repo.log", s, params).unwrap();
        let commits = serde_json::from_value(out["commits"].clone()).unwrap();
        (commits, out["has_more"].as_bool().unwrap())
    }

    #[test]
    fn catalog() {
        let r = Registry::builtin();
        let ids: Vec<_> = r.list().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "branch.checkout",
                "branch.create",
                "branch.delete",
                "branch.rename",
                "branch.reset",
                "branch.reset_preview",
                "cherry_pick.abort",
                "cherry_pick.continue",
                "commit.cherry_pick",
                "commit.create",
                "commit.revert",
                "commit.show",
                "conflict.resolve",
                "conflict.show",
                "diff.commit",
                "diff.index",
                "diff.range",
                "diff.worktree",
                "edit.history",
                "edit.redo",
                "edit.undo",
                "file.blame",
                "file.log",
                "index.stage",
                "index.stage_lines",
                "index.unstage",
                "index.unstage_lines",
                "merge.abort",
                "merge.continue",
                "merge.start",
                "rebase.abort",
                "rebase.continue",
                "rebase.plan",
                "rebase.start",
                "remote.add",
                "remote.delete_branch",
                "remote.delete_tag",
                "remote.fetch",
                "remote.list",
                "remote.pull",
                "remote.push",
                "remote.push_tag",
                "remote.remove",
                "repo.clone",
                "repo.init",
                "repo.log",
                "repo.open",
                "repo.operation",
                "repo.optimize",
                "repo.refs",
                "repo.search",
                "repo.status",
                "revert.abort",
                "revert.continue",
                "stash.apply",
                "stash.drop",
                "stash.list",
                "stash.pop",
                "stash.push",
                "tag.create",
                "tag.delete",
                "worktree.discard"
            ]
        );
        let ids: Vec<_> = r.search("COMMIT HISTORY").map(|c| c.id).collect();
        assert_eq!(ids, ["repo.log"]);
        let writes: Vec<_> = r.list().filter(|c| c.writes).map(|c| c.id).collect();
        assert_eq!(
            writes,
            [
                "branch.checkout",
                "branch.create",
                "branch.delete",
                "branch.rename",
                "branch.reset",
                "cherry_pick.abort",
                "cherry_pick.continue",
                "commit.cherry_pick",
                "commit.create",
                "commit.revert",
                "conflict.resolve",
                "edit.redo",
                "edit.undo",
                "index.stage",
                "index.stage_lines",
                "index.unstage",
                "index.unstage_lines",
                "merge.abort",
                "merge.continue",
                "merge.start",
                "rebase.abort",
                "rebase.continue",
                "rebase.start",
                "remote.add",
                "remote.delete_branch",
                "remote.delete_tag",
                "remote.fetch",
                "remote.pull",
                "remote.push",
                "remote.push_tag",
                "remote.remove",
                "repo.clone",
                "repo.init",
                "repo.optimize",
                "revert.abort",
                "revert.continue",
                "stash.apply",
                "stash.drop",
                "stash.pop",
                "stash.push",
                "tag.create",
                "tag.delete",
                "worktree.discard"
            ]
        );
        let outside: Vec<_> = r.list().filter(|c| c.outside_repo).map(|c| c.id).collect();
        assert_eq!(outside, ["repo.clone", "repo.init", "repo.open"]);
    }

    #[test]
    fn every_command_has_a_self_contained_parameter_schema() {
        let r = Registry::builtin();
        for spec in r.list() {
            let schema = &spec.params;
            assert_eq!(schema["type"], "object", "{}", spec.id);
            assert_eq!(schema["additionalProperties"], false, "{}", spec.id);
            let text = schema.to_string();
            assert!(
                !text.contains("$ref") && !text.contains("$schema"),
                "{}: {text}",
                spec.id
            );
        }
        let open = &r.get("repo.open").unwrap().params;
        assert_eq!(open["required"], serde_json::json!(["path"]));
        assert_eq!(open["properties"]["path"]["type"], "string");
    }

    #[test]
    fn open_returns_path_and_head() {
        let r = Registry::builtin();
        let mut s = session();
        let out = r
            .run(
                "repo.open",
                &mut s,
                json!({ "path": MemoryOpener::SAMPLE_PATH }),
            )
            .unwrap();
        assert_eq!(out["path"], MemoryOpener::SAMPLE_PATH);
        let head = MemoryBackend::sample().head().unwrap();
        assert_eq!(out["head"], to_json(head));
        assert_eq!(s.path(), Some(MemoryOpener::SAMPLE_PATH));
        assert_eq!(out["commit_graph"], false);
    }

    /// Reads `out` as `T` and checks that `T` gives the same JSON again.
    fn same<T: serde::de::DeserializeOwned + Serialize>(out: Value) -> T {
        let typed: T = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(to_json(&typed), out);
        typed
    }

    /// The typed results of `repo.open`, `repo.optimize`, `repo.log` and `diff.worktree`/
    /// `diff.index` read back the JSON of the command and give the same JSON again: the UI
    /// deserializes into them.
    #[test]
    fn read_results_round_trip() {
        let r = Registry::builtin();
        let mut s = session();
        let open: RepoOpenResult = same(
            r.run(
                "repo.open",
                &mut s,
                json!({ "path": MemoryOpener::SAMPLE_PATH }),
            )
            .unwrap(),
        );
        assert_eq!(open.path, MemoryOpener::SAMPLE_PATH);
        assert_eq!(open.head, MemoryBackend::sample().head().unwrap());
        assert!(!open.commit_graph);

        let log: RepoLogResult = same(r.run("repo.log", &mut s, json!({ "limit": 2 })).unwrap());
        assert_eq!(log.commits.len(), 2);
        assert!(log.has_more);
        let last: RepoLogResult = same(r.run("repo.log", &mut s, json!({ "all": true })).unwrap());
        assert!(!last.has_more);

        for id in ["diff.worktree", "diff.index"] {
            let diff: DiffWithDigests = same(r.run(id, &mut s, json!({})).unwrap());
            assert!(!diff.files.is_empty(), "{id}");
            for f in &diff.files {
                assert_eq!(f.digest, file_digest(&f.file), "{id}");
            }
            // Read as a plain `Diff`, the digests are ignored.
            let plain: Diff =
                serde_json::from_value(r.run(id, &mut s, json!({})).unwrap()).unwrap();
            let files: Vec<FileDiff> = diff.files.into_iter().map(|f| f.file).collect();
            assert_eq!(plain.files, files, "{id}");
        }

        let optimized: RepoOptimizeResult =
            same(r.run("repo.optimize", &mut s, json!({})).unwrap());
        assert!(optimized.commit_graph);
    }

    /// Session on the sample repo with `core.commitGraph` set to `value` (unset if `None`).
    fn opened_with_commit_graph_config(value: Option<&str>) -> (Registry, Session) {
        let mut repo = MemoryBackend::sample();
        repo.set_config("core.commitGraph", value.as_slice());
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(opener));
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        (r, s)
    }

    #[test]
    fn optimize_writes_the_commit_graph_and_changes_nothing_else() {
        let (r, mut s) = opened();
        let log_before = r.run("repo.log", &mut s, json!({ "all": true })).unwrap();
        let refs_before = r.run("repo.refs", &mut s, json!({})).unwrap();
        assert!(r.list().any(|c| c.id == "repo.optimize" && c.writes));

        let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
        assert_eq!(out, json!({ "commit_graph": true }));
        assert!(s.repo().unwrap().commit_graph().unwrap().in_use());
        // A cache: same log, same refs, nothing in the journal (nothing to undo).
        assert_eq!(
            r.run("repo.log", &mut s, json!({ "all": true })).unwrap(),
            log_before
        );
        assert_eq!(r.run("repo.refs", &mut s, json!({})).unwrap(), refs_before);
        assert!(s.journal().unwrap().done().is_empty());
        // Running it again just rewrites it.
        let again = r.run("repo.optimize", &mut s, json!({})).unwrap();
        assert_eq!(again["commit_graph"], true);
    }

    #[test]
    fn optimize_is_refused_when_core_commit_graph_is_false() {
        for value in ["false", "no", "off", "0"] {
            let (r, mut s) = opened_with_commit_graph_config(Some(value));
            let err = r.run("repo.optimize", &mut s, json!({})).unwrap_err();
            assert!(matches!(err, EngineError::NotAllowed(_)), "{value}: {err}");
            assert!(err.to_string().contains("core.commitGraph"), "{err}");
            assert!(!s.repo().unwrap().commit_graph().unwrap().present);
        }
        for value in [Some("true"), None] {
            let (r, mut s) = opened_with_commit_graph_config(value);
            let out = r.run("repo.optimize", &mut s, json!({})).unwrap();
            assert_eq!(out["commit_graph"], true, "{value:?}");
        }
    }

    #[test]
    fn optimize_cancelled_writes_nothing() {
        let (r, mut s) = opened();
        let token = crate::CancelToken::new();
        token.cancel();
        let job = crate::JobContext::new().with_cancel(token);
        let err = r
            .run_with("repo.optimize", &mut s, json!({}), job)
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::Cancelled {
                    command: "repo.optimize"
                }
            ),
            "{err}"
        );
        assert!(!s.repo().unwrap().commit_graph().unwrap().present);
    }

    #[test]
    fn optimize_checks_params_and_needs_a_repo() {
        let (r, mut s) = opened();
        let err = r
            .run("repo.optimize", &mut s, json!({ "split": true }))
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::InvalidParams {
                    id: "repo.optimize",
                    ..
                }
            ),
            "{err}"
        );
        let err = r
            .run("repo.optimize", &mut session(), json!({}))
            .unwrap_err();
        assert!(matches!(err, EngineError::NoRepo), "{err}");
    }

    #[test]
    fn reads_need_an_open_repo() {
        let r = Registry::builtin();
        let mut s = session();
        for id in [
            "repo.status",
            "repo.refs",
            "repo.log",
            "commit.show",
            "diff.worktree",
            "diff.index",
            "diff.commit",
            "index.stage",
            "index.unstage",
            "index.stage_lines",
            "index.unstage_lines",
            "commit.create",
            "worktree.discard",
        ] {
            let params = if matches!(id, "commit.show" | "diff.commit") {
                json!({ "id": "HEAD" })
            } else if id.ends_with("_lines") {
                json!({ "path": "README.md", "lines": [{ "new": 1 }], "digest": "x" })
            } else if id.starts_with("index.") || id == "worktree.discard" {
                json!({ "paths": ["README.md"] })
            } else if id == "commit.create" {
                json!({ "message": "x" })
            } else {
                json!({})
            };
            let err = r.run(id, &mut s, params).unwrap_err();
            assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
        }
    }

    #[test]
    fn status_and_refs_match_the_backend() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();

        let status: Status =
            serde_json::from_value(r.run("repo.status", &mut s, Value::Null).unwrap()).unwrap();
        assert_eq!(status, sample.status().unwrap());

        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", &mut s, json!({})).unwrap()).unwrap();
        assert_eq!(refs, sample.refs().unwrap());
    }

    #[test]
    fn log_pages_cover_the_whole_history() {
        let (r, mut s) = opened();
        let all = MemoryBackend::sample().log(&LogQuery::all()).unwrap();
        assert!(all.len() > 3, "the sample repo has about a dozen commits");

        let (default, more) = log_page(&r, &mut s, json!({}));
        assert_eq!((default.as_slice(), more), (all.as_slice(), false));

        let mut pages = Vec::new();
        let mut skip = 0;
        loop {
            let (page, more) = log_page(&r, &mut s, json!({ "skip": skip, "limit": 3 }));
            assert!(page.len() <= 3);
            skip += page.len();
            pages.extend(page);
            if !more {
                break;
            }
        }
        assert_eq!(pages, all);

        let (exact, more) = log_page(&r, &mut s, json!({ "limit": all.len() }));
        assert_eq!((exact.len(), more), (all.len(), false));
        let (empty, more) = log_page(&r, &mut s, json!({ "limit": 0 }));
        assert_eq!((empty.len(), more), (0, true));
    }

    #[test]
    fn log_from_rev() {
        let (r, mut s) = opened();
        let first = MemoryBackend::sample().log(&LogQuery::all()).unwrap()[1].id;
        let (page, _) = log_page(&r, &mut s, json!({ "rev": first.to_string(), "limit": 1 }));
        assert_eq!(page[0].id, first);

        let err = r
            .run("repo.log", &mut s, json!({ "rev": "missing" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    }

    #[test]
    fn log_of_all_refs() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();
        let (all, more) = log_page(&r, &mut s, json!({ "all": true }));
        assert_eq!(
            (all, more),
            (sample.log(&LogQuery::all().from_all_refs()).unwrap(), false)
        );
        let (page, more) = log_page(&r, &mut s, json!({ "all": true, "skip": 2, "limit": 3 }));
        assert_eq!(
            page,
            sample.log(&LogQuery::page(2, 3).from_all_refs()).unwrap()
        );
        assert!(more);

        let err = r
            .run("repo.log", &mut s, json!({ "all": true, "rev": "main" }))
            .unwrap_err();
        assert!(
            matches!(err, EngineError::InvalidParams { id: "repo.log", .. }),
            "{err}"
        );
    }

    #[test]
    fn commit_show_returns_metadata_and_files() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();
        let head = sample.head().unwrap().target().unwrap();

        let out = r
            .run("commit.show", &mut s, json!({ "id": "HEAD" }))
            .unwrap();
        let details: CommitDetails = serde_json::from_value(out.clone()).unwrap();
        assert_eq!(details, sample.show_commit("HEAD").unwrap());
        assert_eq!(details.commit.id, head);
        assert_eq!(details.compared_to, Some(details.commit.parents[0]));
        assert!(!details.files.is_empty());

        // Shape of the JSON the UI reads.
        assert_eq!(out["commit"]["id"], head.to_string());
        assert_eq!(out["compared_to"], details.commit.parents[0].to_string());
        let renamed = out["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["change"] == "Renamed")
            .expect("commit 12 of the sample has a rename");
        assert_eq!(renamed["original"], "src/main.rs");
        assert_eq!(renamed["path"], "src/bin/app.rs");
        assert_eq!(renamed["score"], 92);

        // Abbreviated id and ref name lead to the same commit.
        let short = head.to_string()[..8].to_owned();
        for id in [short.as_str(), "main", "refs/heads/main"] {
            let out = r.run("commit.show", &mut s, json!({ "id": id })).unwrap();
            assert_eq!(out["commit"]["id"], head.to_string(), "{id}");
        }

        let err = r
            .run("commit.show", &mut s, json!({ "id": "missing" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    }

    #[test]
    fn diff_commands_return_the_backend_diff() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();
        let path = |p: &str| Some(RepoPath::from(p));
        for (id, params, spec) in [
            (
                "diff.worktree",
                json!({}),
                DiffSpec::Worktree { path: None },
            ),
            ("diff.index", Value::Null, DiffSpec::Index { path: None }),
            (
                "diff.index",
                json!({ "path": "README.md" }),
                DiffSpec::Index {
                    path: path("README.md"),
                },
            ),
            (
                "diff.worktree",
                json!({ "path": null }),
                DiffSpec::Worktree { path: None },
            ),
            (
                "diff.commit",
                json!({ "id": "main" }),
                DiffSpec::Commit {
                    rev: "HEAD".into(),
                    path: None,
                },
            ),
            (
                "diff.commit",
                json!({ "id": "HEAD", "path": "src/main.rs" }),
                DiffSpec::Commit {
                    rev: "HEAD".into(),
                    path: path("src/main.rs"),
                },
            ),
        ] {
            let out = r.run(id, &mut s, params.clone()).unwrap();
            let diff: Diff = serde_json::from_value(out).unwrap();
            assert_eq!(diff, sample.diff(&spec).unwrap(), "{id} {params}");
            assert!(!diff.files.is_empty(), "{id} {params}");
        }

        // Shape of the JSON the UI reads.
        let out = r
            .run(
                "diff.commit",
                &mut s,
                json!({ "id": "HEAD", "path": "src/bin/app.rs" }),
            )
            .unwrap();
        let files = out["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        let f = &files[0];
        assert_eq!(f["change"], "Renamed");
        assert_eq!(f["old_path"], "src/main.rs");
        assert_eq!(f["new_path"], "src/bin/app.rs");
        assert_eq!(f["similarity"], 92);
        assert_eq!(f["old_mode"], 0o100644);
        assert_eq!(f["binary"], false);
        assert!(f["old_oid"].is_string());
        let hunk = &f["hunks"][0];
        assert_eq!(hunk["old_start"], 1);
        assert_eq!(hunk["section"], "");
        let line = &hunk["lines"][1];
        assert_eq!(line["kind"], "Removed");
        assert_eq!(line["old_line"], 2);
        assert_eq!(line["new_line"], Value::Null);
        assert_eq!(line["content"], "// last change: commit 3");
        assert_eq!(line["no_newline"], false);

        // A non-UTF-8 path is passed as a byte array; here it is unchanged.
        let out = r
            .run("diff.index", &mut s, json!({ "path": [99, 97, 255] }))
            .unwrap();
        assert_eq!(out, json!({ "files": [] }));

        let err = r
            .run("diff.commit", &mut s, json!({ "id": "missing" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
    }

    #[test]
    fn stage_and_unstage_return_the_new_status() {
        let (r, mut s) = opened();
        let original = MemoryBackend::sample();

        let out = r
            .run(
                "index.stage",
                &mut s,
                json!({ "paths": ["note.txt", "src/lib.rs", "note.txt"] }),
            )
            .unwrap();
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        // Nothing at risk: `note.txt` and `src/lib.rs` had no staged-only versions.
        assert_eq!(out["backup"], Value::Null);
        let status: Status = serde_json::from_value(out["status"].clone()).unwrap();
        let staged: Vec<_> = status
            .entries
            .iter()
            .map(|e| (e.path.to_string(), e.index.letter(), e.worktree.letter()))
            .collect();
        assert_eq!(
            staged,
            [
                ("README.md".to_owned(), 'M', 'M'),
                ("docs/guide.md".to_owned(), 'A', '.'),
                ("note.txt".to_owned(), 'A', '.'),
                ("src/lib.rs".to_owned(), 'M', '.'),
            ]
        );
        // Shape of the JSON the UI reads: `status` as in `repo.status`.
        assert_eq!(out["status"]["entries"][2]["path"], "note.txt");
        assert_eq!(out["status"]["entries"][2]["index"], "Added");
        assert_eq!(out["status"]["head"]["Branch"]["name"], "main");
        // The diffs see the new index.
        let diff = r
            .run("diff.index", &mut s, json!({ "path": "note.txt" }))
            .unwrap();
        assert_eq!(diff["files"][0]["change"], "Added");

        let out = r
            .run(
                "index.unstage",
                &mut s,
                json!({ "paths": ["note.txt", "src/lib.rs"] }),
            )
            .unwrap();
        let status: Status = serde_json::from_value(out["status"].clone()).unwrap();
        assert_eq!(status, original.status().unwrap());

        // README.md is `MM`: the staged version exists only in the index, a backup first.
        let out = r
            .run("index.unstage", &mut s, json!({ "paths": ["README.md"] }))
            .unwrap();
        assert_eq!(out["backup"], "refs/project/backup/index-1");
        assert_eq!(out["status"]["entries"][0]["index"], "Unmodified");

        // A non-UTF-8 path is passed as a byte array; here it does not exist.
        let err = r
            .run("index.stage", &mut s, json!({ "paths": [[99, 97, 255]] }))
            .unwrap_err();
        assert!(matches!(err, EngineError::Git { .. }), "{err}");
    }

    #[test]
    fn stage_and_unstage_lines() {
        let (r, mut s) = opened();
        let original = r.run("repo.status", &mut s, json!({})).unwrap();

        // `diff.worktree` and `diff.index` give the digest of each file.
        let diff = r
            .run("diff.worktree", &mut s, json!({ "path": "src/lib.rs" }))
            .unwrap();
        let file = &diff["files"][0];
        let parsed: FileDiff = serde_json::from_value(file.clone()).unwrap();
        assert_eq!(file["digest"], file_digest(&parsed));
        let all = r.run("diff.index", &mut s, json!({})).unwrap();
        assert!(
            all["files"]
                .as_array()
                .unwrap()
                .iter()
                .all(|f| f["digest"].is_string())
        );
        let first = parsed.line_refs()[0];

        let out = r
            .run(
                "index.stage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": [first], "digest": file["digest"] }),
            )
            .unwrap();
        assert_eq!(out["backup"], Value::Null);
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        let lib = out["status"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["path"] == "src/lib.rs")
            .unwrap();
        assert_eq!(
            (&lib["index"], &lib["worktree"]),
            (&json!("Modified"), &json!("Modified"))
        );

        // The same digest is now stale: nothing changes.
        let err = r
            .run(
                "index.stage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": [first], "digest": file["digest"] }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::StaleDiff { .. }), "{err}");

        // Lines that do not exist in the diff.
        let diff = r
            .run("diff.index", &mut s, json!({ "path": "src/lib.rs" }))
            .unwrap();
        let digest = diff["files"][0]["digest"].clone();
        let err = r
            .run(
                "index.unstage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": [{ "old": 999 }], "digest": digest }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::InvalidParams { .. }), "{err}");

        // Unstage of all the staged lines: back to the initial state, with a backup
        // because the file was `MM`.
        let parsed: FileDiff = serde_json::from_value(diff["files"][0].clone()).unwrap();
        let out = r
            .run(
                "index.unstage_lines",
                &mut s,
                json!({ "path": "src/lib.rs", "lines": parsed.line_refs(), "digest": digest }),
            )
            .unwrap();
        assert_eq!(out["backup"], "refs/project/backup/index-1");
        assert_eq!(out["status"], original);
    }

    #[test]
    fn commit_create_returns_commit_previous_branch_and_status() {
        let (r, mut s) = opened();
        let head = MemoryBackend::sample().head().unwrap().target().unwrap();
        let out = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "Update README\n\nWith the guide." }),
            )
            .unwrap();
        let commit: Commit = serde_json::from_value(out["commit"].clone()).unwrap();
        assert_eq!(commit.summary, "Update README");
        assert_eq!(commit.body, "With the guide.");
        assert_eq!(commit.parents, [head]);
        assert_eq!(out["previous"], head.to_string());
        assert_eq!(out["branch"], "refs/heads/main");
        assert_eq!(out["backup"], Value::Null);
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        assert_eq!(
            out["status"]["head"]["Branch"]["target"],
            commit.id.to_string()
        );
        // The same commit is at the top of `repo.log`.
        let (log, _) = log_page(&r, &mut s, json!({ "limit": 1 }));
        assert_eq!(log, std::slice::from_ref(&commit));

        // Nothing staged any more.
        let err = r
            .run("commit.create", &mut s, json!({ "message": "again" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NothingToCommit), "{err}");

        // `amend` of an unpublished commit: without confirmation, with the backup.
        let out = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "Update README and guide", "amend": true }),
            )
            .unwrap();
        assert_eq!(out["previous"], commit.id.to_string());
        assert_eq!(out["backup"], "refs/project/backup/amend-1");
        assert_eq!(out["commit"]["parents"], json!([head.to_string()]));

        // Published HEAD: `amend` asks for `confirm`.
        let mut repo = MemoryBackend::sample();
        repo.set_remote_branch("origin/main", head);
        let mut opener = MemoryOpener::new();
        opener.insert("published", repo);
        let mut s = Session::new(Arc::new(opener));
        r.run("repo.open", &mut s, json!({ "path": "published" }))
            .unwrap();
        let err = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "x", "amend": true }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");
        let out = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "x", "amend": true, "confirm": true }),
            )
            .unwrap();
        assert_eq!(out["previous"], head.to_string());
    }

    #[test]
    fn undo_redo_and_history() {
        let (r, mut s) = opened();
        let start = MemoryBackend::sample().head().unwrap().target().unwrap();
        let original = r.run("repo.status", &mut s, json!({})).unwrap();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(
            history,
            json!({
                "done": [], "undone": [],
                "can_undo": false, "can_redo": false,
                "next_undo": null, "next_redo": null
            })
        );
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
        let err = r.run("edit.redo", &mut s, Value::Null).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        let first = r
            .run("commit.create", &mut s, json!({ "message": "One" }))
            .unwrap();
        let first_id = first["commit"]["id"].as_str().unwrap().to_owned();
        // Stage and unstage do not go into the journal.
        r.run("index.stage", &mut s, json!({ "paths": ["src/lib.rs"] }))
            .unwrap();
        let second = r
            .run("commit.create", &mut s, json!({ "message": "Two" }))
            .unwrap();
        let second_id = second["commit"]["id"].as_str().unwrap().to_owned();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        let done = history["done"].as_array().unwrap();
        assert_eq!(done.len(), 2);
        assert_eq!(done[0]["id"], 1);
        assert_eq!(done[1]["command"], "commit.create");
        assert_eq!(
            done[1]["refs"],
            json!([{ "name": "refs/heads/main", "before": first_id, "after": second_id }])
        );
        assert_eq!(
            history["next_undo"]["text"],
            format!("Commit {} “Two”", &second_id[..7])
        );

        // Two undos in a row.
        let out = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(
            out["undone"]["description"]["text"],
            done[1]["description"]["text"]
        );
        assert_eq!(out["status"]["head"]["Branch"]["target"], first_id);
        assert_eq!(out["can_undo"], true);
        assert_eq!(out["can_redo"], true);
        assert_eq!(out["backups"], json!(["refs/project/backup/undo-1"]));
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        let out = r.run("edit.undo", &mut s, json!({})).unwrap();
        // Index and worktree are not touched: the changes of the two commits are staged
        // (also `src/lib.rs`, staged before the second).
        assert_eq!(out["status"]["entries"][0], original["entries"][0]);
        assert_eq!(out["status"]["entries"][1], original["entries"][1]);
        assert_eq!(out["status"]["entries"][2]["path"], "src/lib.rs");
        assert_eq!(out["status"]["entries"][2]["index"], "Modified");
        assert_eq!(out["status"]["entries"][2]["worktree"], "Unmodified");
        assert_eq!(out["status"]["head"]["Branch"]["target"], start.to_string());
        assert_eq!(out["can_undo"], false);
        assert_eq!(
            out["next_redo"]["text"],
            format!("Commit {} “One”", &first_id[..7])
        );

        // Redo of the last undone (the first commit).
        let out = r.run("edit.redo", &mut s, json!({})).unwrap();
        assert_eq!(out["redone"]["id"], 1);
        assert_eq!(out["status"]["head"]["Branch"]["target"], first_id);
        assert_eq!(out["can_redo"], true);

        // A new operation empties the redo.
        r.run("commit.create", &mut s, json!({ "message": "Other" }))
            .unwrap();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["can_redo"], false);
        assert_eq!(history["undone"], json!([]));
        assert_eq!(history["done"].as_array().unwrap().len(), 2);
        let err = r.run("edit.redo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        // Reopening the repo resets the journal.
        r.run(
            "repo.open",
            &mut s,
            json!({ "path": MemoryOpener::SAMPLE_PATH }),
        )
        .unwrap();
        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        assert_eq!(history["done"], json!([]));

        // Without a repo.
        let mut empty = session();
        for id in ["edit.undo", "edit.redo", "edit.history"] {
            let err = r.run(id, &mut empty, json!({})).unwrap_err();
            assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
        }
    }

    #[test]
    fn discard_needs_confirm_and_undo_restores_the_files() {
        let (r, mut s) = opened();
        let sample = MemoryBackend::sample();
        let before = r.run("repo.status", &mut s, json!({})).unwrap();
        let diff_before = r.run("diff.worktree", &mut s, json!({})).unwrap();

        // Without `confirm`: nothing changes, the error says what would be lost.
        for params in [
            json!({ "paths": ["src", "README.md"] }),
            json!({ "paths": ["src", "README.md"], "confirm": false }),
        ] {
            let err = r.run("worktree.discard", &mut s, params).unwrap_err();
            let EngineError::ConfirmRequired {
                command, reason, ..
            } = &err
            else {
                panic!("{err}");
            };
            assert_eq!(*command, "worktree.discard");
            assert!(reason.contains("`README.md`, `src/lib.rs`"), "{reason}");
            assert!(err.to_string().contains("\"confirm\": true"), "{err}");
        }
        assert_eq!(r.run("repo.status", &mut s, json!({})).unwrap(), before);
        assert_eq!(
            r.run("edit.history", &mut s, json!({})).unwrap()["done"],
            json!([])
        );
        // A refusal comes before the confirmation.
        let err = r
            .run("worktree.discard", &mut s, json!({ "paths": ["note.txt"] }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        let out = r
            .run(
                "worktree.discard",
                &mut s,
                json!({ "paths": ["src", "README.md"], "confirm": true }),
            )
            .unwrap();
        assert_eq!(out["backup"], "refs/project/backup/discard-1");
        assert_eq!(out["files"][0]["path"], "README.md");
        assert_eq!(out["files"][0]["change"], "Modified");
        assert_eq!(out["files"][1]["path"], "src/lib.rs");
        assert_eq!(out["stash"].as_str().unwrap().len(), 40);
        assert_eq!(out["can_undo"], true);
        assert_eq!(
            out["next_undo"]["text"],
            "Discard changes to 2 files (“README.md”, “src/lib.rs”)"
        );
        assert_eq!(
            out["status"],
            r.run("repo.status", &mut s, json!({})).unwrap()
        );
        let diff = r.run("diff.worktree", &mut s, json!({})).unwrap();
        assert_eq!(diff["files"], json!([]));
        // The staged diff does not change.
        assert_eq!(
            r.run("diff.index", &mut s, json!({})).unwrap(),
            to_json_diff(&sample, DiffSpec::Index { path: None })
        );

        let history = r.run("edit.history", &mut s, json!({})).unwrap();
        let entry = &history["done"][0];
        assert_eq!(entry["command"], "worktree.discard");
        assert_eq!(entry["refs"], json!([]));
        assert_eq!(entry["backup"], "refs/project/backup/discard-1");
        assert_eq!(entry["worktree"]["stash"], out["stash"]);
        assert_eq!(entry["worktree"]["files"], out["files"]);

        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(undo["undone"]["command"], "worktree.discard");
        assert_eq!(undo["status"], before);
        assert_eq!(
            r.run("diff.worktree", &mut s, json!({})).unwrap(),
            diff_before
        );
        let redo = r.run("edit.redo", &mut s, json!({})).unwrap();
        assert_eq!(redo["status"], out["status"]);
    }

    fn to_json_diff(repo: &MemoryBackend, spec: DiffSpec) -> Value {
        diff_with_digests(repo.diff(&spec).unwrap())
    }

    #[test]
    fn undo_of_an_amend() {
        let (r, mut s) = opened();
        let out = r
            .run("commit.create", &mut s, json!({ "message": "One" }))
            .unwrap();
        let out2 = r
            .run(
                "commit.create",
                &mut s,
                json!({ "message": "One again", "amend": true }),
            )
            .unwrap();
        assert_eq!(out2["previous"], out["commit"]["id"]);
        let undo = r.run("edit.undo", &mut s, json!({})).unwrap();
        assert_eq!(
            undo["status"]["head"]["Branch"]["target"],
            out["commit"]["id"]
        );
        // The undone `amend` commit is saved; the rewritten one was in the amend backup.
        assert_eq!(undo["backups"], json!(["refs/project/backup/undo-1"]));
        assert_eq!(
            undo["undone"]["backup"],
            json!("refs/project/backup/amend-1")
        );
    }

    #[test]
    fn errors() {
        let (r, mut s) = opened();
        assert!(matches!(
            r.run("nope", &mut s, json!({})),
            Err(EngineError::UnknownCommand(_))
        ));
        for (id, params) in [
            ("repo.open", json!({})),
            ("repo.open", json!({ "path": "x", "other": 1 })),
            ("repo.status", json!({ "x": 1 })),
            ("repo.log", json!({ "limit": -1 })),
            ("repo.log", json!({ "page": 2 })),
            ("commit.show", json!({})),
            ("commit.show", json!({ "id": 3 })),
            ("commit.show", json!({ "id": "HEAD", "path": "a" })),
            ("diff.worktree", json!({ "id": "HEAD" })),
            ("diff.index", json!({ "paths": ["a"] })),
            ("diff.index", json!({ "path": 3 })),
            ("diff.commit", json!({})),
            ("diff.commit", json!({ "path": "a" })),
            ("diff.commit", json!({ "id": "HEAD", "rev": "HEAD" })),
            ("diff.range", json!({ "from": "HEAD" })),
            (
                "diff.range",
                json!({ "from": "HEAD", "to": "HEAD", "base": true }),
            ),
            (
                "diff.range",
                json!({ "from": "HEAD", "to": "HEAD", "merge_base": "yes" }),
            ),
            ("index.stage", json!({})),
            ("index.stage", json!({ "paths": [] })),
            ("index.stage", json!({ "paths": "README.md" })),
            ("index.stage", json!({ "path": "README.md" })),
            (
                "index.unstage",
                json!({ "paths": ["README.md"], "confirm": true }),
            ),
            ("index.unstage", json!({ "paths": [3] })),
            ("index.stage_lines", json!({})),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [{ "new": 2 }] }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [], "digest": "x" }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [2], "digest": "x" }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [{ "row": 2 }], "digest": "x" }),
            ),
            (
                "index.stage_lines",
                json!({ "path": "src/lib.rs", "lines": [{ "new": -1 }], "digest": "x" }),
            ),
            (
                "index.unstage_lines",
                json!({ "path": "README.md", "lines": [{ "new": 1 }], "digest": "x", "confirm": true }),
            ),
            (
                "index.unstage_lines",
                json!({ "paths": ["README.md"], "lines": [{ "new": 1 }], "digest": "x" }),
            ),
            ("commit.create", json!({})),
            ("commit.create", json!({ "message": 3 })),
            ("commit.create", json!({ "message": "" })),
            ("commit.create", json!({ "message": " \n\t\n" })),
            ("commit.create", json!({ "message": "x", "amend": "yes" })),
            ("commit.create", json!({ "message": "x", "paths": ["a"] })),
            (
                "commit.create",
                json!({ "message": "x", "allow_empty": true }),
            ),
            (
                "commit.create",
                json!({ "message": "x", "no_verify": true }),
            ),
            ("worktree.discard", json!({})),
            ("worktree.discard", json!({ "confirm": true })),
            ("worktree.discard", json!({ "paths": [], "confirm": true })),
            (
                "worktree.discard",
                json!({ "paths": ["README.md"], "confirm": "yes" }),
            ),
            (
                "worktree.discard",
                json!({ "paths": ["README.md"], "confirm": true, "untracked": true }),
            ),
            (
                "worktree.discard",
                json!({ "path": "README.md", "confirm": true }),
            ),
        ] {
            let err = r.run(id, &mut s, params).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{id}: {err}"
            );
        }
        // Paths that are not file paths of the repo.
        for path in [
            json!(""),
            json!("/etc/passwd"),
            json!("../fuori"),
            json!("a/../b"),
            json!("./a"),
            json!("a//b"),
            json!("folder/"),
            json!([97, 0, 98]),
        ] {
            for (id, params) in [
                ("diff.worktree", json!({ "path": path })),
                ("diff.index", json!({ "path": path })),
                ("diff.commit", json!({ "id": "HEAD", "path": path })),
                (
                    "diff.range",
                    json!({ "from": "HEAD~1", "to": "HEAD", "path": path }),
                ),
                ("index.stage", json!({ "paths": ["README.md", path] })),
                ("index.unstage", json!({ "paths": [path] })),
                (
                    "index.stage_lines",
                    json!({ "path": path, "lines": [{ "new": 1 }], "digest": "x" }),
                ),
                (
                    "index.unstage_lines",
                    json!({ "path": path, "lines": [{ "old": 1 }], "digest": "x" }),
                ),
            ] {
                let err = r.run(id, &mut s, params).unwrap_err();
                assert!(
                    matches!(err, EngineError::InvalidParams { .. }),
                    "{id} {path}: {err}"
                );
            }
        }
        let err = r
            .run("repo.open", &mut s, json!({ "path": "altrove" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::RepoNotFound { .. }), "{err}");
        assert_eq!(s.path(), Some(MemoryOpener::SAMPLE_PATH));
    }
}
