//! Commands of the local branches (S24): `branch.create`, `branch.checkout`, `branch.delete`,
//! `branch.rename`. The checks that do not depend on the backend are here; writes and checks
//! under lock are in the backends ([`GitBackend`](crate::GitBackend)). Every successful command that changes
//! something enters the journal: `edit.undo` undoes it (§7).

use super::{JournalState, Out, reread, to_json};
use crate::{
    ConfigEntry, EngineError, HeadChange, HeadState, JournalEntry, LogQuery, Message, RefMove,
    RefRestore, Session, TrackingBranch, UpstreamChange, branch_ref, branch_short,
    check_branch_name, check_new_branch_name, check_no_operation, resolve_tracking, upstream_label,
};
use project_domain::{Head, Oid, RefInfo, Status};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters of `branch.create`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchCreateParams {
    /// The short name of the new branch.
    pub name: String,
    /// Where the branch starts (rev); without: HEAD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    /// Take `start` (a remote-tracking branch `<remote>/<branch>` of a configured remote) as
    /// upstream, like `git branch --track`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub track: bool,
}

/// Parameters of `branch.set_upstream`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchSetUpstreamParams {
    /// The short name of the local branch.
    pub name: String,
    /// The new upstream, a remote-tracking branch `<remote>/<branch>` (or
    /// `refs/remotes/<remote>/<branch>`); `null` removes it. Required: a missing field is an error,
    /// so a forgotten parameter never removes the upstream.
    #[serde(deserialize_with = "required_nullable")]
    #[schemars(required)]
    pub upstream: Option<String>,
}

/// An `Option` field that must be present in the JSON, possibly `null`.
fn required_nullable<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(d)
}

/// Parameters of `branch.checkout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchCheckoutParams {
    /// The short name of the branch.
    pub name: String,
}

/// Parameters of `branch.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchDeleteParams {
    /// The short name of the branch.
    pub name: String,
    /// Delete also an unmerged branch.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
    /// The full id of the commit on which the caller saw the branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
}

/// Parameters of `branch.rename`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BranchRenameParams {
    /// The current short name.
    pub name: String,
    /// The new short name.
    pub new_name: String,
}

/// Result of `branch.create`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchCreateResult {
    /// The full name of the new branch, `refs/heads/<name>`.
    pub branch: String,
    /// The commit the branch points to.
    pub target: Oid,
    /// With `track`: the upstream written, `<remote>/<branch>`; otherwise `null`.
    pub upstream: Option<String>,
    /// Something unexpected found after the successful creation (usually empty).
    #[serde(default)]
    pub warnings: Vec<Message>,
    /// The refs re-read after the creation; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `branch.checkout`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchCheckoutResult {
    /// The full name of the branch now active, `refs/heads/<name>`.
    pub branch: String,
    /// HEAD before the command.
    pub previous: HeadState,
    /// `false` if the branch was already active (nothing done, nothing in the journal).
    pub changed: bool,
    /// Something unexpected found after the successful checkout (usually empty).
    #[serde(default)]
    pub warnings: Vec<Message>,
    /// The state re-read after the checkout; `null` if it could not be re-read (see
    /// `status_error`).
    pub status: Option<Status>,
    /// Why the state could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `branch.delete`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchDeleteResult {
    /// The full name of the deleted branch, `refs/heads/<name>`.
    pub branch: String,
    /// The commit the branch pointed to.
    pub target: Oid,
    /// The commits of the branch that were neither in HEAD nor in its upstream.
    pub unmerged: usize,
    /// The backup ref that keeps the commit (`refs/project/backup/delete-...`).
    pub backup: String,
    /// The `branch.<name>.*` configuration removed with the branch (upstream, description):
    /// `edit.undo` puts it back.
    #[serde(default)]
    pub removed_config: Vec<ConfigEntry>,
    /// Something unexpected found after the successful deletion (usually empty).
    #[serde(default)]
    pub warnings: Vec<Message>,
    /// The refs re-read after the deletion; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `branch.rename`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchRenameResult {
    /// The new full name, `refs/heads/<new_name>`.
    pub branch: String,
    /// The old full name, `refs/heads/<name>`.
    pub previous: String,
    /// The commit the branch points to.
    pub target: Oid,
    /// The renamed branch was the active one (HEAD follows it).
    pub head: bool,
    /// Something unexpected found after the successful rename (usually empty).
    #[serde(default)]
    pub warnings: Vec<Message>,
    /// The refs re-read after the rename; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `branch.set_upstream`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchSetUpstreamResult {
    /// The full name of the branch, `refs/heads/<name>`.
    pub branch: String,
    /// The upstream now, `<remote>/<branch>`; `null` if removed.
    pub upstream: Option<String>,
    /// The upstream before (of the repo file), `<remote>/<branch>`; `null` if there was none.
    pub previous: Option<String>,
    /// `false` if the upstream was already that one (nothing written, nothing in the journal).
    pub changed: bool,
    /// Something unexpected found after the successful write (usually empty).
    #[serde(default)]
    pub warnings: Vec<Message>,
    /// The refs re-read after the command (with the new upstream); `null` if they could not be
    /// re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// The local branch `full` among the refs, `None` if there is none.
fn find<'a>(refs: &'a [RefInfo], full: &str) -> Option<&'a RefInfo> {
    refs.iter().find(|r| r.name == full)
}

fn missing(name: &str) -> EngineError {
    EngineError::NotAllowed(msg!("engine-branch-no-local", name = name.escape_debug()))
}

fn taken(name: &str) -> EngineError {
    EngineError::NotAllowed(msg!("engine-branch-exists", name = name.escape_debug()))
}

/// Creates the branch `name` at `start` (a rev: id, also abbreviated, branch or tag name;
/// HEAD if missing), without switching branch (see
/// [`GitRefs::create_branch`](crate::GitRefs::create_branch)). An existing branch is never
/// overwritten. The operation goes into the journal: `edit.undo` deletes the branch, only if in the
/// meantime it was not moved.
///
/// Result: [`BranchCreateResult`].
pub(super) fn branch_create(
    s: &mut Session,
    BranchCreateParams { name, start, track }: BranchCreateParams,
) -> Out {
    const ID: &str = "branch.create";
    check_new_branch_name(ID, &name)?;
    let (repo, journal) = s.repo_and_journal()?;
    let head = repo.head()?;
    let refs = repo.refs()?;
    let tracking = match (track, &start) {
        (false, _) => None,
        (true, None) => {
            return Err(EngineError::InvalidParams {
                id: ID,
                reason: msg!("engine-branch-track-no-start"),
            });
        }
        (true, Some(rev)) => Some(tracked_start(ID, rev, &refs, &repo.remotes()?)?),
    };
    let target: Oid = match (&tracking, &start) {
        (Some(t), _) => t.target,
        (None, Some(rev)) => {
            let commits = repo.log(&LogQuery::page(0, 1).from_rev(rev.clone()))?;
            commits
                .first()
                .map(|c| c.id)
                .ok_or_else(|| EngineError::UnknownRev(rev.clone()))?
        }
        (None, None) => head
            .target()
            .ok_or_else(|| EngineError::NotAllowed(msg!("engine-branch-unborn-start")))?,
    };
    let full = branch_ref(&name);
    if find(&refs, &full).is_some() {
        return Err(taken(&name));
    }
    // The upstream entries the repo file has now (a leftover of a branch deleted with
    // `update-ref`): undo puts them back.
    let change = match &tracking {
        Some(t) => Some(UpstreamChange {
            branch: name.clone(),
            before: repo.local_upstream(&name)?,
            after: t.entries(&name),
        }),
        None => None,
    };
    let head = HeadState::from_head(&head);
    repo.create_branch(&name, &target)?;
    let mut warnings = Vec::new();
    if let Some(change) = &change {
        match repo.write_upstream(&name, &change.before, &change.after) {
            Ok(w) => warnings = w,
            Err(e) => return Err(roll_back_create(repo, &full, target, &head, e)),
        }
    }
    let upstream = tracking.as_ref().map(TrackingBranch::label);
    journal.record(match change {
        Some(change) => JournalEntry::branch_create_tracking(&full, target, head, change),
        None => JournalEntry::branch_create(&full, target, head),
    });
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(BranchCreateResult {
        branch: full,
        target,
        upstream,
        warnings,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

/// The remote-tracking branch `rev` that `branch.create` with `track` starts from
/// ([`resolve_tracking`]). A short `rev` that git would resolve to a tag or a local branch first
/// (`refs/tags/<rev>`, `refs/heads/<rev>`) is ambiguous: [`EngineError::InvalidParams`].
fn tracked_start(
    id: &'static str,
    rev: &str,
    refs: &[RefInfo],
    remotes: &[String],
) -> Result<TrackingBranch, EngineError> {
    let remote_ref = format!("{}{rev}", crate::REMOTES_PREFIX);
    if !rev.starts_with("refs/") && find(refs, &remote_ref).is_some() {
        let shadow = [crate::TAGS_PREFIX, crate::HEADS_PREFIX]
            .iter()
            .map(|prefix| format!("{prefix}{rev}"))
            .find(|full| find(refs, full).is_some());
        if let Some(shadow) = shadow {
            return Err(EngineError::InvalidParams {
                id,
                reason: msg!(
                    "engine-branch-track-ambiguous",
                    start = rev.escape_debug(),
                    other = shadow
                ),
            });
        }
    }
    resolve_tracking(id, rev, refs, remotes)
}

/// After the upstream of a new tracking branch could not be written (`error`): the branch just
/// created is deleted again (only if it is still at `target`), so nothing is left half-done.
/// The error says whether the deletion succeeded.
fn roll_back_create(
    repo: &mut dyn crate::GitBackend,
    full: &str,
    target: Oid,
    head: &HeadState,
    error: EngineError,
) -> EngineError {
    let request = RefRestore {
        command: "branch.create",
        head_expected: head.clone(),
        head_target: head.clone(),
        refs: vec![RefMove {
            name: full.to_owned(),
            expected: Some(target),
            target: None,
        }],
        reflog: format!("project: branch.create {full} rolled back"),
    };
    let reason = match repo.restore_refs(&request) {
        Ok(_) => msg!(
            "engine-branch-track-rolled-back",
            name = branch_short(full).escape_debug(),
            error = error
        ),
        Err(e) => msg!(
            "engine-branch-track-not-rolled-back",
            name = branch_short(full).escape_debug(),
            error = error,
            rollback = e
        ),
    };
    EngineError::GitWithReason {
        command: "config".to_owned(),
        reason,
    }
}

/// Sets or removes the upstream of the local branch `name` (see
/// [`GitRefs::write_upstream`](crate::GitRefs::write_upstream)): `upstream` must be an existing
/// remote-tracking branch of a configured remote ([`resolve_tracking`]); `null` removes it. Only
/// `branch.<name>.remote` and `branch.<name>.merge` of the repo file are written (`git config
/// --local`); other keys of the branch stay. Allowed with detached HEAD and during an operation:
/// the configuration does not touch refs, index or worktree. The same upstream: nothing
/// changes (`changed: false`). It goes into the journal: `edit.undo` puts the previous upstream
/// back, only if the configuration is still the one written ([`EngineError::UndoConflict`]
/// otherwise).
///
/// Result: [`BranchSetUpstreamResult`].
pub(super) fn branch_set_upstream(
    s: &mut Session,
    BranchSetUpstreamParams { name, upstream }: BranchSetUpstreamParams,
) -> Out {
    const ID: &str = "branch.set_upstream";
    check_branch_name(ID, &name)?;
    if upstream.as_deref() == Some("") {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-upstream-empty"),
        });
    }
    let (repo, journal) = s.repo_and_journal()?;
    let refs = repo.refs()?;
    let full = branch_ref(&name);
    if find(&refs, &full).is_none() {
        return Err(missing(&name));
    }
    let tracking = match &upstream {
        Some(spec) => Some(resolve_tracking(ID, spec, &refs, &repo.remotes()?)?),
        None => None,
    };
    let before = repo.local_upstream(&name)?;
    let after = tracking
        .as_ref()
        .map(|t| t.entries(&name))
        .unwrap_or_default();
    let previous = upstream_label(&before);
    let changed = before != after;
    let mut warnings = Vec::new();
    if changed {
        // HEAD read before writing: an error after the write would leave a change without an undo.
        let head = HeadState::from_head(&repo.head()?);
        warnings = repo.write_upstream(&name, &before, &after)?;
        journal.record(JournalEntry::set_upstream(
            &full,
            head,
            UpstreamChange {
                branch: name.clone(),
                before,
                after,
            },
        ));
    }
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(BranchSetUpstreamResult {
        branch: full,
        upstream: tracking.as_ref().map(TrackingBranch::label),
        previous,
        changed,
        warnings,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

/// Switches to the local branch `name` (`git switch`, see
/// [`GitRefs::checkout`](crate::GitRefs::checkout)): index and worktree become those
/// of the branch, local changes that do not touch the differing files stay. It never forces: if
/// a local change (or an untracked or ignored file) would be overwritten,
/// [`EngineError::CheckoutConflict`] with the files and nothing changes. Refused
/// ([`EngineError::NotAllowed`]) during merge, rebase, cherry-pick, revert, bisect or `git am`,
/// and with conflicted files. The branch already active: no change (`changed: false`), nothing
/// in the journal.
///
/// It goes into the journal (except when leaving a branch without commits, where going back would mean
/// emptying the index): `edit.undo` returns to the previous HEAD (branch or detached commit) with another
/// equally safe checkout, only if HEAD is still on the branch.
///
/// Result: [`BranchCheckoutResult`].
pub(super) fn branch_checkout(
    s: &mut Session,
    BranchCheckoutParams { name }: BranchCheckoutParams,
) -> Out {
    const ID: &str = "branch.checkout";
    check_branch_name(ID, &name)?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_no_operation(ID, msg!("engine-busy-checkout"), status.operation)?;
    let full = branch_ref(&name);
    if find(&repo.refs()?, &full).is_none() {
        return Err(missing(&name));
    }
    let before = HeadState::from_head(&status.head);
    let target = HeadState::Branch(full.clone());
    let mut warnings = Vec::new();
    let changed = before != target;
    if changed {
        let outcome = repo.checkout(&target)?;
        warnings = outcome.warnings;
        // There is no going back from a branch without commits without emptying the index: no entry.
        let unborn = matches!(status.head, Head::Branch { target: None, .. });
        if !unborn {
            journal.record(JournalEntry::checkout(before.clone(), target));
        }
    }
    let (status, status_error) = reread(repo.status());
    Ok(to_json(BranchCheckoutResult {
        branch: full,
        previous: before,
        changed,
        warnings,
        status,
        status_error,
        journal: JournalState::of(journal),
    }))
}

/// Deletes the local branch `name` (see
/// [`GitRefs::delete_branch`](crate::GitRefs::delete_branch)). Refused
/// ([`EngineError::NotAllowed`]) for the branch of HEAD, a branch open in another worktree and
/// during an operation in progress. If the branch has commits that are neither in HEAD nor in its
/// upstream (if any), i.e. it is not merged, `"confirm": true` is needed. With `expected` (full id of the
/// commit on which the branch was shown) the branch must still point there, checked before
/// the confirmation and the deletion: otherwise [`EngineError::NotAllowed`] and nothing changes,
/// so a confirmation holds only for the commit seen: without it,
/// [`EngineError::ConfirmRequired`] with the number of commits and nothing changes. Before deleting
/// it always saves the commit in `refs/project/backup/delete-...`; the operation goes into the journal and
/// `edit.undo` recreates the branch on the same commit. The `branch.<name>.*` configuration
/// (upstream, description, ...) is removed as with `git branch -d`, but stays in the journal
/// entry: undo rewrites it (if a new one has not appeared in the meantime), redo removes it.
///
/// Result: [`BranchDeleteResult`].
pub(super) fn branch_delete(
    s: &mut Session,
    BranchDeleteParams {
        name,
        confirm,
        expected,
    }: BranchDeleteParams,
) -> Out {
    const ID: &str = "branch.delete";
    check_branch_name(ID, &name)?;
    let expected = expected
        .map(|hex| {
            let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
            full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
                .flatten()
                .ok_or_else(|| EngineError::InvalidParams {
                    id: ID,
                    reason: msg!(
                        "engine-branch-expected-hex",
                        len = Oid::HEX_LEN,
                        value = hex.escape_debug()
                    ),
                })
        })
        .transpose()?;
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    let full = branch_ref(&name);
    let head = HeadState::from_head(&status.head);
    if head == HeadState::Branch(full.clone()) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-branch-delete-active",
            name = name.escape_debug()
        )));
    }
    check_no_operation(ID, msg!("engine-busy-branch-delete"), status.operation)?;
    let refs = repo.refs()?;
    let branch = find(&refs, &full).ok_or_else(|| missing(&name))?;
    let target = branch.target;
    if let Some(seen) = expected
        && seen != target
    {
        return Err(EngineError::NotAllowed(msg!(
            "engine-branch-moved",
            name = name.escape_debug(),
            seen = seen,
            target = target
        )));
    }
    // Merged: reachable from HEAD or from its upstream, as for `git branch -d`.
    let upstream = branch
        .upstream
        .as_ref()
        .and_then(|up| find(&refs, &up.name));
    let into: Vec<Oid> = status
        .head
        .target()
        .into_iter()
        .chain(upstream.map(|r| r.target))
        .collect();
    let unmerged = repo.unmerged_count(&target, &into)?;
    if unmerged > 0 && !confirm {
        let reference = match upstream {
            Some(up) => msg!("engine-branch-not-in-upstream", upstream = up.name),
            None => msg!("engine-branch-not-in-head"),
        };
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-branch-unmerged",
                name = name.escape_debug(),
                unmerged = unmerged,
                tip = target.short(),
                reference = reference
            ),
            expected: None,
        });
    }
    let deleted = repo.delete_branch(&name, &target)?;
    journal.record(JournalEntry::branch_delete(
        &full,
        target,
        head,
        deleted.backup.clone(),
        deleted.config.clone(),
    ));
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(BranchDeleteResult {
        branch: full,
        target,
        unmerged,
        backup: deleted.backup,
        removed_config: deleted.config,
        warnings: deleted.warnings,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

/// Renames the local branch `name` to `new_name` (see
/// [`GitRefs::rename_branch`](crate::GitRefs::rename_branch)): never over an
/// existing branch; reflog, upstream and HEAD (if it was the active branch) follow. Refused during
/// an operation in progress. It goes into the journal: `edit.undo` renames back, only if in the meantime
/// nobody touched the two names.
///
/// Result: [`BranchRenameResult`].
pub(super) fn branch_rename(
    s: &mut Session,
    BranchRenameParams { name, new_name }: BranchRenameParams,
) -> Out {
    const ID: &str = "branch.rename";
    check_branch_name(ID, &name)?;
    check_new_branch_name(ID, &new_name)?;
    if name == new_name {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-branch-same-name"),
        });
    }
    let (repo, journal) = s.repo_and_journal()?;
    let status = repo.status()?;
    check_no_operation(ID, msg!("engine-busy-branch-rename"), status.operation)?;
    let (from, to) = (branch_ref(&name), branch_ref(&new_name));
    let refs = repo.refs()?;
    let target = find(&refs, &from).ok_or_else(|| missing(&name))?.target;
    if find(&refs, &to).is_some() {
        return Err(taken(&new_name));
    }
    let before = HeadState::from_head(&status.head);
    let is_head = before == HeadState::Branch(from.clone());
    let outcome = repo.rename_branch(&name, &new_name, &target)?;
    let after = if is_head {
        HeadState::Branch(to.clone())
    } else {
        before.clone()
    };
    journal.record(JournalEntry::branch_rename(
        &from,
        &to,
        target,
        HeadChange { before, after },
    ));
    let (refs, refs_error) = reread(repo.refs());
    Ok(to_json(BranchRenameResult {
        branch: to,
        previous: from,
        target,
        head: is_head,
        warnings: outcome.warnings,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
mod tests {
    use crate::{
        EngineError, GitIndex, GitRead, HeadState, MemoryBackend, MemoryOpener, Registry, Session,
    };
    use project_domain::{Change, EntryKind, Head, Oid, RepoPath, StatusEntry};
    use serde_json::{Value, json};
    use std::sync::Arc;

    const FILE: u32 = 0o100644;

    /// The sample repo with a clean worktree (no changes, no untracked files).
    fn clean_sample() -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        repo.set_entries(Vec::new());
        repo
    }

    fn open(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(opener));
        r.run("repo.open", &mut s, json!({ "path": "r" })).unwrap();
        (r, s)
    }

    fn run(r: &Registry, s: &mut Session, id: &str, params: Value) -> Value {
        r.run(id, s, params).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    fn branch(s: &Session, name: &str) -> Option<Oid> {
        s.repo()
            .unwrap()
            .refs()
            .unwrap()
            .into_iter()
            .find(|r| r.name == format!("refs/heads/{name}"))
            .map(|r| r.target)
    }

    fn head(s: &Session) -> Head {
        s.repo().unwrap().head().unwrap()
    }

    /// The short name of the branch of HEAD, `None` if detached.
    fn on(s: &Session) -> Option<String> {
        match head(s) {
            Head::Branch { name, .. } => Some(name),
            Head::Detached(_) => None,
        }
    }

    fn head_oid(s: &Session) -> Oid {
        head(s).target().unwrap()
    }

    #[test]
    fn create_from_head_and_from_a_rev_then_undo_and_redo() {
        let (r, mut s) = open(MemoryBackend::sample());
        let main = head_oid(&s);
        let out = run(&r, &mut s, "branch.create", json!({ "name": "brand-new" }));
        assert_eq!(out["branch"], "refs/heads/brand-new");
        assert_eq!(out["target"], main.to_string());
        assert_eq!(out["can_undo"], true);
        assert!(
            out["next_undo"]["text"]
                .as_str()
                .unwrap()
                .starts_with("Create branch `brand-new` at ")
        );
        assert!(
            out["refs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["name"] == "refs/heads/brand-new")
        );
        assert_eq!(branch(&s, "brand-new"), Some(main));
        // HEAD does not change.
        assert_eq!(on(&s).as_deref(), Some("main"));

        let graph_tip = branch(&s, "feature/graph").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.create",
            json!({ "name": "from-graph", "start": "feature/graph" }),
        );
        assert_eq!(out["target"], graph_tip.to_string());
        let short = &graph_tip.to_string()[..8];
        let out = run(
            &r,
            &mut s,
            "branch.create",
            json!({ "name": "from-id", "start": short }),
        );
        assert_eq!(out["target"], graph_tip.to_string());

        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(branch(&s, "from-id"), None);
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(branch(&s, "from-id"), Some(graph_tip));
    }

    #[test]
    fn create_refuses_existing_and_invalid_names() {
        let (r, mut s) = open(MemoryBackend::sample());
        let refs = s.repo().unwrap().refs().unwrap();
        for (params, want_invalid) in [
            (json!({ "name": "main" }), false),
            (json!({ "name": "feature" }), false), // `feature/graph` exists
            (json!({ "name": "main/under" }), false),
            (json!({ "name": "-x" }), true),
            (json!({ "name": "--force" }), true),
            (json!({ "name": "HEAD" }), true),
            (json!({ "name": "a b" }), true),
            (json!({ "name": "a..b" }), true),
            (json!({ "name": "refs/heads/x" }), true),
            (json!({ "name": "" }), true),
            (json!({ "name": "x", "other": 1 }), true),
        ] {
            let err = r.run("branch.create", &mut s, params.clone()).unwrap_err();
            if want_invalid {
                assert!(
                    matches!(err, EngineError::InvalidParams { .. }),
                    "{params}: {err}"
                );
            } else {
                assert!(matches!(err, EngineError::NotAllowed(_)), "{params}: {err}");
            }
        }
        let err = r
            .run(
                "branch.create",
                &mut s,
                json!({ "name": "x", "start": "missing" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownRev(_)), "{err}");
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
        assert!(!s.journal().unwrap().can_undo());

        // Without commits `start` is needed.
        let (r, mut s) = open(MemoryBackend::new());
        let err = r
            .run("branch.create", &mut s, json!({ "name": "x" }))
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }

    #[test]
    fn checkout_switches_and_undo_comes_back() {
        let (r, mut s) = open(clean_sample());
        let graph_tip = branch(&s, "feature/graph").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.checkout",
            json!({ "name": "feature/graph" }),
        );
        assert_eq!(out["branch"], "refs/heads/feature/graph");
        assert_eq!(out["previous"], json!({ "branch": "refs/heads/main" }));
        assert_eq!(out["changed"], true);
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(out["status"]["head"]["Branch"]["name"], "feature/graph");
        assert_eq!(
            out["next_undo"]["text"],
            "Checkout of `feature/graph` (from `main`)"
        );
        assert_eq!(head_oid(&s), graph_tip);
        assert!(s.repo().unwrap().status().unwrap().entries.is_empty());

        // Already on the branch: nothing changes, nothing in the journal.
        let out = run(
            &r,
            &mut s,
            "branch.checkout",
            json!({ "name": "feature/graph" }),
        );
        assert_eq!(out["changed"], false);
        assert_eq!(s.journal().unwrap().done().len(), 1);

        let out = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(out["undone"]["command"], "branch.checkout");
        assert_eq!(on(&s).as_deref(), Some("main"));
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(on(&s).as_deref(), Some("feature/graph"));

        // Undo refused if HEAD changed from outside.
        let main = branch(&s, "main").unwrap();
        s.repo_mut()
            .unwrap()
            .checkout(&HeadState::Detached(main))
            .unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
    }

    #[test]
    fn checkout_with_conflicting_changes_is_refused() {
        let (r, mut s) = open(MemoryBackend::sample());
        let status = s.repo().unwrap().status().unwrap();
        let err = r
            .run(
                "branch.checkout",
                &mut s,
                json!({ "name": "feature/graph" }),
            )
            .unwrap_err();
        let EngineError::CheckoutConflict { target, files, .. } = &err else {
            panic!("{err}");
        };
        assert_eq!(target, "feature/graph");
        assert!(!files.is_empty());
        assert!(err.to_string().contains("Nothing changed"), "{err}");
        assert_eq!(s.repo().unwrap().status().unwrap(), status);
        assert!(!s.journal().unwrap().can_undo());

        for (params, invalid) in [
            (json!({ "name": "missing" }), false),
            (json!({ "name": "a b" }), true),
            (json!({}), true),
        ] {
            let err = r
                .run("branch.checkout", &mut s, params.clone())
                .unwrap_err();
            assert_eq!(
                matches!(err, EngineError::InvalidParams { .. }),
                invalid,
                "{params}: {err}"
            );
        }
    }

    #[test]
    fn checkout_keeps_changes_to_files_that_do_not_differ() {
        let mut repo = clean_sample();
        let main = repo.head().unwrap().target().unwrap();
        // A branch on the same commit: no differing files, the changes stay.
        repo.set_branch("gemello", main);
        let sample = MemoryBackend::sample();
        let entries = sample.status().unwrap().entries;
        repo.set_entries(entries.clone());
        let (r, mut s) = open(repo);
        run(&r, &mut s, "branch.checkout", json!({ "name": "gemello" }));
        let status = s.repo().unwrap().status().unwrap();
        assert_eq!(status.entries, entries);
        assert!(matches!(&status.head, Head::Branch { name, .. } if name == "gemello"));
    }

    #[test]
    fn checkout_refused_during_an_operation_or_with_conflicts() {
        let mut repo = clean_sample();
        repo.set_operation(Some(project_domain::Operation::Rebase));
        let (r, mut s) = open(repo);
        let err = r
            .run(
                "branch.checkout",
                &mut s,
                json!({ "name": "feature/graph" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");

        let mut repo = clean_sample();
        repo.set_entries(vec![StatusEntry {
            path: RepoPath::from("x"),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(project_domain::UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: FILE,
                mode_ours: FILE,
                mode_theirs: FILE,
                mode_worktree: FILE,
                oid_base: MemoryBackend::fake_oid(1),
                oid_ours: MemoryBackend::fake_oid(2),
                oid_theirs: MemoryBackend::fake_oid(3),
            }),
        }]);
        let (r, mut s) = open(repo);
        let err = r
            .run(
                "branch.checkout",
                &mut s,
                json!({ "name": "feature/graph" }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NotAllowed(_)), "{err}");
    }

    #[test]
    fn delete_needs_confirm_when_unmerged_and_undo_recreates() {
        let mut repo = MemoryBackend::sample();
        let main = repo.head().unwrap().target().unwrap();
        // Is `feature/graph` (commit 9) in the upstream and in `main`? A branch with a commit
        // all its own is not.
        let tip = repo
            .commit(&crate::CommitRequest::new("Only here"))
            .unwrap()
            .commit
            .id;
        repo.set_branch("only", tip);
        repo.set_branch("main", main);
        repo.set_head_branch("main");
        let (r, mut s) = open(repo);
        let refs = s.repo().unwrap().refs().unwrap();

        let err = r
            .run("branch.delete", &mut s, json!({ "name": "only" }))
            .unwrap_err();
        let EngineError::ConfirmRequired {
            command, reason, ..
        } = &err
        else {
            panic!("{err}");
        };
        assert_eq!(*command, "branch.delete");
        assert!(reason.contains("1 commit"), "{reason}");
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
        assert!(!s.journal().unwrap().can_undo());

        // `expected` different from the tip (even with the confirmation), abbreviated or invalid:
        // nothing changes.
        let short = &tip.to_string()[..10];
        for (params, invalid) in [
            (
                json!({ "name": "only", "confirm": true, "expected": main.to_string() }),
                false,
            ),
            (
                json!({ "name": "only", "expected": main.to_string() }),
                false,
            ),
            (
                json!({ "name": "only", "confirm": true, "expected": short }),
                true,
            ),
            (
                json!({ "name": "only", "confirm": true, "expected": "x".repeat(40) }),
                true,
            ),
        ] {
            let err = r.run("branch.delete", &mut s, params.clone()).unwrap_err();
            if invalid {
                assert!(
                    matches!(err, EngineError::InvalidParams { .. }),
                    "{params}: {err}"
                );
            } else {
                assert!(
                    matches!(&err, EngineError::NotAllowed(m) if m.contains(&tip.to_string())),
                    "{params}: {err}"
                );
            }
        }
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
        assert!(!s.journal().unwrap().can_undo());
        // Without confirmation, with the right `expected`: the confirmation request remains.
        let err = r
            .run(
                "branch.delete",
                &mut s,
                json!({ "name": "only", "expected": tip.to_string() }),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::ConfirmRequired { .. }), "{err}");

        let out = run(
            &r,
            &mut s,
            "branch.delete",
            json!({ "name": "only", "confirm": true, "expected": tip.to_string().to_uppercase() }),
        );
        assert_eq!(out["branch"], "refs/heads/only");
        assert_eq!(out["target"], tip.to_string());
        assert_eq!(out["unmerged"], 1);
        assert_eq!(out["backup"], "refs/project/backup/delete-1");
        assert_eq!(branch(&s, "only"), None);

        let out = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(out["undone"]["command"], "branch.delete");
        assert_eq!(out["undone"]["backup"], "refs/project/backup/delete-1");
        assert_eq!(branch(&s, "only"), Some(tip));
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(branch(&s, "only"), None);
        run(&r, &mut s, "edit.undo", json!({}));

        // A branch with the same name created in the meantime: undo does not overwrite it.
        run(
            &r,
            &mut s,
            "branch.delete",
            json!({ "name": "only", "confirm": true }),
        );
        let (repo, _) = s.repo_and_journal().unwrap();
        repo.create_branch("only", &main).unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
        assert_eq!(branch(&s, "only"), Some(main));
    }

    #[test]
    fn delete_of_a_merged_branch_needs_no_confirm_but_the_current_is_refused() {
        let (r, mut s) = open(MemoryBackend::sample());
        let graph_tip = branch(&s, "feature/graph").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.delete",
            json!({ "name": "feature/graph" }),
        );
        assert_eq!(out["unmerged"], 0);
        assert_eq!(out["target"], graph_tip.to_string());
        assert_eq!(branch(&s, "feature/graph"), None);
        assert_eq!(
            out["removed_config"],
            json!([
                { "key": "branch.feature/graph.remote", "value": "origin" },
                { "key": "branch.feature/graph.merge", "value": "refs/heads/feature/graph" }
            ])
        );
        // Undo puts back branch and upstream, redo removes them.
        let upstream = |s: &Session| {
            s.repo()
                .unwrap()
                .refs()
                .unwrap()
                .into_iter()
                .find(|r| r.name == "refs/heads/feature/graph")
                .and_then(|r| r.upstream)
                .map(|u| u.name)
        };
        let out = run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(out["warnings"], json!([]));
        assert_eq!(
            upstream(&s).as_deref(),
            Some("refs/remotes/origin/feature/graph")
        );
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(branch(&s, "feature/graph"), None);
        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(
            upstream(&s).as_deref(),
            Some("refs/remotes/origin/feature/graph")
        );
        run(&r, &mut s, "edit.redo", json!({}));

        let refs = s.repo().unwrap().refs().unwrap();
        for (params, invalid) in [
            (json!({ "name": "main", "confirm": true }), false),
            (json!({ "name": "missing", "confirm": true }), false),
            (json!({ "name": "a b" }), true),
        ] {
            let err = r.run("branch.delete", &mut s, params.clone()).unwrap_err();
            assert_eq!(
                matches!(err, EngineError::InvalidParams { .. }),
                invalid,
                "{params}: {err}"
            );
        }
        assert_eq!(s.repo().unwrap().refs().unwrap(), refs);
    }

    #[test]
    fn rename_and_undo() {
        let (r, mut s) = open(MemoryBackend::sample());
        let main = branch(&s, "main").unwrap();
        let out = run(
            &r,
            &mut s,
            "branch.rename",
            json!({ "name": "main", "new_name": "principale" }),
        );
        assert_eq!(out["branch"], "refs/heads/principale");
        assert_eq!(out["previous"], "refs/heads/main");
        assert_eq!(out["head"], true);
        assert_eq!(
            out["next_undo"]["text"],
            "Rename branch `main` to `principale`"
        );
        assert_eq!(branch(&s, "main"), None);
        assert_eq!(branch(&s, "principale"), Some(main));
        assert_eq!(on(&s).as_deref(), Some("principale"));
        // The upstream follows the branch.
        let refs = s.repo().unwrap().refs().unwrap();
        let renamed = refs
            .iter()
            .find(|r| r.name == "refs/heads/principale")
            .unwrap();
        assert_eq!(
            renamed.upstream.as_ref().unwrap().name,
            "refs/remotes/origin/main"
        );

        run(&r, &mut s, "edit.undo", json!({}));
        assert_eq!(branch(&s, "main"), Some(main));
        assert_eq!(branch(&s, "principale"), None);
        assert_eq!(on(&s).as_deref(), Some("main"));
        run(&r, &mut s, "edit.redo", json!({}));
        assert_eq!(on(&s).as_deref(), Some("principale"));

        // Refusals: name already taken, same, invalid, missing branch.
        for (params, invalid) in [
            (
                json!({ "name": "principale", "new_name": "feature/graph" }),
                false,
            ),
            (json!({ "name": "missing", "new_name": "x" }), false),
            (
                json!({ "name": "principale", "new_name": "principale" }),
                true,
            ),
            (json!({ "name": "principale", "new_name": "-x" }), true),
            (json!({ "name": "principale" }), true),
        ] {
            let err = r.run("branch.rename", &mut s, params.clone()).unwrap_err();
            assert_eq!(
                matches!(err, EngineError::InvalidParams { .. }),
                invalid,
                "{params}: {err}"
            );
        }

        // Undo refused if the name was taken again in the meantime.
        let (repo, _) = s.repo_and_journal().unwrap();
        repo.create_branch("main", &main).unwrap();
        let err = r.run("edit.undo", &mut s, json!({})).unwrap_err();
        assert!(matches!(err, EngineError::UndoConflict { .. }), "{err}");
        assert_eq!(on(&s).as_deref(), Some("principale"));
    }

    #[test]
    fn branch_commands_need_an_open_repo() {
        let r = Registry::builtin();
        let mut s = Session::new(Arc::new(MemoryOpener::sample()));
        for (id, params) in [
            ("branch.create", json!({ "name": "x" })),
            ("branch.checkout", json!({ "name": "x" })),
            ("branch.delete", json!({ "name": "x" })),
            ("branch.rename", json!({ "name": "x", "new_name": "y" })),
        ] {
            let err = r.run(id, &mut s, params).unwrap_err();
            assert!(matches!(err, EngineError::NoRepo), "{id}: {err}");
        }
    }

    /// The typed results read back the JSON of the commands and give the same JSON again: the
    /// UI deserializes into them.
    #[test]
    fn results_round_trip() {
        use super::{
            BranchCheckoutResult, BranchCreateResult, BranchDeleteResult, BranchRenameResult,
        };
        fn same<T: serde::de::DeserializeOwned + serde::Serialize>(out: Value) {
            let typed: T = serde_json::from_value(out.clone()).unwrap();
            assert_eq!(serde_json::to_value(&typed).unwrap(), out);
        }
        let (r, mut s) = open(clean_sample());
        let out = run(&r, &mut s, "branch.create", json!({ "name": "x" }));
        assert!(out.get("refs_error").is_none());
        same::<BranchCreateResult>(out);
        let out = run(&r, &mut s, "branch.checkout", json!({ "name": "x" }));
        assert!(out.get("status_error").is_none());
        same::<BranchCheckoutResult>(out);
        let out = run(&r, &mut s, "branch.checkout", json!({ "name": "main" }));
        same::<BranchCheckoutResult>(out);
        let out = run(
            &r,
            &mut s,
            "branch.rename",
            json!({ "name": "x", "new_name": "y" }),
        );
        same::<BranchRenameResult>(out);
        let out = run(
            &r,
            &mut s,
            "branch.delete",
            json!({ "name": "feature/graph", "confirm": true }),
        );
        same::<BranchDeleteResult>(out);
    }

    /// A re-read that fails gives `null` and `<key>_error`; one that works has no `<key>_error`.
    #[test]
    fn reread_error_fields() {
        use super::{BranchCreateResult, JournalState, reread};
        let (refs, refs_error) = reread::<Vec<project_domain::RefInfo>>(Err(EngineError::NoRepo));
        let result = BranchCreateResult {
            branch: "refs/heads/x".into(),
            target: Oid::from_bytes([1; 20]),
            upstream: None,
            warnings: Vec::new(),
            refs,
            refs_error,
            journal: JournalState::default(),
        };
        let out = serde_json::to_value(&result).unwrap();
        assert_eq!(out["refs"], Value::Null);
        assert_eq!(out["refs_error"], EngineError::NoRepo.to_string());
        let back: BranchCreateResult = serde_json::from_value(out).unwrap();
        assert_eq!(back, result);
    }
}

#[cfg(test)]
#[path = "upstream_tests.rs"]
mod upstream_tests;
