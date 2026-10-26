//! Catalog of the built-in commands.

pub(crate) mod branch;
mod clone;
mod conflict;
mod file;
mod merge;
mod pick;
pub(crate) mod pull;
pub(crate) mod push;
mod rebase;
pub(crate) mod remote;
pub(crate) mod remote_branch;
pub(crate) mod remote_tag;
mod remotes;
mod reset;
mod search;
pub(crate) mod stash;
pub(crate) mod tag;

use crate::commands::parse_params;
use crate::{
    CommandSpec, CommitRequest, DiffSpec, EngineError, GitBackend, HeadState, Journal,
    JournalEntry, LineSelection, LogQuery, Message, Registry, RestoreOutcome, Session,
    commit_graph_disabled, commit_graph_replaced, commit_graph_shallow, discard_plan, file_digest,
};
use project_domain::{Diff, LineRef, PatchDirection, RepoPath};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Commits returned by `repo.log` when `limit` is missing.
pub const DEFAULT_LOG_LIMIT: usize = 200;

impl Registry {
    /// Registry with all the built-in commands.
    pub fn builtin() -> Self {
        let mut r = Self::default();
        let mut add = |id, title, params_help, handler| {
            r.register(
                CommandSpec {
                    id,
                    title,
                    params_help,
                    writes: WRITES.contains(&id),
                },
                handler,
            )
        };
        add(
            "repo.open",
            "Open repository",
            r#"{"path": string}"#,
            repo_open,
        );
        add(
            "repo.clone",
            "Clone a repository into an absent or empty folder and open it",
            r#"{"url": string, "path": string, "branch"?: string, "depth"?: number (> 0)}"#,
            clone::repo_clone,
        );
        add(
            "repo.init",
            "New empty repository in an absent or empty folder, then open it",
            r#"{"path": string, "branch"?: string (without: init.defaultBranch)}"#,
            clone::repo_init,
        );
        add("repo.status", "Repository status", "{}", repo_status);
        add("repo.refs", "Branches, tags and stashes", "{}", repo_refs);
        add(
            "repo.operation",
            "Operation in progress (merge, rebase...) and conflicted files",
            "{}",
            merge::repo_operation,
        );
        add(
            "repo.log",
            "Commit history",
            r#"{"skip"?: number, "limit"?: number (200), "rev"?: string, "all"?: bool, "stashes"?: bool (with all: also the stash bases)}"#,
            repo_log,
        );
        add(
            "repo.search",
            "Commits whose message, author or id contains a text (literal, case-insensitive), in the order of repo.log",
            r#"{"query": string, "in"?: ["message" | "author" | "id"] (all three), "path"?: path (only commits touching it, no rename following), "all"?: bool, "stashes"?: bool (as in repo.log), "skip"?: number, "limit"?: number (200)} -> {"ids", "total", "has_more"}"#,
            search::repo_search,
        );
        add(
            "repo.optimize",
            "Write the commit-graph (fast history on huge repos)",
            "{}",
            repo_optimize,
        );
        add(
            "commit.show",
            "Commit details",
            r#"{"id": string}"#,
            commit_show,
        );
        add(
            "file.blame",
            "Who last changed each line of a file",
            r#"{"path": path, "rev"?: string (without: the worktree, uncommitted lines have commit null)}"#,
            file::file_blame,
        );
        add(
            "file.log",
            "Commits that change a file",
            r#"{"path": path, "rev"?: string, "skip"?: number, "limit"?: number (200), "follow"?: bool (true: across renames)}"#,
            file::file_log,
        );
        add(
            "diff.worktree",
            "Diff of the unstaged changes",
            r#"{"path"?: path}"#,
            diff_worktree,
        );
        add(
            "diff.index",
            "Diff of the staged changes",
            r#"{"path"?: path}"#,
            diff_index,
        );
        add(
            "diff.commit",
            "Commit diff",
            r#"{"id": string, "path"?: path}"#,
            diff_commit,
        );
        add(
            "index.stage",
            "Stage files",
            r#"{"paths": [path, ...]}"#,
            index_stage,
        );
        add(
            "index.unstage",
            "Unstage files",
            r#"{"paths": [path, ...]}"#,
            index_unstage,
        );
        add(
            "index.stage_lines",
            "Stage the chosen lines",
            r#"{"path": path, "lines": [{"old": n} | {"new": n}, ...], "digest": string (from diff.worktree)}"#,
            index_stage_lines,
        );
        add(
            "index.unstage_lines",
            "Unstage the chosen lines",
            r#"{"path": path, "lines": [{"old": n} | {"new": n}, ...], "digest": string (from diff.index)}"#,
            index_unstage_lines,
        );
        add(
            "commit.create",
            "Commit the staged changes",
            r#"{"message": string, "amend"?: bool, "confirm"?: bool (amend of a published commit)}"#,
            commit_create,
        );
        add(
            "worktree.discard",
            "Discard the unstaged changes",
            r#"{"paths": [path, ...], "confirm": true}"#,
            worktree_discard,
        );
        add(
            "branch.create",
            "Create a branch",
            r#"{"name": string, "start"?: string (rev, HEAD)}"#,
            branch::branch_create,
        );
        add(
            "branch.checkout",
            "Switch to a branch",
            r#"{"name": string}"#,
            branch::branch_checkout,
        );
        add(
            "branch.delete",
            "Delete a branch",
            r#"{"name": string, "confirm"?: bool (unmerged branch), "expected"?: full oid (the branch must point there)}"#,
            branch::branch_delete,
        );
        add(
            "branch.rename",
            "Rename a branch",
            r#"{"name": string, "new_name": string}"#,
            branch::branch_rename,
        );
        add(
            "branch.reset",
            "Reset the current branch to a commit",
            r#"{"target": string (rev), "mode": "soft"|"mixed"|"hard", "confirm"?: bool (hard with local changes), "expected"?: full oid (HEAD must be there)}"#,
            reset::branch_reset,
        );
        add(
            "branch.reset_preview",
            "What a reset would do (files a hard overwrites)",
            r#"{"target": string (rev), "mode": "soft"|"mixed"|"hard"}"#,
            reset::branch_reset_preview,
        );
        add(
            "tag.create",
            "Create a tag",
            r#"{"name": string, "target"?: string (rev, HEAD), "message"?: string (annotated tag; without: lightweight)}"#,
            tag::tag_create,
        );
        add(
            "tag.delete",
            "Delete a tag",
            r#"{"name": string, "confirm": true (always required; the tag is saved in a backup ref first), "expected"?: string (full id of the value shown in the confirmation)}"#,
            tag::tag_delete,
        );
        add(
            "remote.fetch",
            "Download from the remote (fetch)",
            r#"{"remote"?: string (configured remote; upstream, origin or the only one), "branch"?: string (only that branch), "prune"?: bool (also remove the remote branches gone from the remote; not with branch)}"#,
            remote::remote_fetch,
        );
        add(
            "remote.pull",
            "Download and integrate the upstream (pull)",
            r#"{"remote"?: string (the upstream's), "branch"?: string (local branch; HEAD), "strategy"?: "merge" | "rebase" | "ff_only" (instead of the configuration)}"#,
            pull::remote_pull,
        );
        add(
            "remote.push",
            "Publish the branch to the remote (push)",
            r#"{"remote"?: string (upstream, origin or the only one), "branch"?: string (local branch; HEAD), "force"?: bool (force-with-lease), "confirm"?: bool (force that removes commits from the remote)}"#,
            push::remote_push,
        );
        add(
            "remote.delete_branch",
            "Delete a branch from the remote",
            r#"{"remote": string, "branch": string (name on the remote), "confirm": true, "expected": string (full id of the remote-tracking branch shown in the confirmation)}"#,
            remote_branch::remote_delete_branch,
        );
        add(
            "remote.push_tag",
            "Publish a tag to the remote",
            r#"{"remote": string, "name": string (local tag; never overwrites a tag on the remote)}"#,
            remote_tag::remote_push_tag,
        );
        add(
            "remote.delete_tag",
            "Delete a tag from the remote",
            r#"{"remote": string, "name": string (tag on the remote), "confirm": true, "expected": string (full id of the remote tag shown in the confirmation)}"#,
            remote_tag::remote_delete_tag,
        );
        add(
            "remote.list",
            "Configured remotes",
            "{}",
            remotes::remote_list,
        );
        add(
            "remote.add",
            "Add a remote",
            r#"{"name": string, "url": string, "fetch"?: bool (then download)}"#,
            remotes::remote_add,
        );
        add(
            "remote.remove",
            "Remove a remote",
            r#"{"name": string, "confirm": true}"#,
            remotes::remote_remove,
        );
        add("stash.list", "Stashes", "{}", stash::stash_list);
        add(
            "stash.push",
            "Stash the changes",
            r#"{"message"?: string (one line), "include_untracked"?: bool, "keep_index"?: bool, "paths"?: [path, ...]}"#,
            stash::stash_push,
        );
        add(
            "stash.apply",
            "Apply a stash",
            r#"{"index"?: number (stash@{n}, 0), "expected"?: full oid (the stash seen), "restore_index"?: bool}"#,
            stash::stash_apply,
        );
        add(
            "stash.pop",
            "Apply a stash and remove it from the list",
            r#"{"index"?: number (stash@{n}, 0), "expected"?: full oid (the stash seen), "restore_index"?: bool}"#,
            stash::stash_pop,
        );
        add(
            "stash.drop",
            "Drop a stash",
            r#"{"index"?: number (stash@{n}, 0), "expected": full oid (the stash to drop; required with confirm), "confirm": true}"#,
            stash::stash_drop,
        );
        add(
            "merge.start",
            "Merge a branch into the current branch",
            r#"{"rev": string (branch, remote branch, tag or id), "no_ff"?: bool (always a merge commit), "ff_only"?: bool (fast-forward only), "message"?: string (of the merge commit)}"#,
            merge::merge_start,
        );
        add(
            "merge.continue",
            "Finish the merge in progress",
            r#"{"message"?: string (otherwise the one prepared by git)}"#,
            merge::merge_continue,
        );
        add(
            "merge.abort",
            "Abort the merge in progress",
            r#"{"confirm": true}"#,
            merge::merge_abort,
        );
        add(
            "commit.cherry_pick",
            "Apply a commit onto the current branch (cherry-pick)",
            r#"{"rev": string (id, branch or tag of the commit), "mainline"?: number (for a merge commit: the parent, from 1)}"#,
            pick::commit_cherry_pick,
        );
        add(
            "cherry_pick.continue",
            "Finish the cherry-pick in progress",
            r#"{"message"?: string (otherwise the one prepared by git)}"#,
            pick::cherry_pick_continue,
        );
        add(
            "cherry_pick.abort",
            "Abort the cherry-pick in progress",
            r#"{"confirm": true}"#,
            pick::cherry_pick_abort,
        );
        add(
            "commit.revert",
            "Undo a commit with a new commit (revert)",
            r#"{"rev": string (id, branch or tag of the commit), "mainline"?: number (for a merge commit: the parent, from 1)}"#,
            pick::commit_revert,
        );
        add(
            "revert.continue",
            "Finish the revert in progress",
            r#"{"message"?: string (otherwise the one prepared by git)}"#,
            pick::revert_continue,
        );
        add(
            "revert.abort",
            "Abort the revert in progress",
            r#"{"confirm": true}"#,
            pick::revert_abort,
        );
        add(
            "rebase.plan",
            "Default plan of an interactive rebase of the current branch",
            r#"{"base": string (the commit to rebuild on, excluded: id, branch or tag)}"#,
            rebase::rebase_plan,
        );
        add(
            "rebase.start",
            "Rewrite the commits of the current branch (interactive rebase)",
            r#"{"base": string, "steps": [{"action": "pick" | "reword" | "squash" | "fixup" | "drop", "commit": full oid, "message"?: string}] (oldest first, every commit of base..HEAD once), "confirm"?: bool (rewrites published commits)}"#,
            rebase::rebase_start,
        );
        add(
            "rebase.continue",
            "Continue the rebase in progress",
            "{}",
            rebase::rebase_continue,
        );
        add(
            "rebase.abort",
            "Abort the rebase in progress",
            r#"{"confirm": true}"#,
            rebase::rebase_abort,
        );
        add(
            "conflict.show",
            "Versions of a conflicted file (base, ours, theirs, worktree)",
            r#"{"path": path}"#,
            conflict::conflict_show,
        );
        add(
            "conflict.resolve",
            "Resolve a conflicted file and stage it",
            r#"{"path": path, "digest": string (from conflict.show), "content"?: string (the resolved text), "delete"?: bool (deletes the file; instead of content)}"#,
            conflict::conflict_resolve,
        );
        add("edit.undo", "Undo the last operation", "{}", edit_undo);
        add("edit.redo", "Redo the undone operation", "{}", edit_redo);
        add("edit.history", "Operation journal", "{}", edit_history);
        r
    }
}

/// The commands that modify the repository ([`CommandSpec::writes`]).
const WRITES: &[&str] = &[
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
    "index.unstage",
    "index.stage_lines",
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
    "worktree.discard",
];

type Out = Result<Value, EngineError>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoParams {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathParam {
    path: String,
}

fn to_json(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("the domain types are serializable")
}

/// `{"path", "head", "commit_graph"}` of the repo just opened. `path`: the worktree root,
/// absolute ([`GitBackend::root`]), so the same repo opened from a
/// subfolder or a relative path gives the same path; the given path if the backend has none.
/// `commit_graph`: git reads a
/// commit-graph for the repo ([`CommitGraphState::in_use`](crate::CommitGraphState::in_use)); without it `repo.log` is slow on
/// huge histories and the UI can offer `repo.optimize`. A failed check gives `false` (the
/// repo opens anyway).
fn repo_open(s: &mut Session, p: Value) -> Out {
    let PathParam { path } = parse_params("repo.open", p)?;
    s.open(&path)?;
    opened(s, path)
}

/// The result of `repo.open` for the repo just opened at `path` (also `repo.clone`, `repo.init`).
fn opened(s: &Session, path: String) -> Out {
    let repo = s.repo()?;
    let path = repo.root().unwrap_or(path);
    let head = repo.head()?;
    let commit_graph = repo.commit_graph().is_ok_and(|g| g.in_use());
    Ok(json!({ "path": path, "head": head, "commit_graph": commit_graph }))
}

/// Writes the commit-graph of all reachable commits (`git commit-graph write --reachable`), so
/// that `repo.log` (`--date-order`) no longer parses the whole history before the first page.
///
/// A write (it adds or replaces `objects/info/commit-graph`), but it cannot lose data: the file
/// is a cache that git derives from the objects and that `git gc` itself rewrites. So no
/// `confirm`, no backup, and no [`JournalEntry`]: no ref, index or worktree file changes.
/// Slow (~19 s on the Linux kernel): progress and cancellation from [`Session::job`]. Refused
/// with `core.commitGraph=false` ([`commit_graph_disabled`]), in a shallow repo
/// ([`commit_graph_shallow`]) and with replace refs or grafts ([`commit_graph_replaced`]): git
/// would not read it.
///
/// Result: `{"commit_graph": bool}`, the state afterwards (as in `repo.open`).
fn repo_optimize(s: &mut Session, p: Value) -> Out {
    const ID: &str = "repo.optimize";
    let NoParams {} = parse_params(ID, p)?;
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
    Ok(json!({ "commit_graph": after.in_use() }))
}

/// The [`Status`](project_domain::Status) of the open repo.
fn repo_status(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("repo.status", p)?;
    Ok(to_json(s.repo()?.status()?))
}

/// The [`RefInfo`](project_domain::RefInfo) of the open repo, in name order.
fn repo_refs(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("repo.refs", p)?;
    Ok(to_json(s.repo()?.refs()?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdParam {
    id: String,
}

/// The [`CommitDetails`](project_domain::CommitDetails) of commit `id` (also abbreviated, or a
/// ref name): `{"commit", "compared_to", "files"}`, with the files compared to the first
/// parent.
fn commit_show(s: &mut Session, p: Value) -> Out {
    let IdParam { id } = parse_params("commit.show", p)?;
    Ok(to_json(s.repo()?.show_commit(&id)?))
}

/// Parameters of `diff.worktree` and `diff.index`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiffParams {
    #[serde(default)]
    path: Option<RepoPath>,
}

/// Parameters of `diff.commit`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommitDiffParams {
    id: String,
    #[serde(default)]
    path: Option<RepoPath>,
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
/// pass to `index.stage_lines`/`index.unstage_lines`. Whoever reads the result as a
/// [`Diff`] ignores it.
fn diff_with_digests(diff: &Diff) -> Value {
    let mut out = to_json(diff);
    if let Some(files) = out["files"].as_array_mut() {
        for (json, file) in files.iter_mut().zip(&diff.files) {
            json["digest"] = Value::String(file_digest(file));
        }
    }
    out
}

/// The [`Diff`] of the worktree against the index (unstaged changes): `{"files": [...]}`,
/// each file with its `digest`. Untracked files do not appear.
fn diff_worktree(s: &mut Session, p: Value) -> Out {
    let DiffParams { path } = parse_params("diff.worktree", p)?;
    let path = check_opt_path("diff.worktree", path)?;
    Ok(diff_with_digests(
        &s.repo()?.diff(&DiffSpec::Worktree { path })?,
    ))
}

/// The [`Diff`] of the index against HEAD (staged changes), each file with its
/// `digest`.
fn diff_index(s: &mut Session, p: Value) -> Out {
    let DiffParams { path } = parse_params("diff.index", p)?;
    let path = check_opt_path("diff.index", path)?;
    Ok(diff_with_digests(
        &s.repo()?.diff(&DiffSpec::Index { path })?,
    ))
}

/// The [`Diff`] of commit `id` against the first parent (like
/// `commit.show`).
fn diff_commit(s: &mut Session, p: Value) -> Out {
    let CommitDiffParams { id, path } = parse_params("diff.commit", p)?;
    let path = check_opt_path("diff.commit", path)?;
    Ok(to_json(
        s.repo()?.diff(&DiffSpec::Commit { rev: id, path })?,
    ))
}

/// Parameters of `index.stage` and `index.unstage`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsParams {
    paths: Vec<RepoPath>,
}

/// The paths of `index.stage`/`index.unstage`, validated as in `diff.*` and without duplicates
/// (in the given order). An empty list is an error: for git "no path" would mean
/// "everything", and it is better that it cannot be asked for by mistake.
fn check_paths(id: &'static str, p: Value) -> Result<Vec<RepoPath>, EngineError> {
    let PathsParams { paths } = parse_params(id, p)?;
    check_path_list(id, paths)
}

/// Like [`check_paths`], for an already read list.
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

/// Stages `paths` (see [`GitBackend::stage`]).
/// Result: `{"status": Status, "backup": string | null}`, with the new
/// [`Status`](project_domain::Status) (the caller updates the view without a second command) and
/// the index backup ref, if before writing staged versions
/// that existed only in the index were saved.
fn index_stage(s: &mut Session, p: Value) -> Out {
    index_paths("index.stage", s, p, |repo, paths| repo.stage(paths))
}

/// Unstages `paths` (see [`GitBackend::unstage`]),
/// without touching the worktree. Result like `index.stage`.
fn index_unstage(s: &mut Session, p: Value) -> Out {
    index_paths("index.unstage", s, p, |repo, paths| repo.unstage(paths))
}

/// The write of `index.stage`/`index.unstage`: the index backup ref, if one was saved.
type IndexWrite = fn(&mut dyn GitBackend, &[RepoPath]) -> Result<Option<String>, EngineError>;

/// `index.stage`/`index.unstage`: `write` on the validated paths, then the new status.
fn index_paths(id: &'static str, s: &mut Session, p: Value, write: IndexWrite) -> Out {
    let paths = check_paths(id, p)?;
    let repo = s.repo_mut()?;
    let backup = write(repo, &paths)?;
    Ok(json!({ "status": repo.status()?, "backup": backup }))
}

/// Parameters of `index.stage_lines` and `index.unstage_lines`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LinesParams {
    path: RepoPath,
    lines: Vec<LineRef>,
    digest: String,
}

/// The `lines` of file `path` staged or unstaged, with the diff seen identified by
/// `digest` (see [`GitBackend::apply_lines`]). An empty
/// selection is an error, not a no-op: it is almost always a mistake by the
/// caller. Result like `index.stage`.
fn index_lines(id: &'static str, direction: PatchDirection, s: &mut Session, p: Value) -> Out {
    let LinesParams {
        path,
        lines,
        digest,
    } = parse_params(id, p)?;
    let path = check_path(id, path)?;
    if lines.is_empty() {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-lines-empty"),
        });
    }
    let repo = s.repo_mut()?;
    let backup = repo.apply_lines(&LineSelection {
        path,
        lines,
        digest,
        direction,
    })?;
    Ok(json!({ "status": repo.status()?, "backup": backup }))
}

/// Stages the chosen lines of the worktree diff.
fn index_stage_lines(s: &mut Session, p: Value) -> Out {
    index_lines("index.stage_lines", PatchDirection::Stage, s, p)
}

/// Unstages the chosen lines of the index diff.
fn index_unstage_lines(s: &mut Session, p: Value) -> Out {
    index_lines("index.unstage_lines", PatchDirection::Unstage, s, p)
}

/// Creates a commit with what is staged, or with `amend` rewrites the last one (see
/// [`GitBackend::commit`]). `confirm` is needed only for `amend` of a
/// commit already published.
///
/// Result: `{"commit": Commit, "previous": string | null, "branch": string | null, "backup":
/// string | null, "also_on": [string], "status": Status}`: the created commit (as in `repo.log`), HEAD before
/// (`null` for the first commit), the moved branch with its full name (`null` with HEAD
/// detached), the backup ref of the rewritten commit (only with `amend`), the other branches and tags
/// that remain on the rewritten commit (only with `amend`) and the new state. An error after a
/// successful commit is [`EngineError::PostCommit`].
fn commit_create(s: &mut Session, p: Value) -> Out {
    let request: CommitRequest = parse_params("commit.create", p)?;
    request.check_message()?;
    let (repo, journal) = s.repo_and_journal()?;
    let outcome = repo.commit(&request)?;
    // The commit is made: it goes into the journal (for `edit.undo`) even if the state cannot be re-read afterwards.
    if let Some(entry) = JournalEntry::commit(&outcome, request.amend) {
        journal.record(entry);
    }
    // The commit is made: an error from here on must not look like a failed commit.
    let status = repo.status().map_err(|e| EngineError::PostCommit {
        commit: Some(outcome.commit.id),
        backup: outcome.backup.clone(),
        reason: msg!("engine-status-unreadable", error = e),
    })?;
    let mut out = to_json(outcome);
    out["status"] = to_json(status);
    Ok(out)
}

/// Parameters of `worktree.discard`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscardParams {
    paths: Vec<RepoPath>,
    #[serde(default)]
    confirm: bool,
}

/// Discards the unstaged changes of the files `paths` (files or folders): the worktree goes back
/// to the index, the index is not touched (see
/// [`GitBackend::discard`] and [`discard_plan`]: untracked files are
/// never touched). Needs `"confirm": true`: without it, [`EngineError::ConfirmRequired`] with the files
/// that would be lost and nothing changes (the refusals of [`discard_plan`] come first). Before
/// writing, the worktree is saved with `git stash create` in `refs/project/backup/discard-...`;
/// the operation goes into the journal and `edit.undo` puts the files back as they were.
///
/// Result: `{"files": [{"path", "change", "mode", "oid"}], "backup": string, "stash":
/// string, "status": Status | null, "can_undo", "can_redo", "next_undo", "next_redo"}`; the
/// state is `null` (with `status_error`) if it cannot be re-read: the discard is done anyway.
fn worktree_discard(s: &mut Session, p: Value) -> Out {
    const ID: &str = "worktree.discard";
    let DiscardParams { paths, confirm } = parse_params(ID, p)?;
    let paths = check_path_list(ID, paths)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let files = discard_plan(&status, &paths)?;
    if !confirm {
        let names: Vec<String> = files.iter().map(|f| format!("`{}`", f.path)).collect();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-discard-confirm",
                n = files.len(),
                files = names.join(", ")
            ),
            expected: None,
        });
    }
    // The backend re-reads the state and refuses if the files are no longer these.
    let outcome = repo.discard(&paths, &files)?;
    // The files are discarded: the entry goes into the journal even if the state cannot be re-read afterwards.
    journal.record(JournalEntry::discard(
        &outcome,
        HeadState::from_head(&status.head),
    ));
    let mut out = journal_state(journal);
    out["files"] = to_json(&outcome.files);
    out["backup"] = Value::String(outcome.backup);
    out["stash"] = Value::String(outcome.stash.to_string());
    put_reread(&mut out, "status", repo.status());
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

/// `{"can_undo", "can_redo", "next_undo", "next_redo"}`: see [`JournalState`].
fn journal_state(journal: &Journal) -> Value {
    to_json(JournalState::of(journal))
}

/// What a command re-reads after a successful write, as the pair of fields `<key>` and
/// `<key>_error` of a typed result (see [`put_reread`]): the value, or `None` and the error text.
fn reread<T>(result: Result<T, EngineError>) -> (Option<T>, Option<String>) {
    match result {
        Ok(value) => (Some(value), None),
        Err(e) => (None, Some(e.to_string())),
    }
}

/// Adds the fields of [`journal_state`] to `out`.
fn put_journal_state(out: &mut Value, journal: &Journal) {
    if let Value::Object(state) = journal_state(journal) {
        for (key, value) in state {
            out[key] = value;
        }
    }
}

/// Puts what a command re-reads after a successful write at `out[key]` (`refs`, `status`...);
/// if it cannot be re-read, `null` and the error at `out["<key>_error"]`: the operation is done
/// anyway and must not look failed (or be repeated).
fn put_reread<T: Serialize>(out: &mut Value, key: &str, result: Result<T, EngineError>) {
    match result {
        Ok(value) => out[key] = to_json(value),
        Err(e) => {
            out[key] = Value::Null;
            out[format!("{key}_error")] = Value::String(e.to_string());
        }
    }
}

/// Result of `edit.undo`/`edit.redo`: `{<key>: entry, "backups": [string], "warnings":
/// [string], "status": Status | null, "can_undo", "can_redo", "next_undo", "next_redo"}`.
/// `warnings` reports something unexpected found after the successful move (usually
/// empty). The state is `null` (with
/// `status_error`) if it cannot be re-read: the operation is done anyway and must not be repeated (a
/// second undo would undo the previous entry).
fn undo_result(s: &mut Session, key: &str, entry: JournalEntry, outcome: RestoreOutcome) -> Out {
    let mut out = journal_state(s.journal()?);
    out[key] = to_json(entry);
    out["backups"] = to_json(outcome.backups);
    out["warnings"] = to_json(outcome.warnings);
    put_reread(&mut out, "status", s.repo()?.status());
    Ok(out)
}

/// Undoes the last operation of the journal (see [`Journal::undo`]): the refs go back to their previous
/// values, only if nobody moved them in the meantime ([`EngineError::UndoConflict`]
/// otherwise, and nothing changes). Index and worktree are not touched. Result: see
/// [`undo_result`], with the entry in `undone`.
fn edit_undo(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.undo", p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let (entry, outcome) = journal.undo(repo)?;
    undo_result(s, "undone", entry, outcome)
}

/// Redoes the last undone operation (see [`Journal::redo`]), with the same check.
/// Result: see [`undo_result`], with the entry in `redone`.
fn edit_redo(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.redo", p)?;
    let (repo, journal) = s.repo_and_journal()?;
    let (entry, outcome) = journal.redo(repo)?;
    undo_result(s, "redone", entry, outcome)
}

/// The journal of the open repo: `{"done": [entry], "undone": [entry], "can_undo", "can_redo",
/// "next_undo", "next_redo"}`. In `done` the last entry is the one `edit.undo` undoes, in
/// `undone` the last is the one `edit.redo` redoes.
fn edit_history(s: &mut Session, p: Value) -> Out {
    let NoParams {} = parse_params("edit.history", p)?;
    let journal = s.journal()?;
    let mut out = journal_state(journal);
    out["done"] = to_json(journal.done());
    out["undone"] = to_json(journal.undone());
    Ok(out)
}

/// A page of the log: `{"commits": [...], "has_more": bool}`. `has_more` says whether after
/// the last returned commit there are others (the next page starts at `skip + limit`).
fn repo_log(s: &mut Session, p: Value) -> Out {
    let query: LogQuery = parse_params("repo.log", p)?;
    let limit = query.limit.unwrap_or(DEFAULT_LOG_LIMIT);
    // One more commit to know whether the page is the last.
    let probe = LogQuery {
        limit: Some(limit.saturating_add(1)),
        ..query
    };
    let mut commits = s.repo()?.log(&probe)?;
    let has_more = commits.len() > limit;
    commits.truncate(limit);
    Ok(json!({ "commits": commits, "has_more": has_more }))
}

#[cfg(test)]
#[path = "builtin/snapshot_tests.rs"]
mod snapshot_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GitBackend, MemoryBackend, MemoryOpener};
    use project_domain::{Commit, CommitDetails, Diff, RefInfo, Status};
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
        let parsed: project_domain::FileDiff = serde_json::from_value(file.clone()).unwrap();
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
        let parsed: project_domain::FileDiff =
            serde_json::from_value(diff["files"][0].clone()).unwrap();
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
        diff_with_digests(&repo.diff(&spec).unwrap())
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
