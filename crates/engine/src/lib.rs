//! L5 — UI-independent engine.
//!
//! Every operation on the repository goes through [`Registry::run`], used the same way by UI,
//! CLI, MCP server and tests. Git repositories are read through the [`GitBackend`] and
//! [`RepoOpener`] traits; the [`Session`] holds the open repo.

#[macro_use]
mod message;
mod builtin;
mod commands;
mod git;
mod job;
mod journal;
mod platform;
mod session;

pub use builtin::DEFAULT_LOG_LIMIT;
pub use commands::{CommandSpec, Handler, Registry};
pub use git::{
    BACKUP_PREFIX, BranchDeleted, CommitOutcome, CommitRequest, ConfigEntry, DiffSpec,
    DiscardOutcome, DiscardedFile, FetchFailure, FetchOutcome, FetchRequest, GitBackend,
    HEADS_PREFIX, HeadState, LinePatch, LineSelection, LogQuery, MemoryBackend, MemoryOpener,
    RefMove, RefRestore, RepoOpener, RestoreOutcome, StagedVersion, StashedFile, WorktreeBackup,
    WorktreeRestore, WorktreeStash, amend_impact, apply_patch, backup_message, branch_ref,
    branch_short, check_amend_published, check_branch_name, check_commit_state,
    check_new_branch_name, check_no_operation, check_remote_name, check_same_plan, cleanup_message,
    default_remote, discard_description, discard_plan, file_digest, line_patch, path_matches,
    split_message, unknown_remote, valid_ref_name, versions_at_risk, versions_at_risk_hidden,
};
pub use git::{
    BlameQuery, FileBlame, FileLogQuery, blame_binary, blame_is_binary, blame_missing_at,
    blame_missing_worktree, blame_no_commits, is_binary_content,
};
pub use git::{
    ConflictResolution, ConflictResolved, ConflictVersions, DEFAULT_MARKER_SIZE, VersionText,
    check_conflict, check_digest, marker_size, worktree_digest,
};
pub use git::{
    INDEX_NOT_RESTORED, STASH_REF, StashApplied, StashApply, StashDrop, StashDropped, StashEntry,
    StashPopped, StashPush, StashRecord, check_stash_apply, check_stash_push, find_stash,
    pop_stash, stash_blocked, stash_branch, stash_local_changes,
};
pub use git::{
    MAX_REBASE_COMMITS, REBASE_RECORD, RebaseRecord, RebaseRequest, check_rebase_request,
    check_rebase_state, check_rebasing, explicit_squash_messages, plan_error, rebase_range,
    rewritten_commits,
};
pub use git::{
    MergeAborted, MergeMode, MergeOutcome, MergeRequest, MergeSource, MergeSourceKind,
    OperationState, check_merge_state, check_merging, clean_merge_message, conflicted_paths,
    merge_source,
};
pub use git::{
    PickKind, PickRequest, check_pick_state, check_picking, pick_empty_error, pick_message,
    pick_parent,
};
pub use git::{
    PullMode, PullOperation, PullOutcome, PullPolicy, PullRequest, PullSettings, PullStrategy,
    PushKind, PushOutcome, PushRejection, PushRequest, StrategySource, UpstreamConfig,
    check_pull_state, merge_message, pull_mode, tracking_ref,
};
pub use git::{
    REMOTES_PREFIX, RemoteRef, RemoteSnapshot, RemoteStillListed, check_new_remote_name,
    check_remote_url, ensure_remote_gone, mask_url, masked_entry, owned_by, remote_dependents,
    remove_remote, restore_remote, split_branch_key,
};
pub use job::{CancelToken, JobContext, Progress, ProgressSink};
pub use journal::{HeadChange, Journal, JournalEntry, RefChange};
pub use message::{Message, MessageArg, ToArg};
pub use platform::{Capabilities, Picked, Platform};
pub use project_core::Invocation;
pub use project_i18n::Lang;
pub use session::Session;

use project_core::CoreError;
use project_domain::{DomainError, Oid};

#[derive(Debug)]
pub enum EngineError {
    UnknownCommand(String),
    InvalidParams {
        id: &'static str,
        reason: Message,
    },
    /// There is no git repository at the path (or it cannot be opened).
    RepoNotFound {
        path: String,
        reason: Message,
    },
    /// The command needs a repo, but the session has none open.
    NoRepo,
    /// The rev matches no ref or commit (or the abbreviated id is ambiguous).
    UnknownRev(String),
    /// The `git` program does not start (not installed or not in the `PATH`).
    GitNotFound(Message),
    /// The installed `git` is older than the minimum supported version.
    GitTooOld {
        found: String,
        required: String,
    },
    /// A git command ended with an error; `stderr` is its message.
    Git {
        /// The git subcommand, e.g. `status`.
        command: String,
        stderr: String,
    },
    /// Like [`EngineError::Git`], but the text is an engine explanation that contains git's
    /// text (e.g. "undo applied the stash … but could not remove it from the list
    /// (…)"): `Display` is the same as `Git`, with `reason` in place of `stderr`.
    GitWithReason {
        command: String,
        reason: Message,
    },
    /// The index is locked by another git process: the `lock` file (usually
    /// `.git/index.lock`) exists. It is not retried and the lock is not deleted: if no git is
    /// running it was left over from a crash and must be removed by hand.
    IndexLocked {
        lock: String,
    },
    /// The operation is refused in the current state of the repo (e.g. stage of an ignored file,
    /// unstage of a conflicted file); nothing was modified.
    NotAllowed(Message),
    /// The diff the chosen lines come from (`index.stage_lines`, `index.unstage_lines`) is no
    /// longer the current one: the file changed since the diff was read (or no longer has
    /// changes). Nothing was modified; the diff must be re-read and the selection redone.
    StaleDiff {
        path: String,
        reason: Message,
    },
    /// `commit.create` with nothing to commit: the index equals HEAD (or, on a branch
    /// without commits, is empty) and there is no merge in progress. Nothing was modified.
    NothingToCommit,
    /// The operation can lose work or rewrites something already published and needs
    /// `"confirm": true` in the parameters (§1). `reason` says what would happen. Nothing was
    /// modified.
    ConfirmRequired {
        command: &'static str,
        reason: Message,
        /// The full id (40 digits) to repeat in `"expected"` to confirm: for the force of
        /// `remote.push` (the local remote-tracking branch read now) and for `stash.drop` (the stash to
        /// delete), which is also in the text of `reason`; `None` for the other commands.
        expected: Option<String>,
    },
    /// The commit **succeeded**, but afterwards something does not add up: HEAD moved by a
    /// `post-commit` hook or by another process, `previous` not verifiable, state not re-readable.
    /// It must not be repeated: `commit` is the created commit (if known), `backup` the ref of the
    /// commit rewritten by `amend` (if any), which remains.
    PostCommit {
        commit: Option<Oid>,
        backup: Option<String>,
        reason: Message,
    },
    /// `edit.undo`/`edit.redo`: HEAD or the refs are no longer as the operation to undo
    /// (or the undo to redo) left them: someone moved them from outside (a `git commit` or
    /// `git reset` from a terminal, another program). Nothing was modified and the entry
    /// stays in the journal; `refs` says what changed, one line per ref.
    UndoConflict {
        command: &'static str,
        refs: Vec<Message>,
    },
    /// `branch.checkout` (or the undo/redo of a checkout) refused by git because it would overwrite
    /// local changes: modified files (staged or not) that differ in the target branch, or
    /// untracked or ignored files that the target branch contains. Nothing was
    /// modified (git checks before writing); `files` are the files named by git,
    /// `stderr` its message. The changes must be committed, stashed or discarded
    /// before the checkout.
    CheckoutConflict {
        /// The branch (short name) or the target commit.
        target: String,
        files: Vec<String>,
        stderr: String,
    },
    /// The remote rejected the credentials, or they would have to be asked for (user name, password,
    /// passphrase, host key confirmation) and there is no way to do so without a terminal:
    /// "Authentication failed", "could not read Username", "Permission denied (publickey)",
    /// HTTP 401/403, "Host key verification failed". Nothing was modified. `message` is git's
    /// text (stderr).
    Auth {
        remote: String,
        message: String,
    },
    /// The remote cannot be reached: the URL points to a folder that does not exist or is not a
    /// repository, unknown host, connection refused or interrupted, repository nonexistent
    /// on the server. Nothing was modified. `message` is git's text (stderr).
    RemoteUnreachable {
        remote: String,
        message: String,
    },
    /// The command was cancelled by whoever launched it
    /// ([`CancelToken::cancel`](crate::CancelToken::cancel)). What the command says about itself
    /// applies here too (e.g. `remote.fetch` cancelled: the refs already updated stay, no
    /// local ref touched).
    Cancelled {
        command: &'static str,
    },
    /// `remote.pull`: the merge or rebase onto the upstream stopped on a conflict and was aborted:
    /// branch, index and worktree are as before the pull, the fetch stays done. `files` are the
    /// files in conflict, `message` git's text. No backend produces it any more (since S32 a
    /// stopped merge or rebase stays in progress,
    /// [`PullOutcome::in_progress`](crate::PullOutcome::in_progress)); it stays for compatibility.
    PullConflict {
        branch: String,
        /// The remote branch (short name, e.g. `origin/main`).
        upstream: String,
        operation: PullOperation,
        files: Vec<String>,
        message: String,
    },
    /// `remote.push` rejected: nothing changed, neither on the remote nor locally (the local
    /// remote-tracking branch stays as it was). `branch` is the branch on the remote, `message` git's text.
    PushRejected {
        remote: String,
        branch: String,
        reason: PushRejection,
        message: String,
    },
    Core(CoreError),
    Domain(DomainError),
}

impl EngineError {
    /// [`EngineError::NotAllowed`] for the stage of ignored files requested by name: nothing was
    /// staged.
    pub fn ignored(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(msg!("engine-ignored", files = quoted_list(paths)))
    }

    /// [`EngineError::NotAllowed`] for the unstage of conflicted files: nothing was removed
    /// from the stage.
    pub fn unmerged(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(msg!("engine-unmerged", files = quoted_list(paths)))
    }

    /// [`EngineError::NotAllowed`] for the stage of a folder with conflicted files: a
    /// conflict is resolved only by asking for the file by name. Nothing was staged.
    pub fn unmerged_in_folder(paths: &[impl std::fmt::Display]) -> Self {
        EngineError::NotAllowed(msg!(
            "engine-unmerged-in-folder",
            files = quoted_list(paths)
        ))
    }

    /// [`EngineError::InvalidParams`] of `commit.create` for a message that git's cleanup
    /// (`commit.cleanup`, e.g. `strip` which removes comments) made empty: nothing committed.
    pub fn empty_commit_message() -> Self {
        EngineError::InvalidParams {
            id: "commit.create",
            reason: msg!("engine-empty-commit-message"),
        }
    }

    /// The error text as key and parameters: git's text (`stderr`), paths and
    /// names stay parameters as they were.
    pub fn message(&self) -> Message {
        use EngineError as E;
        match self {
            E::UnknownCommand(id) => msg!("engine-unknown-command", id = id),
            E::InvalidParams { id, reason } => {
                msg!("engine-invalid-params", id = *id, reason = reason)
            }
            E::RepoNotFound { path, reason } => {
                msg!("engine-repo-not-found", path = path, reason = reason)
            }
            E::NoRepo => msg!("engine-no-repo"),
            E::UnknownRev(rev) => msg!("engine-unknown-rev", rev = rev),
            E::GitNotFound(error) => msg!("engine-git-not-found", error = error),
            E::GitTooOld { found, required } => {
                msg!("engine-git-too-old", found = found, required = required)
            }
            E::Git { command, stderr } => msg!("engine-git", command = command, stderr = stderr),
            E::GitWithReason { command, reason } => {
                msg!("engine-git", command = command, stderr = reason)
            }
            E::IndexLocked { lock } => msg!("engine-index-locked", lock = lock),
            E::NotAllowed(reason) => msg!("engine-not-allowed", reason = reason),
            E::StaleDiff { path, reason } => {
                msg!("engine-stale-diff", path = path, reason = reason)
            }
            E::NothingToCommit => msg!("engine-nothing-to-commit"),
            E::ConfirmRequired {
                command, reason, ..
            } => msg!(
                "engine-confirm-required",
                command = *command,
                reason = reason
            ),
            E::PostCommit {
                commit,
                backup,
                reason,
            } => match (commit, backup) {
                (Some(id), Some(backup)) => msg!(
                    "engine-post-commit-id-backup",
                    id = id,
                    reason = reason,
                    backup = backup
                ),
                (Some(id), None) => msg!("engine-post-commit-id", id = id, reason = reason),
                (None, Some(backup)) => {
                    msg!(
                        "engine-post-commit-backup",
                        reason = reason,
                        backup = backup
                    )
                }
                (None, None) => msg!("engine-post-commit", reason = reason),
            },
            E::UndoConflict { command, refs } => msg!(
                "engine-undo-conflict",
                command = *command,
                refs = MessageArg::Joined(refs.clone(), "; ".into())
            ),
            E::CheckoutConflict { target, files, .. } => {
                msg!(
                    "engine-checkout-conflict",
                    target = target,
                    files = quoted_list(files)
                )
            }
            E::Auth { remote, message } => {
                msg!("engine-auth", remote = remote, message = message)
            }
            E::RemoteUnreachable { remote, message } => {
                msg!(
                    "engine-remote-unreachable",
                    remote = remote,
                    message = message
                )
            }
            E::Cancelled { command } => msg!("engine-cancelled", command = *command),
            E::PullConflict {
                branch,
                upstream,
                operation,
                files,
                ..
            } => msg!(
                "engine-pull-conflict",
                branch = branch,
                upstream = upstream,
                operation = operation.to_string(),
                files = quoted_list(files)
            ),
            E::PushRejected {
                remote,
                branch,
                reason,
                message,
            } => msg!(
                "engine-push-rejected",
                branch = branch,
                remote = remote,
                reason = reason.advice(),
                message = message
            ),
            E::Core(e) => msg!("engine-other", text = e.to_string()),
            E::Domain(e) => msg!("engine-other", text = e.to_string()),
        }
    }

    /// The text in `lang` (the UI); `Display` is the English text (CLI, MCP, tests).
    pub fn text_in(&self, lang: Lang) -> String {
        self.message().text_in(lang)
    }

    /// The text in the UI's active language.
    pub fn text(&self) -> String {
        self.message().text()
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text_in(Lang::En))
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            EngineError::Core(e) => Some(e),
            EngineError::Domain(e) => Some(e),
            _ => None,
        }
    }
}

impl From<CoreError> for EngineError {
    fn from(e: CoreError) -> Self {
        EngineError::Core(e)
    }
}

impl From<DomainError> for EngineError {
    fn from(e: DomainError) -> Self {
        EngineError::Domain(e)
    }
}

/// The paths in backticks, separated by commas: `` `a`, `b` ``.
pub fn quoted_list(paths: &[impl std::fmt::Display]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// An error inside the text of another (e.g. "the state cannot be re-read (…)"): translated into the
/// same language.
impl ToArg for EngineError {
    fn to_arg(&self) -> MessageArg {
        self.message().to_arg()
    }
}
