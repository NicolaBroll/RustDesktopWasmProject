//! Reset of the current branch (S45): `branch.reset` and its read-only preview
//! `branch.reset_preview`. Types and checks shared by the backends in `git/reset.rs`; contract in
//! `impl-docs/git/reset.md`.

use super::Out;
use super::merge::finish;
use crate::commands::parse_params;
use crate::{
    EngineError, GitBackend, HeadState, JournalEntry, LogQuery, Message, RefMove, RefRestore,
    ResetMode, ResetRequest, Session, branch_short, check_no_operation, reset_files,
};
use project_domain::remote_short;
use project_domain::{Head, Oid, RepoPath, Status};
use serde::Deserialize;
use serde_json::{Value, json};

/// Parameters of `branch.reset`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResetParams {
    /// The commit: an id (also abbreviated), a branch, a tag or another rev.
    target: String,
    mode: ResetMode,
    #[serde(default)]
    confirm: bool,
    /// The full id of the commit HEAD was on when the reset was confirmed (the `expected` of the
    /// confirmation request): HEAD must still be there.
    #[serde(default)]
    expected: Option<String>,
}

/// Parameters of `branch.reset_preview`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewParams {
    target: String,
    mode: ResetMode,
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
/// Result: `{"mode", "branch": string | null, "before", "after", "files": [path],
/// "needs_confirm", "expected", "published": n, "upstream": string | null, "warnings":
/// [Message]}`.
pub(super) fn branch_reset_preview(s: &mut Session, p: Value) -> Out {
    const ID: &str = "branch.reset_preview";
    let PreviewParams { target, mode } = parse_params(ID, p)?;
    let repo = s.repo()?;
    let status = repo.status()?;
    let plan = plan(ID, repo, &status, &target, mode)?;
    Ok(json!({
        "mode": mode,
        "branch": plan.branch,
        "before": plan.before.to_string(),
        "after": plan.after.to_string(),
        "files": plan.files,
        "needs_confirm": plan.needs_confirm(mode),
        "expected": plan.before.to_string(),
        "published": plan.published.as_ref().map_or(0, |(_, n)| *n),
        "upstream": plan.published.as_ref().map(|(u, _)| u.clone()),
        "warnings": published_warning(&plan).into_iter().collect::<Vec<_>>(),
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
/// Result: `{"mode", "branch": string | null, "before", "after", "files": [path], "backup":
/// string | null, "stash": oid | null, "index_backup": string | null, "backups": [string],
/// "published": n, "warnings": [Message], "status", "refs", "can_undo", "can_redo",
/// "next_undo", "next_redo"}`.
pub(super) fn branch_reset(s: &mut Session, p: Value) -> Out {
    const ID: &str = "branch.reset";
    let ResetParams {
        target,
        mode,
        confirm,
        expected,
    } = parse_params(ID, p)?;
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
    let mut warnings = outcome.warnings.clone();
    warnings.extend(published_warning(&plan));
    let mut out = json!({
        "mode": mode,
        "branch": plan.branch,
        "before": plan.before.to_string(),
        "after": plan.after.to_string(),
        "files": outcome.files,
        "backup": outcome.backup,
        "stash": outcome.stash.map(|o| o.to_string()),
        "index_backup": outcome.index_backup,
        "backups": outcome.backups,
        "published": plan.published.as_ref().map_or(0, |(_, n)| *n),
        "warnings": warnings,
    });
    finish(&mut out, repo, journal);
    Ok(out)
}

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;
