//! Commands of the stashes (S28): `stash.list`, `stash.push`, `stash.apply`, `stash.pop`,
//! `stash.drop`. Types, checks common to the backends and choices in `git/stash.rs`; here the commands.
//! `stash.push` and `stash.drop` enter the journal, `stash.apply` and `stash.pop` do not (§7).

use super::{Out, check_path_list, journal_state, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, GitBackend, HeadState, JournalEntry, Session, StashEntry, StashPush,
    check_stash_apply, pop_stash,
};
use project_domain::Oid;
use serde::Deserialize;
use serde_json::{Value, json};

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

/// The list as JSON, or `null` with `stashes_error` if it cannot be re-read (the operation is done).
fn with_stashes(out: &mut Value, repo: &dyn GitBackend) {
    match repo.stashes() {
        Ok(list) => out["stashes"] = to_json(list),
        Err(e) => {
            out["stashes"] = Value::Null;
            out["stashes_error"] = Value::String(e.to_string());
        }
    }
}

/// The state as JSON, or `null` with `status_error` if it cannot be re-read.
fn with_status(out: &mut Value, repo: &dyn GitBackend) {
    match repo.status() {
        Ok(status) => out["status"] = to_json(status),
        Err(e) => {
            out["status"] = Value::Null;
            out["status_error"] = Value::String(e.to_string());
        }
    }
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

/// The list of stashes, most recent first: `{"stashes": [StashEntry]}` (see
/// [`StashEntry`]: `index`, `name`, `id`, `base`, `message`, `branch`, `time`, `untracked`).
pub(super) fn stash_list(s: &mut Session, p: Value) -> Out {
    let ListParams {} = parse_params("stash.list", p)?;
    Ok(json!({ "stashes": to_json(s.repo()?.stashes()?) }))
}

/// Sets changes aside in a new stash (see
/// [`GitBackend::stash_push`] and
/// [`check_stash_push`](crate::check_stash_push)): nothing is lost, the changes are in the
/// stash. It goes into the journal: `edit.undo` reapplies it (with the index) and removes it from the list, only
/// if HEAD has not moved.
///
/// Result: `{"stash": StashEntry, "stashes": [StashEntry], "status": Status, "can_undo",
/// "can_redo", "next_undo", "next_redo"}`.
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
    let mut out = journal_state(journal);
    out["stash"] = to_json(stash);
    with_stashes(&mut out, repo);
    with_status(&mut out, repo);
    Ok(out)
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
    let mut out = json!({
        "stash": to_json(&entry),
        "conflicts": to_json(&applied.conflicts),
        "dropped": was_dropped,
        "backup": backup,
        "warnings": warnings,
    });
    with_stashes(&mut out, repo);
    with_status(&mut out, repo);
    Ok(out)
}

/// Applies a stash to index and worktree without removing it from the list (see
/// [`GitBackend::stash_apply`]): never over local changes to the
/// files it changes. Conflicts stay in the files (`conflicts`), the stash stays.
///
/// Result: `{"stash": StashEntry, "conflicts": [string], "dropped": false, "backup": null,
/// "warnings": [string], "stashes": [StashEntry], "status": Status}`.
pub(super) fn stash_apply(s: &mut Session, p: Value) -> Out {
    apply("stash.apply", false, s, p)
}

/// Like `stash.apply`, then, if no conflicts remain, removes the stash from the list (first a
/// backup of its commit: `backup`). With conflicts the stash stays (`dropped: false`).
/// Result like `stash.apply`.
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
/// Result: `{"stash": StashEntry, "backup": string, "warnings": [string], "stashes":
/// [StashEntry], "can_undo", "can_redo", "next_undo", "next_redo"}`.
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
    let mut out = journal_state(journal);
    out["stash"] = to_json(&entry);
    out["backup"] = to_json(&dropped.backup);
    out["warnings"] = to_json(&dropped.warnings);
    with_stashes(&mut out, repo);
    Ok(out)
}

#[cfg(test)]
#[path = "stash_tests.rs"]
mod tests;
