//! Access to a git repository through a trait (see `impl-docs/ARCHITECTURE-git.md` §4).
//!
//! The engine does not run `git` and does not touch the file system: it reads the repo through
//! [`GitBackend`], opened by a [`RepoOpener`] injected by the app. The implementations are
//! `MemoryBackend` (here, for tests: feature `test-support`) and `CliBackend` (in `project-git`).

mod backup;
mod branch;
mod clone;
mod commit;
mod conflict;
mod discard;
mod file;
#[cfg(any(test, feature = "test-support"))]
mod line_diff;
mod lines;
mod maintenance;
#[cfg(any(test, feature = "test-support"))]
mod memory;
mod merge;
mod pick;
mod pull;
mod push;
mod rebase;
mod remote;
mod remote_admin;
mod reset;
mod restore;
#[cfg(any(test, feature = "test-support"))]
mod sample;
mod search;
mod stash;
mod tag;

#[cfg(test)]
mod memory_diff_tests;
#[cfg(test)]
mod memory_tests;

pub use backup::{
    BACKUP_PREFIX, StagedVersion, backup_message, path_matches, versions_at_risk,
    versions_at_risk_hidden,
};
pub use branch::{
    BranchDeleted, ConfigEntry, HEADS_PREFIX, TrackingBranch, UpstreamChange, branch_ref,
    branch_short, check_branch_name, check_new_branch_name, check_no_operation, resolve_tracking,
    upstream_label,
};
pub use clone::{
    CloneOutcome, CloneRequest, InitRequest, clone_no_such_branch, destination_no_parent,
    destination_not_empty, destination_not_folder,
};
pub use commit::{
    CommitOutcome, CommitRequest, amend_impact, check_amend_published, check_commit_state,
};
#[cfg(any(test, feature = "test-support"))]
pub use commit::{cleanup_message, split_message};
pub use conflict::{
    ConflictResolution, ConflictResolved, ConflictVersions, DEFAULT_MARKER_SIZE, VersionText,
    check_conflict, check_digest, marker_size, worktree_digest,
};
pub use discard::{
    DiscardOutcome, DiscardedFile, WorktreeBackup, WorktreeRestore, check_same_plan,
    discard_description, discard_plan,
};
#[cfg(any(test, feature = "test-support"))]
pub use file::is_binary_content;
pub use file::{
    BlameQuery, FileBlame, FileLogQuery, blame_binary, blame_is_binary, blame_missing_at,
    blame_missing_worktree, blame_no_commits,
};
pub use lines::{LinePatch, LineSelection, apply_patch, file_digest, line_patch};
pub use maintenance::{
    CommitGraphState, commit_graph_disabled, commit_graph_replaced, commit_graph_shallow,
};
#[cfg(any(test, feature = "test-support"))]
pub use memory::{FetchFailure, MemoryBackend, MemoryOpener, StashedFile, WorktreeStash};
pub(crate) use merge::check_in_progress;
pub use merge::{
    MergeAborted, MergeMode, MergeOutcome, MergeRequest, MergeSource, MergeSourceKind,
    OperationState, check_merge_state, check_merging, clean_merge_message, conflicted_paths,
    merge_source,
};
pub use pick::{
    PickKind, PickRequest, check_pick_state, check_picking, pick_empty_error, pick_message,
    pick_parent,
};
pub use pull::{
    PullMode, PullOperation, PullOutcome, PullPolicy, PullRequest, PullSettings, PullStrategy,
    StrategySource, UpstreamConfig, check_pull_state, merge_message, pull_mode,
};
pub use push::{
    PushKind, PushOutcome, PushRejection, PushRequest, RemoteBranchDelete, RemoteBranchDeleted,
    RemoteTagDelete, TagPush, TagPushed, tracking_ref,
};
pub use rebase::{
    MAX_REBASE_COMMITS, REBASE_RECORD, RebaseRecord, RebaseRequest, check_rebase_request,
    check_rebase_state, check_rebasing, explicit_squash_messages, plan_error, rebase_range,
    rewritten_commits,
};
pub use remote::{FetchOutcome, FetchRequest, check_remote_name, default_remote, unknown_remote};
pub use remote_admin::{
    REMOTES_PREFIX, RemoteRef, RemoteSnapshot, RemoteStillListed, check_new_remote_name,
    check_remote_url, ensure_remote_gone, mask_url, masked_entry, owned_by, remote_dependents,
    remove_remote, restore_remote, split_branch_key,
};
pub use reset::{
    ResetMode, ResetOutcome, ResetRecord, ResetRequest, check_same_reset_files, reset_files,
    reset_moved, status_paths, strict_conflicts,
};
pub use restore::{HeadState, RefMove, RefRestore, RestoreOutcome, valid_ref_name};
pub use search::{SearchField, SearchQuery};
pub use stash::{
    INDEX_NOT_RESTORED, STASH_REF, StashApplied, StashApply, StashDrop, StashDropped, StashEntry,
    StashPopped, StashPush, StashRecord, check_stash_apply, check_stash_push, find_stash,
    pop_stash, stash_blocked, stash_branch, stash_local_changes,
};
pub use tag::{TAGS_PREFIX, check_new_tag_name, check_tag_name, tag_ref};

use crate::{EngineError, JobContext, Message};
use project_domain::{
    Commit, CommitDetails, Diff, FileLogEntry, Head, Oid, RefInfo, RepoPath, Status,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Reading: refs, history, status, diffs, blame, configuration. Never writes. Part of
///
/// [`GitBackend`].
pub trait GitRead {
    /// Where HEAD points.
    fn head(&self) -> Result<Head, EngineError>;

    /// Local branches, remote branches, tags and stashes, sorted by full name (byte by byte),
    /// like `git for-each-ref`. For local branches `upstream` reports the full name of the remote
    /// ref (e.g. `refs/remotes/origin/main`) and ahead/behind, `None` if the ref no longer exists.
    fn refs(&self) -> Result<Vec<RefInfo>, EngineError>;

    /// Commits reachable from `query.rev` (HEAD if missing), children always before parents and,
    /// on ties, the most recent by committer date first. On a branch without commits and without
    /// `rev` the result is empty; a `rev` that does not resolve gives
    /// [`EngineError::UnknownRev`]. With `query.all` it starts from all local and remote branches, the
    /// tags and HEAD (a HEAD without commits is skipped); `rev` and `all` together give
    /// [`EngineError::InvalidParams`] ([`LogQuery::check`]).
    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError>;

    /// All the commits that match `query` (`repo.search`), in the order of
    /// [`GitRead::log`] over [`SearchQuery::range`]: the text literally in any of the fields
    /// (OR), only among the commits that touch `query.path` if given. Slow on long histories:
    /// `job` cancels it ([`EngineError::Cancelled`]). HEAD without commits (without `all`) →
    /// empty.
    fn search(&self, query: &SearchQuery, job: &JobContext) -> Result<Vec<Oid>, EngineError>;

    /// State of the worktree and the index, with HEAD and upstream of the current branch.
    fn status(&self) -> Result<Status, EngineError>;

    /// Metadata and changed files of the commit `rev` leads to (an id, also abbreviated, or
    /// any rev accepted by [`GitRead::log`]). The files are compared with the **first
    /// parent**, also for merges (the files the merge brought into the branch); a root commit is
    /// compared with the empty tree. Renames are detected. A `rev` that does not
    /// resolve to a commit gives [`EngineError::UnknownRev`].
    fn show_commit(&self, rev: &str) -> Result<CommitDetails, EngineError>;

    /// The diff described by `spec` (see [`DiffSpec`]), with renames detected.
    ///
    /// With a path in `spec` the result has only the file with that **exact** path,
    /// new or old: a folder does not include the files it contains, and a rename stays
    /// a rename (with the other path) even if only one of the two is requested. A path
    /// that has not changed gives an empty diff.
    ///
    /// There is no size limit: the diff of a huge commit is all in memory. For
    /// large commits it is better to ask for one file at a time (the list comes from
    /// [`GitRead::show_commit`]). For [`DiffSpec::Commit`] a `rev` that does not resolve to
    /// a commit gives [`EngineError::UnknownRev`].
    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError>;

    /// Who last changed each line of `query.path` (`file.blame`): at commit `query.rev`, or in
    /// the worktree (lines not yet committed have no commit). Slow on long histories: `job`
    /// cancels it ([`EngineError::Cancelled`]). Errors: `rev` that does not resolve →
    /// [`EngineError::UnknownRev`]; path that is not a file there ([`blame_missing_at`],
    /// [`blame_missing_worktree`]), worktree on a branch without commits
    /// ([`blame_no_commits`]) → [`EngineError::InvalidParams`]; binary file → [`blame_binary`].
    fn blame(&self, query: &BlameQuery, job: &JobContext) -> Result<FileBlame, EngineError>;

    /// The commits that change `query.path` (`file.log`), newest first, from HEAD or
    /// `query.rev`, with the file's name in each; with `follow` across renames. Merges are
    /// compared with the first parent and shown only if the file differs from every parent.
    /// Slow on long histories: `job` cancels it. A `rev` that does not resolve →
    /// [`EngineError::UnknownRev`]; HEAD without commits → empty.
    fn file_log(
        &self,
        query: &FileLogQuery,
        job: &JobContext,
    ) -> Result<Vec<FileLogEntry>, EngineError>;

    /// Absolute worktree root, `None` if the backend has no folder (`MemoryBackend`) or its
    /// path is not UTF-8. `repo.open` returns it in place of the given path.
    fn root(&self) -> Option<String> {
        None
    }

    /// How many commits reachable from `tip` are reachable from none of `into` (`git rev-list
    /// --count <tip> ^<into>...`): 0 means `tip` is merged. A read, for the confirmation of
    /// `branch.delete`.
    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError>;

    /// All the values of the configuration key `key` (all sources, as git reads it:
    /// `git config --get-all`), in order; empty if none. A read. The keys are
    /// built by the engine from already checked names (e.g. `branch.<name>.pushRemote`,
    /// `remote.<name>.fetch`).
    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError>;

    /// The commit `rev` leads to (`rev^{commit}`), `None` if it does not resolve to a commit
    /// (unknown, ambiguous, a tree or a blob). A read, for `diff.range`.
    fn commit_id(&self, rev: &str) -> Result<Option<Oid>, EngineError>;

    /// The best common ancestor of commits `a` and `b` (`git merge-base`), `None` if they have
    /// no common history. With several best ones, the one git picks (the one `git diff a...b`
    /// uses). A read, for `diff.range` with `merge_base`.
    fn common_base(&self, a: &Oid, b: &Oid) -> Result<Option<Oid>, EngineError>;

    /// The `branch.<name>.remote` and `branch.<name>.merge` entries of the **repo file** only
    /// (`git config --local`: no global configuration, no `include`), in file order: the upstream
    /// that `branch.set_upstream` and `branch.create` with `track` replace. A non-UTF-8 value
    /// gives [`EngineError::NotAllowed`] (it could not be written back). A read.
    fn local_upstream(&self, name: &str) -> Result<Vec<ConfigEntry>, EngineError>;
}

/// Index and worktree: stage, unstage, commit, discard and their restores. Part of
/// [`GitBackend`].
///
/// Writes (here and in the other traits) take `&mut self` because the `MemoryBackend` holds the
/// state itself; a read done afterwards always sees the new state.
pub trait GitIndex {
    // Stage and unstage return the name of the backup ref (`refs/project/backup/...`) if
    // before writing they saved staged versions that existed only in the index
    // ([`versions_at_risk`]); if the backup fails the index is not touched and the result is
    // the error.
    //
    // The risk check reads the state before writing: between the read and the write
    // another git process could change the index (short window, only with concurrent
    // external writes). The `CliBackend` rechecks the touched entries right before
    // writing and, if they changed, fails without writing; only the instant between
    // that last check and the write remains.

    /// Stages the files `paths` as they were in the worktree (`git add`): modifications, new
    /// untracked files and deletions. For a rename done in the worktree both paths are passed
    /// (the deletion of the old one and the new file). Does not touch the worktree.
    ///
    /// - Paths are taken literally (no globs or magic pathspecs); a path that
    ///   is a folder includes the files it contains, as in git (ignored files inside it
    ///   are skipped).
    /// - A file ignored by `.gitignore` and untracked, requested by name, gives
    ///   [`EngineError::NotAllowed`] and **nothing** is staged (never forced).
    /// - A path that exists neither in the worktree nor in the index nor in HEAD gives an error and
    ///   nothing changes; one (file or folder) that only indicates deletions already staged
    ///   does nothing.
    /// - A conflicted file that is staged is **resolved** with the worktree content (like
    ///   `git add`): the conflict stages disappear from the index (git keeps their versions
    ///   in the REUC extension, from which `git checkout -m` recreates the conflict). Only if requested by
    ///   name: a conflicted file inside a requested folder gives
    ///   [`EngineError::NotAllowed`] and nothing changes.
    /// - Empty `paths` does nothing (it does not mean "everything").
    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;

    /// Unstages the files `paths`: the index goes back as in HEAD for those paths (an
    /// added file becomes untracked again), like `git restore --staged`. On a branch without
    /// commits the index becomes empty for those paths. **Never touches the worktree.**
    ///
    /// - Literal paths, folders included, as in [`GitIndex::stage`].
    /// - A conflicted file gives [`EngineError::NotAllowed`]: removing the conflict stages
    ///   from the index would lose their versions; it is resolved with [`GitIndex::stage`].
    /// - A path unknown to git (neither in HEAD nor in the index) gives an error and nothing changes.
    /// - Empty `paths` does nothing.
    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError>;

    /// Stages ([`PatchDirection::Stage`](project_domain::PatchDirection)) or unstages
    /// ([`PatchDirection::Unstage`](project_domain::PatchDirection)) only the lines
    /// `selection.lines` of a file: `index.stage_lines` and `index.unstage_lines`. **Never
    /// touches the worktree.**
    ///
    /// Rereads the file's diff ([`LineSelection::diff_spec`]), checks that it is the one seen
    /// (`selection.digest`) and builds the patch with [`line_patch`] (see there for the errors), then
    /// computes the new staged content with [`apply_patch`] (strict: each hunk only at
    /// its position) and writes it to the index. Like stage and unstage: first a backup if the
    /// staged version exists only in the index (e.g. lines of an `MM` file), and the ref in the
    /// result; the `CliBackend` rechecks the index before writing, rereads the entry afterwards
    /// and with the index locked gives [`EngineError::IndexLocked`]. An error before the write
    /// changes nothing.
    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError>;

    /// Creates a commit with what is staged (`commit.create`), or with `request.amend`
    /// rewrites the last one. The worktree is not touched; afterwards, the index equals the new HEAD.
    ///
    /// Before writing: [`CommitRequest::check_message`], [`check_commit_state`] (conflicts,
    /// operations in progress, nothing to commit, `amend` without a commit or during a merge) and,
    /// with `amend`, [`check_amend_published`] on the remote branches and tags that contain HEAD
    /// ([`amend_impact`]; the other branches and tags that contain it go in `also_on`). With
    /// `amend` the rewritten commit is saved in a backup ref
    /// (`refs/project/backup/amend-...`, in the result) before writing; if the backup
    /// fails nothing is committed. The `CliBackend` runs `git commit` with hooks and signing per the
    /// user's configuration: a rejecting hook or a failing signature gives
    /// [`EngineError::Git`] with everything they wrote, and no commit.
    ///
    /// The first commit of a branch and detached HEAD work (with detached HEAD it moves
    /// HEAD, no branch). Without an identity (`user.name`/`user.email`) the error is git's:
    /// no invented name.
    fn commit(&mut self, request: &CommitRequest) -> Result<CommitOutcome, EngineError>;

    /// Discards the unstaged changes of the files `paths` (`worktree.discard`, §7): the worktree
    /// goes back as the index, like `git restore --worktree`. **The index is not touched.**
    ///
    /// The files are those of [`discard_plan`] on the state read now (see there for the refusals: untracked
    /// files requested by name, submodules, conflicts, `intent-to-add`, branch without
    /// commits, nothing to discard); untracked files inside a folder are not touched. Before
    /// writing, the worktree is saved with `git stash create` in a new ref
    /// `refs/project/backup/discard-...` (in the result, with the commit and the files); if the backup
    /// fails, or does not contain the current content of all files, nothing is discarded.
    /// Empty `paths` does nothing (error: nothing to discard).
    ///
    /// `expected` is the plan shown to whoever confirmed ([`discard_plan`] on a state read
    /// earlier): if the plan computed now is different (one file more or less, another
    /// change, another staged version), [`EngineError::NotAllowed`] and nothing changes.
    fn discard(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError>;

    /// Undoes (`request.redo == false`) or redoes a discard (`edit.undo`/`edit.redo` of an entry
    /// of `worktree.discard`). The index is not touched.
    ///
    /// Undo: every file must still have the recorded entry in the index
    /// ([`WorktreeRestore::index_conflicts`]) and the worktree equal to the index (as the discard
    /// left it); then it goes back as it was in commit `request.stash` (a file that is not there, because
    /// it was deleted, is removed). Redo: same index entry and worktree equal to
    /// `request.stash` (as the undo left it); then it goes back to the staged version. If a file
    /// changed, [`EngineError::UndoConflict`] and nothing changes.
    fn restore_worktree(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<RestoreOutcome, EngineError>;
}

/// Refs and journal: branches, tags, checkout, reset, and the ref restores of undo/redo. Part
/// of [`GitBackend`].
///
/// Branch names are short (`feature/x`), already checked by the command with
/// [`check_branch_name`] / [`check_new_branch_name`]; backends receive them only as
/// `refs/heads/<name>` or after `--end-of-options`, so a name starting with `-` does not become an
/// option.
pub trait GitRefs {
    /// Moves HEAD and the refs as `request` asks (`edit.undo` and `edit.redo`, §7): all together
    /// or none, and **only** if `expected` holds now. Index and worktree are not touched.
    /// Returns the backup refs created.
    ///
    /// Before writing: [`RefRestore::check`] (operations in progress, names, unsupported
    /// HEAD changes); then HEAD and the current refs compared with `expected`
    /// ([`RefRestore::conflicts`]): if something changed from outside,
    /// [`EngineError::UndoConflict`] and nothing changes. Every commit of
    /// [`RefRestore::abandoned`] that afterwards would no longer be reachable from the new values
    /// or from other refs (backups included) is saved in `refs/project/backup/<undo|redo>-...`;
    /// if the backup fails nothing is moved. The `CliBackend` writes with a single
    /// `git update-ref --stdin` transaction (`start`, `update`/`create`/`delete` with the old
    /// value, `commit`) and the message `request.reflog`; if the transaction fails because
    /// a ref changed in the meantime, again [`EngineError::UndoConflict`]. A problem
    /// found after a successful transaction (HEAD changed by another process at exactly
    /// that instant) goes in [`RestoreOutcome::warnings`].
    fn restore_refs(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError>;

    /// Creates the local branch `name` on commit `target` (`branch.create`), **only if it does not
    /// exist** (`update-ref refs/heads/<name> <id> ""`: an existing ref is never
    /// overwritten, not even if created by another process the instant before). HEAD, index and
    /// worktree are not touched; no upstream. A name already taken (also by a ref that
    /// contains it, like `a` for `a/b`) gives [`EngineError::NotAllowed`]; a name git rejects gives
    /// [`EngineError::InvalidParams`].
    fn create_branch(&mut self, name: &str, target: &Oid) -> Result<(), EngineError>;

    /// Creates the tag `name` on commit `target` (`tag.create`), **only if it does not exist**:
    /// lightweight without `message`, annotated (a tag object with the user's identity, signed if
    /// `tag.gpgSign` says so) with it. HEAD, index, worktree and the other refs are not touched. A
    /// name already taken gives [`EngineError::NotAllowed`]. Returns the value of the new ref:
    /// the commit, or the tag object for an annotated tag.
    fn create_tag(
        &mut self,
        name: &str,
        target: &Oid,
        message: Option<&str>,
    ) -> Result<Oid, EngineError>;

    /// Deletes the local tag `name` (`tag.delete` and the redo of `tag.create` undone), only if
    /// `refs/tags/<name>` still has the value `expected` (the commit, or the tag object of an annotated tag).
    /// First it saves `expected` in a new backup ref `refs/project/backup/tag-delete-...`
    /// (returned): if the backup fails nothing is deleted. Then `update-ref --no-deref -d
    /// refs/tags/<name> <expected>`; if that fails the backup is removed. A tag missing or with
    /// another value gives [`EngineError::NotAllowed`] and nothing changes.
    fn delete_tag(&mut self, name: &str, expected: &Oid) -> Result<String, EngineError>;

    /// Moves HEAD to `target` updating index and worktree (`git switch`): the local branch
    /// (full name, must exist) or a commit with detached HEAD (for the undo of a checkout
    /// made from detached HEAD). `branch.checkout` and its undo/redo.
    ///
    /// **Never loses local changes**: never `--force`, `--discard-changes` or `--merge`.
    /// Changes that do not touch the files that differ between HEAD and `target` stay (as in git);
    /// if a modified (staged or not), untracked or ignored file would be overwritten,
    /// [`EngineError::CheckoutConflict`] with the files and nothing changes. Rejected before
    /// touching anything ([`EngineError::NotAllowed`]): operations in progress, conflicted files,
    /// a `target` that does not exist. Returns the warnings found after a successful checkout
    /// (e.g. HEAD different from `target` because of a `post-checkout` hook).
    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError>;

    /// Deletes the local branch `name`, only if `expected` still holds (`branch.delete`).
    /// First it saves the commit in a new backup ref `refs/project/backup/delete-...` (in the
    /// result); if the backup fails nothing is deleted. Then it removes the `branch.<name>.*`
    /// configuration (upstream, ...), like `git branch -d`. Rejected
    /// ([`EngineError::NotAllowed`], nothing changes) if it is the branch of HEAD or if it is open in
    /// another worktree, or if `expected` no longer holds. The "merged or not" check is done by the command
    /// ([`GitRead::unmerged_count`]).
    fn delete_branch(&mut self, name: &str, expected: &Oid) -> Result<BranchDeleted, EngineError>;

    /// Renames the local branch `from` to `to` (`branch.rename` and its undo/redo), only if
    /// `from` still equals `expected` and `to` does not exist (never overwritten). Like `git branch
    /// -m`: reflog and `branch.<name>.*` configuration follow the branch, and HEAD (also of other
    /// worktrees) that pointed to `from` points to `to`. Index and worktree are not touched.
    /// Returns the warnings found after the successful rename. Refusals
    /// ([`EngineError::NotAllowed`], nothing changes): `from` missing or changed, `to` already
    /// present; a `to` name that git rejects: [`EngineError::InvalidParams`].
    fn rename_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError>;

    /// The repo's `branch.<name>.*` configuration for undo and redo of `branch.delete`, after
    /// the ref was recreated or deleted. `remove == false` (undo): writes `entries` back,
    /// only if the repo has no `branch.<name>.*` entries yet (otherwise it writes nothing and says so
    /// in a warning). `remove == true` (redo): removes the section, only if it contains exactly
    /// `entries` (what the undo put back); a different configuration stays, with a
    /// warning. Returns the warnings: the ref has already been moved, so a problem here is not an
    /// error of the operation.
    fn restore_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<Message>;

    /// Replaces the upstream of the local branch `name` in the repo file
    /// ([`GitRead::local_upstream`]) with `entries` (`branch.<name>.remote` / `.merge`; empty =
    /// no upstream), **only if** it is still `expected`: otherwise [`EngineError::NotAllowed`]
    /// and nothing changes. Only those two keys are touched (`git config --local`, one key at a
    /// time); if a write fails midway the backend puts `expected` back and returns the error
    /// (saying whether it could). Returns the warnings found re-reading afterwards (another
    /// process wrote the configuration at the same time). `branch.set_upstream`,
    /// `branch.create` with `track` and their undo/redo.
    fn write_upstream(
        &mut self,
        name: &str,
        expected: &[ConfigEntry],
        entries: &[ConfigEntry],
    ) -> Result<Vec<Message>, EngineError>;

    /// Undo and redo of a pull on the active branch: like [`GitRefs::restore_refs`] (a single ref,
    /// the branch of HEAD, from `expected` to `target`, with the same checks and the backups of
    /// abandoned commits), but it also updates index and worktree like `git reset --keep`: the
    /// files that differ between the two commits go back as in `target`, only if they have no local
    /// changes (otherwise [`EngineError::NotAllowed`] and nothing changes); the other changes
    /// stay. It never overwrites an untracked or ignored file.
    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError>;

    // --- Reset (S45, `impl-docs/git/reset.md`).

    /// Moves the branch of HEAD (or a detached HEAD) like `git reset --<mode>`
    /// (`branch.reset`, and its undo and redo). Before writing, without changing anything:
    /// no operation in progress, the target is a commit, HEAD and the branch still hold the
    /// expected values of `request.refs` ([`EngineError::UndoConflict`] for undo/redo,
    /// [`EngineError::NotAllowed`] for `branch.reset`); for `hard` the plan
    /// ([`reset_files`]) is the confirmed one and no untracked or ignored file is where the
    /// target puts a file. Then the backups: `mixed` the staged versions that exist only in the
    /// index (`refs/project/backup/index-...`), `hard` the local changes (`git stash create` and
    /// byte for byte, `refs/project/backup/reset-...`); the commits no longer reachable as for
    /// [`GitRefs::restore_refs`]. If a backup fails nothing is written. The ref moves with
    /// the old value checked by git; then index (`mixed`, `hard`) and worktree (`hard`). An
    /// error after the ref move names the backups. With `request.strict` (undo/redo) nothing may
    /// need an index or worktree backup ([`strict_conflicts`]).
    fn reset(&mut self, request: &ResetRequest) -> Result<ResetOutcome, EngineError>;
}

/// Remotes: fetch, pull, push, remote branches and tags, remote configuration. Part of
/// [`GitBackend`].
pub trait GitRemotes {
    /// The names of the configured remotes (`git remote`), in name order. A read.
    fn remotes(&self) -> Result<Vec<String>, EngineError>;

    /// Downloads from the remote `request.remote` (`git fetch --progress`, §6): new objects and remote
    /// branches (`refs/remotes/<remote>/...`) and tags as the remote's configuration says, plus
    /// `FETCH_HEAD`. Never the local branches, the index or the worktree (except a refspec configured
    /// on purpose towards `refs/heads/`, which the command reports). With `request.branch` only that
    /// branch of the remote (`refs/heads/<branch>`), which updates its remote branch as the configured
    /// refspec says. Slow command: progress goes to `job` ([`JobContext::report`]);
    /// if `job` is cancelled the process stops and the result is
    /// [`EngineError::Cancelled`]. Errors: credentials → [`EngineError::Auth`], remote not
    /// reachable → [`EngineError::RemoteUnreachable`], branch the remote does not have →
    /// [`EngineError::NotAllowed`], other → [`EngineError::Git`]. The remote is already checked
    /// by the command (configured, valid name).
    fn fetch(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError>;

    // --- Pull and push (S26). Names arrive already checked by the command.

    /// The configured upstream of the local branch `branch` (short name): `branch.<name>.remote`,
    /// `branch.<name>.merge` and the local remote branch that mirrors it. `None` if the branch
    /// has no upstream. A read.
    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError>;

    /// The pull strategy configuration for the branch `branch` (`branch.<name>.rebase`
    /// or `pull.rebase`, `pull.ff`), with the values as written. A read.
    fn pull_settings(&self, branch: &str) -> Result<PullSettings, EngineError>;

    /// At most `limit` commits reachable from `tip` but from none of `exclude`, most recent first
    /// (`git log <tip> --not <exclude>...`). A read: for the confirmation of a force push (the
    /// commits the remote would lose).
    fn missing_commits(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError>;

    /// Integrates the upstream into the branch after the fetch (`remote.pull`): fast-forward, merge or
    /// rebase according to `request.mode`. Before writing: [`check_pull_state`] on the state read
    /// now, the branch must equal `request.expected` (and, with `request.current`, be the branch
    /// of HEAD; otherwise it must not be open in another worktree) and a backup ref
    /// `refs/project/backup/pull-...` on the previous value (in the result). Fast-forward of the
    /// active branch: `git merge --ff-only`, which does not overwrite local changes
    /// ([`EngineError::NotAllowed`] with the files, nothing changes); of another branch:
    /// `update-ref` with the old value. Merge and rebase: never `--autostash`. A **merge**
    /// that stops (conflicts, hooks, signing) **stays in progress** as with [`GitOperations::merge`]:
    /// [`PullOutcome::in_progress`] with the conflicted files, the branch as it was and the backup
    /// kept. A **rebase** that stops stays in progress too, with its [`RebaseRecord`] (command
    /// `remote.pull`): `rebase.continue` concludes it with the pull's journal entry. It is not
    /// cancelled midway: the fetch, which can be cancelled, is already done.
    fn pull(&mut self, request: &PullRequest) -> Result<PullOutcome, EngineError>;

    /// Publishes the branch to the remote (`remote.push`, slow like the fetch: progress and
    /// cancellation from `job`): only `refs/heads/<branch>:refs/heads/<remote_branch>`, never tags
    /// or other refs, with `--set-upstream` if requested and `--force-with-lease` with the expected oid if
    /// `request.lease` (never `--force`). Before a force, the backup of `request.backup`.
    /// Errors: rejected (not fast-forward, lease, `pre-push` hook, remote) →
    /// [`EngineError::PushRejected`] and nothing changes; credentials → [`EngineError::Auth`];
    /// remote not reachable → [`EngineError::RemoteUnreachable`]; cancelled →
    /// [`EngineError::Cancelled`].
    fn push(&mut self, request: &PushRequest, job: &JobContext)
    -> Result<PushOutcome, EngineError>;

    /// Deletes the branch `request.branch` from the remote (`remote.delete_branch`, slow like the
    /// push): only `:refs/heads/<branch>`, always with
    /// `--force-with-lease=refs/heads/<branch>:<expected>` (never `--force`, `--prune`,
    /// `--mirror`). Before the push it saves `request.expected` in a backup ref
    /// `refs/project/backup/push-delete-...` (removed if the push is rejected or fails before
    /// starting); git then removes the local remote-tracking branch. Errors as
    /// [`GitRemotes::push`]: rejected (lease, `pre-push` hook, remote, branch already gone) →
    /// [`EngineError::PushRejected`]; credentials → [`EngineError::Auth`]; remote not
    /// reachable → [`EngineError::RemoteUnreachable`]; cancelled → [`EngineError::Cancelled`]
    /// (the backup stays: the remote may have already deleted the branch).
    fn delete_remote_branch(
        &mut self,
        request: &RemoteBranchDelete,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError>;

    /// The value of `refs/tags/<name>` on the remote `remote` (`git ls-remote`, slow: it contacts
    /// the remote; cancellation from `job`), as the remote stores it: the tag object of an annotated
    /// tag, not the peeled commit. `None` if the remote does not have it. Errors as
    /// [`GitRemotes::fetch`]: [`EngineError::Auth`], [`EngineError::RemoteUnreachable`],
    /// [`EngineError::Cancelled`]. A read.
    fn remote_tag(
        &self,
        remote: &str,
        name: &str,
        job: &JobContext,
    ) -> Result<Option<Oid>, EngineError>;

    /// Publishes the local tag `request.name` to the remote (`remote.push_tag`, slow like the
    /// push): only `refs/tags/<name>:refs/tags/<name>`, never `--force` (a tag already on the
    /// remote with another value → [`EngineError::PushRejected`] with
    /// [`PushRejection::TagExists`]); the same value already there is a success with
    /// `up_to_date`. Other errors as [`GitRemotes::push`].
    fn push_tag(&mut self, request: &TagPush, job: &JobContext) -> Result<TagPushed, EngineError>;

    /// Deletes the tag `request.name` from the remote (`remote.delete_tag`), like
    /// [`GitRemotes::delete_remote_branch`]: always with
    /// `--force-with-lease=refs/tags/<name>:<expected>`, after saving `request.expected` in a backup ref
    /// `refs/project/backup/push-delete-...`. If the object is not here it is first fetched from the
    /// remote straight into the backup ref (only that ref: no tags, no `FETCH_HEAD`), which must then
    /// equal `expected` (otherwise [`EngineError::PushRejected`] with
    /// [`PushRejection::StaleLease`], nothing deleted). If the backup cannot be made nothing is
    /// deleted. Backup removal and errors as [`GitRemotes::delete_remote_branch`].
    fn delete_remote_tag(
        &mut self,
        request: &RemoteTagDelete,
        job: &JobContext,
    ) -> Result<RemoteBranchDeleted, EngineError>;

    // --- Remote management (S27). The name arrives already checked by the command; configuration
    // writes touch only the repo file (`git config --local`).

    /// What exists of the remote `name` ([`RemoteSnapshot`]): `remote.<name>.*` of the repo
    /// file, the remote branches `refs/remotes/<name>/...` (not symbolic, excluding those of a
    /// remote `<name>/...`; [`owned_by`]), where `refs/remotes/<name>/HEAD` points and the dependents
    /// ([`remote_dependents`], `remote.pushDefault`). All empty if the remote does not exist.
    /// A read. A non-UTF-8 value (it could not be put back) gives [`EngineError::NotAllowed`].
    fn remote_snapshot(&self, name: &str) -> Result<RemoteSnapshot, EngineError>;

    /// Adds the remote `name` with `url` (`git remote add --end-of-options <name> <url>`: URL
    /// and default refspec `+refs/heads/*:refs/remotes/<name>/*`), without fetch. Name and URL already
    /// checked ([`check_new_remote_name`], [`check_remote_url`]); a remote with that name
    /// that already exists gives [`EngineError::NotAllowed`], a name or URL that git rejects gives
    /// [`EngineError::InvalidParams`].
    fn create_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError>;

    /// Removes the configuration of the remote of `snapshot` (after [`remove_remote`] removed the
    /// remote branches): the section `remote.<name>`, the symbolic ref `refs/remotes/<name>/HEAD`
    /// if `snapshot.head`, and the entries of `snapshot.upstreams` and `remote.pushDefault` if
    /// `snapshot.push_default` (only with those values). Returns the warnings: the refs have already been
    /// removed, so a problem here is not an error of the operation.
    fn remove_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message>;

    /// The opposite of [`GitRemotes::remove_remote_config`] (after [`restore_remote`] put the
    /// remote branches back): writes back `snapshot.config`, the symbolic ref, the entries of
    /// `snapshot.upstreams` and `remote.pushDefault` if `snapshot.push_default`. Returns the
    /// warnings.
    fn write_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message>;

    /// The `remote.<name>.*` entries that git reads **outside** the repo file (global or
    /// system configuration, `include`, `config.worktree`), one per line with key and
    /// origin: `remote.remove` rejects them, because it would remove only part of the remote.
    /// A read.
    fn remote_config_elsewhere(&self, name: &str) -> Result<Vec<String>, EngineError>;
}

/// Stash: list, push, apply, drop, store. Part of [`GitBackend`].
pub trait GitStash {
    // --- Stash (S28). A stash always arrives as an id: the position `stash@{n}` changes on every
    // push and drop, and the backend recomputes it right before writing.

    /// The list of stashes, most recent first (`stash@{0}`), from the reflog of `refs/stash`. Empty
    /// if there are none. A read.
    fn stashes(&self) -> Result<Vec<StashEntry>, EngineError>;

    /// Sets changes aside (`git stash push`): saves them in a new stash at the top
    /// of the list and brings index and worktree back to HEAD (with `keep_index` the index stays, and the
    /// worktree as the index), for the chosen files. First: [`check_stash_push`]. Nothing to
    /// set aside (git does not create the stash) → [`EngineError::NotAllowed`]. Returns the
    /// new stash.
    fn stash_push(&mut self, request: &StashPush) -> Result<StashEntry, EngineError>;

    /// Applies the stash `request.id` to index and worktree (`git stash apply`), without removing it
    /// from the list. First: [`check_stash_apply`] and the files the stash changes or restores must
    /// have no local changes nor be occupied ([`stash_local_changes`], plus the worktree
    /// files the status does not show): otherwise [`EngineError::NotAllowed`] and nothing
    /// changes. A stash that is no longer in the list → [`EngineError::NotAllowed`]. A merge that
    /// stops on conflict is not an error: the conflicted files stay (as git leaves them)
    /// and are in [`StashApplied::conflicts`].
    fn stash_apply(&mut self, request: &StashApply) -> Result<StashApplied, EngineError>;

    /// Removes the stash `request.id` from the list (`git stash drop stash@{n}`, with `n` read
    /// right before), after saving its commit in a new backup ref if
    /// `request.backup` (if the backup fails nothing is removed). A stash that is no
    /// longer in the list → [`EngineError::NotAllowed`]. Index and worktree are not touched.
    fn stash_drop(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError>;

    /// Puts commit `id` back at the top of the stash list with the message `message`
    /// (`git stash store`): the undo of `stash.drop`. Index and worktree are not touched.
    fn stash_store(&mut self, id: &Oid, message: &str) -> Result<(), EngineError>;
}

/// Operations in progress: merge, cherry-pick, revert, rebase and the conflict editor. Part of
/// [`GitBackend`].
pub trait GitOperations {
    // --- Merge (S29). The command resolves the rev, picks the mode and checks the state
    // ([`check_merge_state`]); the backend rechecks before writing.

    /// Merges `request.target` into the branch of HEAD (`merge.start`): fast-forward (`git merge
    /// --ff-only`) or merge commit with `request.message`. Before writing:
    /// [`check_merge_state`] on the state read now, HEAD on branch `request.branch` with value
    /// `request.expected`, no file the merge brings in place of an untracked or
    /// ignored one, and a backup ref `refs/project/backup/merge-...` on the previous value (in the
    /// result; removed if nothing changes). Never `--autostash`. If git refuses because it
    /// would overwrite changes or untracked files → [`EngineError::NotAllowed`] with the files, nothing
    /// changes. **A merge that stops (conflicts, `pre-merge-commit` hook) stays in progress**:
    /// it is not an error, [`MergeOutcome::in_progress`] and the files in [`MergeOutcome::conflicts`].
    fn merge(&mut self, request: &MergeRequest) -> Result<MergeOutcome, EngineError>;

    /// Aborts the merge in progress (`merge.abort`, `git merge --abort`): index and worktree go back
    /// as HEAD for the files of the merge. First it saves index and worktree (conflicts and
    /// resolutions included) in a new ref `refs/project/backup/merge-abort-...` ([`MergeAborted`]); if the
    /// backup fails nothing is aborted. Without a merge in progress
    /// ([`check_merging`]) → [`EngineError::NotAllowed`]. The command asks for confirmation.
    fn merge_abort(&mut self) -> Result<MergeAborted, EngineError>;

    /// The operation in progress with its data (incoming commit, `MERGE_MSG`, rebase
    /// base), for `repo.operation`. A read.
    fn operation_state(&self) -> Result<OperationState, EngineError>;

    // --- Cherry-pick and revert (S31, `impl-docs/git/cherry-pick.md`). The command resolves the
    // rev and the parent and checks the state ([`check_pick_state`]); the backend rechecks.

    /// Applies (cherry-pick) or reverts (revert) the commit `request.commit` on the branch of HEAD
    /// with a new commit. Before writing: [`check_pick_state`] on the state read now, HEAD
    /// on branch `request.branch` with value `request.expected`, no file the operation
    /// brings in place of an untracked or ignored one, no `--assume-unchanged` file that
    /// changes, and a backup ref `refs/project/backup/<cherry-pick|revert>-...` on the previous
    /// value (in the result; removed if nothing changes). Empty result (commit already applied) →
    /// [`EngineError::NotAllowed`], nothing in progress and the backup removed. **A conflict (or a hook
    /// that rejects the commit) leaves the operation in progress**: it is not an error,
    /// [`MergeOutcome::in_progress`] and the files in [`MergeOutcome::conflicts`].
    fn pick(&mut self, request: &PickRequest) -> Result<MergeOutcome, EngineError>;

    /// Aborts the cherry-pick or revert in progress (`git cherry-pick --abort`, `git revert
    /// --abort`) like [`GitOperations::merge_abort`]: first it saves index and worktree in a new
    /// ref `refs/project/backup/<cherry-pick|revert>-abort-...`; if the backup fails nothing
    /// is aborted. It rejects ([`check_picking`]) another operation or a sequence of several
    /// commits started from the terminal. The command asks for confirmation.
    fn pick_abort(&mut self, kind: PickKind) -> Result<MergeAborted, EngineError>;

    // --- Interactive rebase (S32, `impl-docs/git/rebase.md`). The command reads the range,
    // validates the plan and checks the state ([`check_rebase_state`]); the backend rechecks
    // ([`check_rebase_request`]).

    /// Rewrites the commits `request.plan.base..HEAD` of the branch of HEAD according to the plan
    /// (`rebase.start`). Before writing: [`check_rebase_request`] on the state read now, HEAD on
    /// branch `request.branch` with value `request.expected`, no file the rebase writes in place
    /// of an untracked or ignored one, and a backup ref `refs/project/backup/rebase-...` on the
    /// previous value (in the result; removed only if nothing changed). **A stop (conflict, failed
    /// hook or signing, a message that could not be applied) leaves the rebase in progress**: it is
    /// not an error, [`MergeOutcome::in_progress`] and the files in [`MergeOutcome::conflicts`],
    /// with the [`RebaseRecord`] kept for `rebase.continue`.
    fn rebase(&mut self, request: &RebaseRequest) -> Result<MergeOutcome, EngineError>;

    /// Goes on with the rebase in progress (`rebase.continue`): the commit stopped on keeps its
    /// message, then the rest of the plan with its messages ([`RebaseRecord`]). Refused
    /// ([`EngineError::NotAllowed`]) without a rebase in progress or with conflicted files. Stopped
    /// again → [`MergeOutcome::in_progress`]; concluded → [`MergeOutcome::after`] is the branch
    /// afterwards. [`MergeOutcome::backup`] is the record's backup (empty without a record).
    fn rebase_continue(&mut self) -> Result<MergeOutcome, EngineError>;

    /// Aborts the rebase in progress (`rebase.abort`, `git rebase --abort`): the branch, index and
    /// worktree go back to before the rebase. First it saves index and worktree (and the files the
    /// abort would overwrite) in a new ref `refs/project/backup/rebase-abort-...`, whose parent is
    /// the commit the rebase had reached; if the backup fails nothing is aborted. Without a rebase
    /// in progress → [`EngineError::NotAllowed`]. The command asks for confirmation.
    fn rebase_abort(&mut self) -> Result<MergeAborted, EngineError>;

    // --- Conflict editor (S30, `impl-docs/git/conflicts.md`).

    /// The versions of the conflicted file `path` for `conflict.show`: stages 1/2/3 of the index
    /// (unfiltered bytes, `None` if the stage is missing), bytes of the worktree file (`None` if
    /// absent) and `conflict-marker-size`. A read. `path` not conflicted, submodule, link or
    /// folder in the worktree → [`EngineError::NotAllowed`] ([`check_conflict`]).
    fn conflict_versions(&self, path: &RepoPath) -> Result<ConflictVersions, EngineError>;

    /// Resolves the conflict of `request.path` (`conflict.resolve`): writes `request.content`
    /// to the worktree (or deletes the file if `None`) and stages it like
    /// [`GitIndex::stage`] of that single path. Before writing, without changing anything:
    /// [`check_conflict`], [`check_digest`] on the worktree bytes read now
    /// ([`EngineError::StaleDiff`]), locked index ([`EngineError::IndexLocked`]); then the
    /// backup of the worktree bytes (if the file exists) in `refs/project/backup/conflict-...`: if it
    /// fails nothing is written. An error after the backup names it.
    fn resolve_conflict(
        &mut self,
        request: &ConflictResolution,
    ) -> Result<ConflictResolved, EngineError>;
}

/// Maintenance: the commit-graph. Part of [`GitBackend`].
pub trait GitMaintenance {
    /// Whether the repo has a commit-graph git reads (`repo.open`, `repo.optimize`): a graph in
    /// the object directory or in an alternate, and `core.commitGraph`. Cheap (no walk).
    fn commit_graph(&self) -> Result<CommitGraphState, EngineError>;

    /// Writes the commit-graph of all reachable commits (`git commit-graph write --reachable`,
    /// `repo.optimize`) into the repo's own object directory, replacing the one there. A cache
    /// derived from the objects: no ref, index or worktree changes. Slow on huge repos (~19 s
    /// on the Linux kernel): progress goes to `job`, a cancelled `job` stops git
    /// ([`EngineError::Cancelled`]).
    fn write_commit_graph(&mut self, job: &JobContext) -> Result<(), EngineError>;
}

/// An open repository: the union of the traits by area ([`GitRead`], [`GitIndex`], [`GitRefs`],
/// [`GitRemotes`], [`GitStash`], [`GitOperations`], [`GitMaintenance`]). Every type that
/// implements all of them is a `GitBackend` (blanket impl); `Session` and the commands hold a
/// `Box<dyn GitBackend>`. A caller with a concrete backend (`MemoryBackend`, `CliBackend`)
/// imports the traits of the methods it calls.
///
/// `Send` because the session may move it to a worker thread for slow commands (§6).
pub trait GitBackend:
    GitRead + GitIndex + GitRefs + GitRemotes + GitStash + GitOperations + GitMaintenance + Send
{
}

impl<T> GitBackend for T where
    T: GitRead
        + GitIndex
        + GitRefs
        + GitRemotes
        + GitStash
        + GitOperations
        + GitMaintenance
        + Send
        + ?Sized
{
}

/// What to compare in [`GitRead::diff`]: parameters of `diff.worktree`, `diff.index` and
/// `diff.commit` and `diff.range` (§4). `path`, if present, limits the diff to that file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSpec {
    /// Worktree relative to the index: the unstaged changes (`git diff`). Untracked files
    /// do not appear.
    Worktree { path: Option<RepoPath> },
    /// Index relative to HEAD: the staged changes (`git diff --cached`). On a branch without
    /// commits the comparison is with the empty tree (every staged file is added).
    Index { path: Option<RepoPath> },
    /// The commit `rev` leads to relative to the **first parent**, also for merges; a
    /// root commit relative to the empty tree. The same files as
    /// [`GitRead::show_commit`].
    Commit { rev: String, path: Option<RepoPath> },
    /// Commit `to` relative to commit `from` (`git diff <from> <to>`), whatever their relation:
    /// `diff.range`, which resolves the revisions (and the merge base) first.
    Range {
        from: Oid,
        to: Oid,
        path: Option<RepoPath>,
    },
}

impl DiffSpec {
    /// The file the diff is limited to, if any.
    pub fn path(&self) -> Option<&RepoPath> {
        match self {
            DiffSpec::Worktree { path }
            | DiffSpec::Index { path }
            | DiffSpec::Commit { path, .. }
            | DiffSpec::Range { path, .. } => path.as_ref(),
        }
    }
}

/// Opens a repository given the path. The app injects it into the session: desktop and CLI use
/// `CliOpener` of `project-git`, the tests `MemoryOpener`.
pub trait RepoOpener: Send + Sync {
    /// [`EngineError::RepoNotFound`] if there is no repository at the path.
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError>;

    /// `repo.clone` (`impl-docs/git/clone.md`): clones `request.url` into `request.path`, which
    /// must be absent or an empty folder ([`destination_not_empty`], [`destination_not_folder`],
    /// [`destination_no_parent`]: nothing written). Progress and cancellation from `job`; on
    /// cancellation ([`EngineError::Cancelled`]) or error the destination goes back to how it was
    /// (a folder the call created is removed). The request is already checked
    /// ([`CloneRequest::check`]). The caller then opens `request.path` with [`RepoOpener::open`].
    fn clone_repo(
        &self,
        request: &CloneRequest,
        job: &JobContext,
    ) -> Result<CloneOutcome, EngineError>;

    /// `repo.init`: a new empty repository in `request.path`, with the same destination rules
    /// as [`RepoOpener::clone_repo`]. The caller then opens it with [`RepoOpener::open`].
    fn init_repo(&self, request: &InitRequest) -> Result<(), EngineError>;
}

/// A page of the log: parameters of `repo.log` (§4), `{"skip"?, "limit"?, "rev"?, "all"?,
/// "stashes"?}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LogQuery {
    /// Commits to skip from the start of the log.
    pub skip: usize,
    /// Maximum number of commits; `None` = all.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Where to start: name of a ref (`main`, `origin/main`, `v1.0`, `refs/heads/main`),
    /// `HEAD` or an id (also abbreviated, at least 4 digits). `None` = HEAD.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
    /// Starts from all local branches, remote branches, tags and HEAD (also detached), like
    /// `git log --branches --remotes --tags HEAD`. The other refs (`refs/stash`, the backups in
    /// `refs/project/backup/`, notes) are not included. Excludes `rev`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    /// Only with `all`: also starts from the stash bases ([`StashEntry::base`], the commit
    /// each was created on), so the graph always has the commit the UI attaches a
    /// stash to, even if no branch contains it anymore. The commits of the stashes (and those of the index
    /// and untracked files that have them as parents) stay out: the list is `stash.list`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub stashes: bool,
}

impl LogQuery {
    /// Error if the parameters contradict each other: `rev` and `all` together, `stashes` without `all`.
    pub fn check(&self) -> Result<(), EngineError> {
        if self.all && self.rev.is_some() {
            return Err(EngineError::InvalidParams {
                id: "repo.log",
                reason: msg!("engine-log-rev-and-all"),
            });
        }
        if self.stashes && !self.all {
            return Err(EngineError::InvalidParams {
                id: "repo.log",
                reason: msg!("engine-log-stashes-without-all"),
            });
        }
        Ok(())
    }

    /// Same page, starting from all branches, tags and HEAD.
    pub fn from_all_refs(mut self) -> Self {
        self.all = true;
        self
    }

    /// The whole log from HEAD.
    pub fn all() -> Self {
        Self::default()
    }

    /// `limit` commits after the first `skip`, from HEAD.
    pub fn page(skip: usize, limit: usize) -> Self {
        Self {
            skip,
            limit: Some(limit),
            rev: None,
            all: false,
            stashes: false,
        }
    }

    /// Same page, with the stash bases too (goes with [`LogQuery::from_all_refs`]).
    pub fn with_stashes(mut self) -> Self {
        self.stashes = true;
        self
    }

    /// Same page, starting from `rev`.
    pub fn from_rev(mut self, rev: impl Into<String>) -> Self {
        self.rev = Some(rev.into());
        self
    }
}
