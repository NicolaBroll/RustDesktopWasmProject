//! Reset of the current branch (S45): `branch.reset` and its read-only preview
//! `branch.reset_preview`. Types and checks shared by the backends in `git/reset.rs`; contract in
//! `impl-docs/git/reset.md`.

use super::{JournalState, Out, reread, to_json};
use crate::{
    EngineError, GitBackend, HeadState, JournalEntry, LogQuery, Message, RefMove, RefRestore,
    ResetMode, ResetRequest, Session, branch_short, check_no_operation, reset_files,
};
use project_domain::remote_short;
use project_domain::{Head, Oid, RefInfo, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// Result of `branch.reset_preview`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResetPreviewResult {
    /// The mode, as given.
    pub mode: ResetMode,
    /// Full name of the branch of HEAD; `null` with detached HEAD.
    pub branch: Option<String>,
    /// The commit HEAD is on.
    pub before: Oid,
    /// The commit the reset would move to.
    pub after: Oid,
    /// `hard`: the files whose local changes would be overwritten (empty for `soft`/`mixed`).
    pub files: Vec<RepoPath>,
    /// `branch.reset` needs `"confirm": true`.
    pub needs_confirm: bool,
    /// The value to repeat in the `expected` of `branch.reset` (the commit HEAD is on).
    pub expected: Oid,
    /// How many commits leaving the branch are already on its upstream (0 if none).
    pub published: usize,
    /// The upstream (full name) when `published` is not 0; `null` otherwise.
    pub upstream: Option<String>,
    /// Warnings (commits leaving the branch already published).
    pub warnings: Vec<Message>,
}

/// Result of `branch.reset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResetResult {
    /// The mode, as given.
    pub mode: ResetMode,
    /// Full name of the branch moved; `null` with detached HEAD.
    pub branch: Option<String>,
    /// The commit before the reset.
    pub before: Oid,
    /// The commit after the reset.
    pub after: Oid,
    /// `hard`: the files whose local changes were overwritten (and saved in `backup`).
    pub files: Vec<RepoPath>,
    /// `hard` with local changes: the backup ref `refs/project/backup/reset-...`; `null`
    /// otherwise.
    pub backup: Option<String>,
    /// The commit of `git stash create` with those changes; `null` without a backup.
    pub stash: Option<Oid>,
    /// `mixed`: the ref `refs/project/backup/index-...` with the staged versions that existed
    /// only in the index; `null` otherwise.
    pub index_backup: Option<String>,
    /// The backup refs of the commits that are no longer reachable.
    pub backups: Vec<String>,
    /// How many commits leaving the branch are already on its upstream (0 if none).
    pub published: usize,
    /// Warnings after the write and about published commits.
    pub warnings: Vec<Message>,
    /// The status re-read after the reset; `null` if it could not be re-read (see
    /// `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The refs re-read after the reset; `null` if they could not be re-read (see
    /// `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Parameters of `branch.reset`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetParams {
    /// The commit: an id (also abbreviated), a branch, a tag or another rev.
    pub target: String,
    /// What the reset moves besides the branch.
    pub mode: ResetMode,
    /// A `hard` reset that overwrites local changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
    /// The full id of the commit HEAD was on when the reset was confirmed (the `expected` of the
    /// confirmation request): HEAD must still be there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
}

/// Parameters of `branch.reset_preview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResetPreviewParams {
    /// The commit, as in `branch.reset`.
    pub target: String,
    /// The mode, as in `branch.reset`.
    pub mode: ResetMode,
}

/// What a reset would do, read before writing: shared by the command and its preview.
struct Plan {
    /// Full name of the branch of HEAD, `None` with detached HEAD.
    branch: Option<String>,
    before: Oid,
    after: Oid,
    /// `hard`: the files whose local changes would be overwritten.
    files: Vec<RepoPath>,
    /// Upstream (full name) and how many of the commits leaving the branch are already there.
    published: Option<(String, usize)>,
}

impl Plan {
    fn needs_confirm(&self, mode: ResetMode) -> bool {
        mode == ResetMode::Hard && !self.files.is_empty()
    }
}

fn plan(
    id: &'static str,
    repo: &dyn GitBackend,
    status: &Status,
    target: &str,
    mode: ResetMode,
) -> Result<Plan, EngineError> {
    if target.trim().is_empty() || target.contains('\0') {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-reset-target-empty"),
        });
    }
    check_no_operation(id, msg!("engine-busy-reset"), status.operation)?;
    let (branch, before) = match &status.head {
        Head::Branch {
            target: Some(t), ..
        } => (Some(full_name(HeadState::from_head(&status.head))), *t),
        Head::Branch { name, target: None } => {
            return Err(EngineError::NotAllowed(msg!(
                "engine-reset-unborn",
                name = name.escape_debug()
            )));
        }
        Head::Detached(t) => (None, *t),
    };
    let after = repo
        .log(&LogQuery::page(0, 1).from_rev(target.to_owned()))?
        .into_iter()
        .next()
        .ok_or_else(|| EngineError::UnknownRev(target.to_owned()))?
        .id;
    let files = match mode {
        ResetMode::Hard => reset_files(status)?,
        _ => Vec::new(),
    };
    let published = match &branch {
        Some(b) => published(repo, b, before, after)?,
        None => None,
    };
    Ok(Plan {
        branch,
        before,
        after,
        files,
        published,
    })
}

fn full_name(head: HeadState) -> String {
    match head {
        HeadState::Branch(b) => b,
        HeadState::Detached(oid) => oid.to_string(),
    }
}

/// The upstream of `branch` (its local remote branch) and how many commits reachable from
/// `before` but not from `after` are already there; `None` if there is no upstream or none.
fn published(
    repo: &dyn GitBackend,
    branch: &str,
    before: Oid,
    after: Oid,
) -> Result<Option<(String, usize)>, EngineError> {
    let Some(tracking) = repo
        .upstream(branch_short(branch))?
        .and_then(|u| u.tracking)
    else {
        return Ok(None);
    };
    let Some(up) = repo
        .refs()?
        .into_iter()
        .find(|r| r.name == tracking)
        .map(|r| r.target)
    else {
        return Ok(None);
    };
    let leaving = repo.unmerged_count(&before, &[after])?;
    if leaving == 0 {
        return Ok(None);
    }
    let unpublished = repo.unmerged_count(&before, &[after, up])?;
    let n = leaving.saturating_sub(unpublished);
    Ok((n > 0).then_some((tracking, n)))
}

fn published_warning(plan: &Plan) -> Option<Message> {
    let (upstream, n) = plan.published.as_ref()?;
    Some(msg!(
        "engine-reset-published",
        n = *n,
        branch = branch_short(plan.branch.as_deref().unwrap_or_default()),
        upstream = remote_short(upstream)
    ))
}

fn file_list(files: &[RepoPath]) -> String {
    files
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a `branch.reset` with the same parameters would do, without writing anything (the
/// same refusals as the command). For the confirmation dialog: `files` are the files whose
/// local changes a `hard` overwrites, `needs_confirm` says whether the command needs `"confirm":
/// true`, `expected` is the value to repeat in the command's `expected`.
///
/// Result: [`ResetPreviewResult`].
pub(super) fn branch_reset_preview(
    s: &mut Session,
    ResetPreviewParams { target, mode }: ResetPreviewParams,
) -> Out {
    const ID: &str = "branch.reset_preview";
    let repo = s.repo()?;
    let status = repo.status()?;
    let plan = plan(ID, repo, &status, &target, mode)?;
    let warnings = published_warning(&plan).into_iter().collect();
    Ok(to_json(ResetPreviewResult {
        mode,
        needs_confirm: plan.needs_confirm(mode),
        expected: plan.before,
        published: plan.published.as_ref().map_or(0, |(_, n)| *n),
        upstream: plan.published.map(|(u, _)| u),
        branch: plan.branch,
        before: plan.before,
        after: plan.after,
        files: plan.files,
        warnings,
    }))
}

/// Moves the branch of HEAD (or a detached HEAD) to `target`, like `git reset --<mode>` (see
/// [`GitBackend::reset`] and `impl-docs/git/reset.md`): `soft` only the
/// ref, `mixed` also the index (the staged versions that exist only there first saved in
/// `refs/project/backup/index-...`), `hard` also the worktree of the tracked files. Untracked
/// and ignored files are never touched.
///
/// Refused ([`EngineError::NotAllowed`], nothing changes): operation in progress, branch without
/// commits, `intent-to-add` files (`hard`), an untracked or ignored file where the target puts a
/// file (`hard`), HEAD no longer on `expected`; a rev that does not exist or is not a commit →
/// [`EngineError::UnknownRev`] or a git error. A `hard` with local changes needs `"confirm":
/// true`: without it, [`EngineError::ConfirmRequired`] with the files and HEAD's commit in
/// `expected` (`branch.reset_preview` gives the same list as data). Before writing it saves them
/// in `refs/project/backup/reset-...` (`git stash create` + byte-for-byte copy): if that fails,
/// nothing is written.
///
/// It enters the journal when the commit changes: `edit.undo` moves the ref back with index and
/// worktree for `mixed`/`hard` (only if nothing would need a new backup); the local changes of a
/// `hard` stay in the backup, and the undo says so. `warnings` says when commits leaving the branch
/// are already on its upstream: the next push will need `--force-with-lease`.
///
/// Result: [`ResetResult`].
pub(super) fn branch_reset(
    s: &mut Session,
    ResetParams {
        target,
        mode,
        confirm,
        expected,
    }: ResetParams,
) -> Out {
    const ID: &str = "branch.reset";
    let expected = expected
        .map(|hex| {
            let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
            full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
                .flatten()
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: msg!(
                        "engine-reset-expected-hex",
                        len = Oid::HEX_LEN,
                        value = hex.escape_debug()
                    ),
                })
        })
        .transpose()?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let plan = plan(ID, repo, &status, &target, mode)?;
    if let Some(seen) = expected
        && seen != plan.before
    {
        return Err(EngineError::NotAllowed(msg!(
            "engine-reset-head-moved",
            seen = seen,
            now = plan.before
        )));
    }
    if plan.needs_confirm(mode) && !confirm {
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-reset-confirm",
                n = plan.files.len(),
                files = file_list(&plan.files),
                after = plan.after.short()
            ),
            expected: Some(plan.before.to_string()),
        });
    }
    let head = HeadState::from_head(&status.head);
    let head_target = match &plan.branch {
        Some(_) => head.clone(),
        None => HeadState::Detached(plan.after),
    };
    let request = ResetRequest {
        mode,
        refs: RefRestore {
            command: ID,
            head_expected: head,
            head_target,
            refs: plan
                .branch
                .iter()
                .map(|b| RefMove {
                    name: b.clone(),
                    expected: Some(plan.before),
                    target: Some(plan.after),
                })
                .collect(),
            reflog: format!(
                "project: reset ({}) to {} ({target})",
                mode.as_str(),
                plan.after
            ),
        },
        files: plan.files.clone(),
        strict: false,
    };
    let outcome = repo.reset(&request)?;
    // Only a move of the commit enters the journal (a `hard` on HEAD itself is a discard of
    // everything: its recovery is the backup).
    if plan.before != plan.after {
        journal.record(JournalEntry::reset(
            plan.branch.as_deref(),
            plan.before,
            plan.after,
            mode,
            outcome.backup.clone(),
            outcome.stash,
        ));
    }
    let mut warnings = outcome.warnings;
    warnings.extend(published_warning(&plan));
    let (status, status_error) = reread(repo.status());
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(ResetResult {
        mode,
        branch: plan.branch,
        before: plan.before,
        after: plan.after,
        files: outcome.files,
        backup: outcome.backup,
        stash: outcome.stash,
        index_backup: outcome.index_backup,
        backups: outcome.backups,
        published: plan.published.as_ref().map_or(0, |(_, n)| *n),
        warnings,
        status,
        status_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;
