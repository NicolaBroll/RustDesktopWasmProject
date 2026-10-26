//! Commands of the stashes (S28): `stash.list`, `stash.push`, `stash.apply`, `stash.pop`,
//! `stash.drop`. Types, checks common to the backends and choices in `git/stash.rs`; here the commands.
//! `stash.push` and `stash.drop` enter the journal, `stash.apply` and `stash.pop` do not (§7).

use super::{JournalState, Out, check_path_list, reread, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, GitBackend, HeadState, JournalEntry, Message, Session, StashEntry, StashPush,
    check_stash_apply, pop_stash,
};
use project_domain::{Oid, Status};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Result of `stash.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashListResult {
    /// The stashes, most recent first.
    pub stashes: Vec<StashEntry>,
}

/// Result of `stash.push`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashPushResult {
    /// The new stash (`stash@{0}`).
    pub stash: StashEntry,
    /// The stashes re-read after the push; `null` if they could not be re-read (see
    /// `stashes_error`).
    pub stashes: Option<Vec<StashEntry>>,
    /// Why the stashes could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stashes_error: Option<String>,
    /// The status re-read after the push; `null` if it could not be re-read (see `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// Result of `stash.apply` and `stash.pop` (same shape; `stash.apply` always has `dropped:
/// false` and `backup: null`). Neither enters the journal, so there are no journal fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashApplyResult {
    /// The stash applied, as it was in the list before the command.
    pub stash: StashEntry,
    /// The files left with conflicts (empty if none).
    pub conflicts: Vec<String>,
    /// The stash was removed from the list (`stash.pop` without conflicts).
    pub dropped: bool,
    /// The backup ref of the dropped stash's commit (`refs/project/backup/stash-...`), `null` if
    /// not dropped or no backup was created.
    pub backup: Option<String>,
    /// Warnings: the stash kept because of conflicts or because its drop failed, and the
    /// warnings of apply and drop.
    pub warnings: Vec<Message>,
    /// The stashes re-read after the command; `null` if they could not be re-read (see
    /// `stashes_error`).
    pub stashes: Option<Vec<StashEntry>>,
    /// Why the stashes could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stashes_error: Option<String>,
    /// The status re-read after the command; `null` if it could not be re-read (see
    /// `status_error`).
    pub status: Option<Status>,
    /// Why the status could not be re-read; absent when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_error: Option<String>,
}

/// Result of `stash.drop`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashDropResult {
    /// The dropped stash, as it was in the list before the command.
    pub stash: StashEntry,
    /// The backup ref that keeps the stash's commit (`refs/project/backup/stash-...`).
    pub backup: Option<String>,
    /// The warnings of the drop.
    pub warnings: Vec<Message>,
    /// The stashes re-read after the drop; `null` if they could not be re-read (see
    /// `stashes_error`).
    pub stashes: Option<Vec<StashEntry>>,
    /// Why the stashes could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stashes_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {}

/// Parameters of `stash.apply` and `stash.pop`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyParams {
    /// The position: `n` of `stash@{n}` (0 if missing).
    #[serde(default)]
    index: usize,
    /// The full id of the stash as seen by the caller.
    #[serde(default)]
    expected: Option<String>,
    #[serde(default)]
    restore_index: bool,
}

/// Parameters of `stash.drop`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DropParams {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    expected: Option<String>,
    #[serde(default)]
    confirm: bool,
}

/// `expected` as a full id (40 digits), or [`EngineError::InvalidParams`].
fn full_id(id: &'static str, hex: Option<String>) -> Result<Option<Oid>, EngineError> {
    hex.map(|hex| {
        let full = hex.len() == Oid::HEX_LEN && hex.bytes().all(|b| b.is_ascii_hexdigit());
        full.then(|| hex.to_ascii_lowercase().parse::<Oid>().ok())
            .flatten()
            .ok_or_else(|| EngineError::InvalidParams {
                id,
                reason: msg!(
                    "engine-stash-expected-hex",
                    len = Oid::HEX_LEN,
                    value = hex.escape_debug()
                ),
            })
    })
    .transpose()
}

/// The stash at position `index`, which must be `expected` if given: otherwise
/// [`EngineError::NotAllowed`] (it does not exist, or the list changed since it was seen).
fn pick(
    repo: &dyn GitBackend,
    index: usize,
    expected: Option<Oid>,
) -> Result<StashEntry, EngineError> {
    let list = repo.stashes()?;
    let Some(entry) = list.get(index) else {
        return Err(EngineError::NotAllowed(match list.len() {
            0 => msg!("engine-stash-none"),
            n => msg!("engine-stash-no-index", index = index, n = n, last = n - 1),
        }));
    };
    if let Some(seen) = expected
        && seen != entry.id
    {
        let now = match list.iter().find(|s| s.id == seen) {
            Some(s) => msg!("engine-stash-now-at", name = s.name),
            None => msg!("engine-stash-now-gone"),
        };
        return Err(EngineError::NotAllowed(msg!(
            "engine-stash-list-changed",
            index = index,
            id = entry.id,
            seen = seen,
            now = now
        )));
    }
    Ok(entry.clone())
}

/// The list of stashes, most recent first (see [`StashEntry`]: `index`, `name`, `id`, `base`,
/// `message`, `branch`, `time`, `untracked`).
///
/// Result: [`StashListResult`].
pub(super) fn stash_list(s: &mut Session, p: Value) -> Out {
    let ListParams {} = parse_params("stash.list", p)?;
    Ok(to_json(StashListResult {
        stashes: s.repo()?.stashes()?,
    }))
}

/// Sets changes aside in a new stash (see
/// [`GitBackend::stash_push`] and
/// [`check_stash_push`](crate::check_stash_push)): nothing is lost, the changes are in the
/// stash. It goes into the journal: `edit.undo` reapplies it (with the index) and removes it from the list, only
/// if HEAD has not moved.
///
/// Result: [`StashPushResult`].
pub(super) fn stash_push(s: &mut Session, p: Value) -> Out {
    const ID: &str = "stash.push";
    let mut request: StashPush = parse_params(ID, p)?;
    request.check()?;
    if let Some(paths) = request.paths.take() {
        request.paths = Some(check_path_list(ID, paths)?);
    }
    let (repo, journal) = s.repo_and_journal()?;
    let head = HeadState::from_head(&repo.head()?);
    let stash = repo.stash_push(&request)?;
    journal.record(JournalEntry::stash_push(&stash, head, request));
    let (stashes, stashes_error) = reread(repo.stashes());
    let (status, status_error) = reread(repo.status());
    Ok(to_json(StashPushResult {
        stash,
        stashes,
        stashes_error,
        status,
        status_error,
        journal: JournalState::of(journal),
    }))
}

/// `stash.apply` and `stash.pop`: applies the stash `index` (checked with `expected`) and, for
/// `pop`, if no conflicts remain removes it from the list after saving its commit in
/// `refs/project/backup/stash-...`. They do not enter the journal.
fn apply(id: &'static str, pop: bool, s: &mut Session, p: Value) -> Out {
    let ApplyParams {
        index,
        expected,
        restore_index,
    } = parse_params(id, p)?;
    let expected = full_id(id, expected)?;
    let repo = s.repo_mut()?;
    let entry = pick(repo, index, expected)?;
    check_stash_apply(id, &repo.status()?)?;
    let (applied, dropped, drop_error) = if pop {
        let popped = pop_stash(repo, id, entry.id, restore_index, Some("stash"))?;
        (popped.applied, popped.dropped, popped.drop_error)
    } else {
        let applied = repo.stash_apply(&crate::StashApply {
            command: id,
            id: entry.id,
            restore_index,
        })?;
        (applied, None, None)
    };
    let was_dropped = dropped.is_some();
    let mut warnings = applied.warnings;
    if let Some(e) = drop_error {
        // The apply is done: it must not be repeated, the stash has only stayed in the list.
        warnings.push(msg!(
            "engine-stash-not-dropped",
            name = entry.name,
            error = e
        ));
    } else if pop && !was_dropped {
        warnings.push(msg!("engine-stash-kept-conflicts", name = entry.name));
    }
    let backup = dropped.as_ref().and_then(|d| d.backup.clone());
    warnings.extend(dropped.into_iter().flat_map(|d| d.warnings));
    let (stashes, stashes_error) = reread(repo.stashes());
    let (status, status_error) = reread(repo.status());
    Ok(to_json(StashApplyResult {
        stash: entry,
        conflicts: applied.conflicts,
        dropped: was_dropped,
        backup,
        warnings,
        stashes,
        stashes_error,
        status,
        status_error,
    }))
}

/// Applies a stash to index and worktree without removing it from the list (see
/// [`GitBackend::stash_apply`]): never over local changes to the
/// files it changes. Conflicts stay in the files (`conflicts`), the stash stays.
///
/// Result: [`StashApplyResult`] (`dropped: false`, `backup: null`).
pub(super) fn stash_apply(s: &mut Session, p: Value) -> Out {
    apply("stash.apply", false, s, p)
}

/// Like `stash.apply`, then, if no conflicts remain, removes the stash from the list (first a
/// backup of its commit: `backup`). With conflicts the stash stays (`dropped: false`).
/// Result: [`StashApplyResult`].
pub(super) fn stash_pop(s: &mut Session, p: Value) -> Out {
    apply("stash.pop", true, s, p)
}

/// Drops the stash `index`: needs `"confirm": true` **with `"expected"`**, the full id of the
/// confirmed stash (without `confirm`, [`EngineError::ConfirmRequired`] which says what would
/// be lost and has the id in `expected`; `confirm` without `expected`,
/// [`EngineError::InvalidParams`]): the confirmation holds only for the stash seen. First a backup of the commit
/// in `refs/project/backup/stash-...`; it goes into the journal, and `edit.undo` puts it back at the top
/// of the list.
///
/// Result: [`StashDropResult`].
pub(super) fn stash_drop(s: &mut Session, p: Value) -> Out {
    const ID: &str = "stash.drop";
    let DropParams {
        index,
        expected,
        confirm,
    } = parse_params(ID, p)?;
    let expected = full_id(ID, expected)?;
    if confirm && expected.is_none() {
        return Err(EngineError::InvalidParams {
            id: ID,
            reason: msg!("engine-stash-confirm-without-expected"),
        });
    }
    let (repo, journal) = s.repo_and_journal()?;
    let entry = pick(repo, index, expected)?;
    if !confirm {
        let files = repo
            .show_commit(&entry.id.to_string())
            .map(|d| d.files.len())
            .ok();
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-stash-drop-confirm",
                name = entry.name,
                id = entry.id,
                message = entry.message,
                files = files.map_or(-1, |n| i64::try_from(n).unwrap_or(i64::MAX)),
                untracked = if entry.untracked { "yes" } else { "no" }
            ),
            expected: Some(entry.id.to_string()),
        });
    }
    let head = HeadState::from_head(&repo.head()?);
    let dropped = repo.stash_drop(&crate::StashDrop {
        command: ID,
        id: entry.id,
        backup: Some("stash"),
    })?;
    journal.record(JournalEntry::stash_drop(
        &entry,
        head,
        dropped.backup.clone(),
    ));
    let (stashes, stashes_error) = reread(repo.stashes());
    Ok(to_json(StashDropResult {
        stash: entry,
        backup: dropped.backup,
        warnings: dropped.warnings,
        stashes,
        stashes_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
#[path = "stash_tests.rs"]
mod tests;
