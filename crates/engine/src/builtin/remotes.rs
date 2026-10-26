//! Management of the remotes (S27): `remote.list`, `remote.add`, `remote.remove`. The parts common
//! to the backends (name and URL checks, credential masking, removal and
//! restoration of a remote) are in `git/remote_admin.rs`; here the commands.

use super::remote::{changes, unexpected};
use super::{Out, journal_state, put_reread, to_json};
use crate::commands::parse_params;
use crate::{
    EngineError, FetchRequest, GitBackend, HeadState, JobContext, JournalEntry, Message,
    MessageArg, RemoteSnapshot, Session, check_new_remote_name, check_no_operation,
    check_remote_name, check_remote_url, default_remote, ensure_remote_gone, mask_url, owned_by,
    remove_remote,
};
use project_domain::RefKind;
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {}

/// Parameters of `remote.add`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AddParams {
    name: String,
    url: String,
    #[serde(default)]
    fetch: bool,
}

/// Parameters of `remote.remove`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveParams {
    name: String,
    #[serde(default)]
    confirm: bool,
}

/// True for a git boolean value (`true`, `yes`, `on`, `1`).
fn truthy(value: Option<&String>) -> bool {
    value.is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "true" | "yes" | "on" | "1" | ""
        )
    })
}

/// The configured remotes as `remote.list` gives them (see there).
fn remote_items(repo: &dyn GitBackend) -> Result<Value, EngineError> {
    let remotes = repo.remotes()?;
    let refs = repo.refs()?;
    let default = repo
        .status()
        .ok()
        .and_then(|status| default_remote("remote.list", &status, &remotes).ok());
    // Like git: the last value wins.
    let push_default = repo.config_values("remote.pushDefault")?.pop();
    let mut items = Vec::new();
    for name in &remotes {
        let urls = repo.config_values(&format!("remote.{name}.url"))?;
        let push = repo.config_values(&format!("remote.{name}.pushurl"))?;
        let mut credentials = false;
        let mut show = |url: &String| {
            let (shown, masked) = mask_url(url);
            credentials |= masked;
            shown
        };
        let url = urls.first().map(&mut show);
        // Without `pushurl` the push goes to all the `url`s: they are listed only if there is more than one.
        let push_urls: Vec<String> = if !push.is_empty() {
            push.iter().map(&mut show).collect()
        } else if urls.len() > 1 {
            urls.iter().map(&mut show).collect()
        } else {
            Vec::new()
        };
        let fetch = repo.config_values(&format!("remote.{name}.fetch"))?;
        let mirror = truthy(repo.config_values(&format!("remote.{name}.mirror"))?.last());
        let branches = refs
            .iter()
            .filter(|r| r.kind == RefKind::RemoteBranch && owned_by(name, &remotes, &r.name))
            .count();
        items.push(json!({
            "name": name,
            "url": url,
            "push_urls": push_urls,
            "fetch": fetch,
            "mirror": mirror,
            "branches": branches,
            "default": default.as_ref() == Some(name),
            "push_default": push_default.as_ref() == Some(name),
            "credentials": credentials,
        }));
    }
    Ok(json!({
        "remotes": items,
        "default": default,
        "push_default": push_default,
    }))
}

/// Adds the re-read remotes and refs to `out` (see [`put_reread`]).
fn with_remotes_and_refs(out: &mut Value, repo: &dyn GitBackend) {
    let remotes = remote_items(repo).map(|mut list| list["remotes"].take());
    put_reread(out, "remotes", remotes);
    put_reread(out, "refs", repo.refs());
}

/// The configured remotes, in name order (read-only).
///
/// Result: `{"remotes": [{"name", "url": string | null, "push_urls": [string], "fetch":
/// [string], "mirror": bool, "branches": n, "default": bool, "push_default": bool,
/// "credentials": bool}], "default": string | null, "push_default": string | null}`.
///
/// - `url`: the first `remote.<name>.url` (the fetch one), as written in the configuration
///   (without applying `url.<base>.insteadOf`); `null` for a remote without a URL.
/// - `push_urls`: where the push goes if it is not `url` (the `remote.<name>.pushurl`s, or all the
///   `url`s if there is more than one); empty = the push goes to `url`.
/// - Credentials in URLs are **never shown**: `https://user:token@host` becomes
///   `https://user:***@host`, and in an `http(s)`/`ftp(s)` URL with only the user
///   (`https://token@host`) the user too becomes `***`; `credentials` says the URL had them
///   (see [`mask_url`]).
/// - `fetch`: the `remote.<name>.fetch` refspecs; `mirror`: `remote.<name>.mirror`.
/// - `branches`: how many remote-tracking branches `refs/remotes/<name>/...` there are (without those of a
///   remote `<name>/...`).
/// - `default`: the remote `remote.fetch` uses without `remote` (upstream of the active branch,
///   `origin`, the only remote; `null` if ambiguous); `push_default`: `remote.pushDefault`
///   (the last value, even if it is not a configured remote).
pub(super) fn remote_list(s: &mut Session, p: Value) -> Out {
    let ListParams {} = parse_params("remote.list", p)?;
    remote_items(s.repo()?)
}

/// The kind of error of a fetch for `fetch_error.kind`.
fn error_kind(e: &EngineError) -> &'static str {
    match e {
        EngineError::Auth { .. } => "auth",
        EngineError::RemoteUnreachable { .. } => "unreachable",
        EngineError::Cancelled { .. } => "cancelled",
        _ => "other",
    }
}

/// Fetch of the whole remote `remote` (like `remote.fetch`): `{"updated", "messages",
/// "warnings"}`.
fn fetch_all(
    repo: &mut dyn GitBackend,
    remote: &str,
    job: &JobContext,
    command: &'static str,
) -> Result<Value, EngineError> {
    if job.is_cancelled() {
        return Err(EngineError::Cancelled { command });
    }
    let before = repo.refs()?;
    let outcome = repo
        .fetch(
            &FetchRequest {
                remote: remote.to_owned(),
                branch: None,
                prune: false,
            },
            job,
        )
        .map_err(|e| match e {
            EngineError::Cancelled { .. } => EngineError::Cancelled { command },
            e => e,
        })?;
    let after = repo.refs()?;
    let updated = changes(&before, &after);
    let warnings: Vec<Message> = updated.iter().filter_map(unexpected).collect();
    Ok(json!({
        "updated": updated,
        "messages": outcome.messages,
        "warnings": warnings,
    }))
}

/// Adds the remote `name` with `url` (see
/// [`GitBackend::create_remote`]: `git remote add`, default
/// refspec) and, with `"fetch": true`, then fetches from there like `remote.fetch` (slow: progress
/// and cancellation from the [`JobContext`]).
///
/// Checks before writing: new name ([`check_new_remote_name`]: no leading `-`) and
/// URL ([`check_remote_url`]: no leading `-`, no `ext::`/`fd::`, no host that
/// starts with `-`, no control characters) → [`EngineError::InvalidParams`]; a remote
/// with that name already configured, or remote-tracking branches `refs/remotes/<name>/...` left over from before
/// (undo would delete them) → [`EngineError::NotAllowed`]. A URL with credentials is accepted
/// (git saves it in clear text in the repo configuration: `warnings` says so).
///
/// It goes into the journal: `edit.undo` removes the remote (configuration and fetched remote-tracking branches) only
/// if it is still as it was right after; `edit.redo` puts it back. A failed or cancelled fetch does not
/// undo the addition: the remote stays (and is in the journal) and the error is in `fetch_error`.
///
/// Result: `{"remote", "url" (masked), "credentials": bool, "fetched": bool, "fetch":
/// {"updated", "messages", "warnings"} | null, "fetch_error": {"kind": "auth" | "unreachable" |
/// "cancelled" | "other", "message"} | null, "warnings": [string], "remotes": [as
/// remote.list], "refs": [RefInfo], "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn remote_add(s: &mut Session, p: Value) -> Out {
    const ID: &str = "remote.add";
    let AddParams { name, url, fetch } = parse_params(ID, p)?;
    check_new_remote_name(ID, &name)?;
    check_remote_url(ID, &url)?;
    let job = s.job().clone();
    let (repo, journal) = s.repo_and_journal()?;
    if repo.remotes()?.contains(&name) {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-add-exists",
            name = name.escape_debug()
        )));
    }
    let before = repo.remote_snapshot(&name)?;
    if !before.config.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-add-config-exists",
            name = name.escape_debug()
        )));
    }
    if !before.refs.is_empty() || before.head.is_some() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-add-refs-exist",
            name = name.escape_debug(),
            n = before.refs.len()
        )));
    }
    let head = HeadState::from_head(&repo.head()?);
    repo.create_remote(&name, &url)?;
    let (shown, credentials) = mask_url(&url);
    let mut warnings = Vec::new();
    if credentials {
        warnings.push(msg!("engine-remote-url-credentials", name = name));
    }
    let (fetch_out, fetch_error) = if fetch {
        match fetch_all(repo, &name, &job, ID) {
            Ok(out) => (out, Value::Null),
            Err(e) => (
                Value::Null,
                json!({ "kind": error_kind(&e), "message": e.to_string() }),
            ),
        }
    } else {
        (Value::Null, Value::Null)
    };
    let fetched = !fetch_out.is_null();
    let after = repo.remote_snapshot(&name)?;
    journal.record(JournalEntry::remote_add(after, head, fetched));
    let mut out = journal_state(journal);
    out["remote"] = Value::String(name);
    out["url"] = Value::String(shown);
    out["credentials"] = Value::Bool(credentials);
    out["fetched"] = Value::Bool(fetched);
    out["fetch"] = fetch_out;
    out["fetch_error"] = fetch_error;
    out["warnings"] = to_json(warnings);
    with_remotes_and_refs(&mut out, repo);
    Ok(out)
}

/// `a`, `b` for the messages.
fn names(list: &[String]) -> String {
    list.iter()
        .map(|b| format!("`{}`", b.escape_debug()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the removal would remove, for the confirmation.
fn removal_summary(snapshot: &RemoteSnapshot) -> MessageArg {
    let mut parts = vec![msg!(
        "engine-remote-remove-config",
        name = snapshot.name,
        n = snapshot.refs.len()
    )];
    let upstreams = snapshot.upstream_branches();
    if !upstreams.is_empty() {
        parts.push(msg!(
            "engine-remote-remove-upstreams",
            branches = names(&upstreams)
        ));
    }
    let push = snapshot.push_remote_branches();
    if !push.is_empty() {
        parts.push(msg!(
            "engine-remote-remove-push-remote",
            branches = names(&push)
        ));
    }
    if snapshot.push_default {
        parts.push(msg!("engine-remote-remove-push-default"));
    }
    MessageArg::Joined(parts, ", ".into())
}

/// Removes the configured remote `name`: the `remote.<name>` section of the repo file, the remote-tracking
/// branches `refs/remotes/<name>/...` (and `refs/remotes/<name>/HEAD`), the upstream of the local
/// branches that use it (`branch.<b>.remote`/`merge`), the `branch.<b>.pushRemote` that name it
/// and `remote.pushDefault` if it is `name` (like `git remote remove`). On the real remote nothing
/// changes; fetched tags stay.
///
/// It always asks for `"confirm": true`: without it, [`EngineError::ConfirmRequired`] listing the branches
/// that would lose their upstream and the number of remote-tracking branches, and nothing changes. Refusals
/// ([`EngineError::NotAllowed`]): a remote that is not configured; one defined, even only in
/// part, outside the repo file (global or system configuration, `include`,
/// `config.worktree`, `.git/remotes/`: the error says where); the promisor remote of a partial
/// clone (`remote.<name>.promisor`, `extensions.partialClone`: without it, missing objects
/// could no longer be found); an operation in progress. If after the removal git still sees the
/// remote, everything is put back immediately as it was and the result is [`EngineError::NotAllowed`].
///
/// Before writing, the journal entry saves everything ([`RemoteSnapshot`]: configuration,
/// remote-tracking branches with the commits, upstream) and the commits that would remain only in remote-tracking branches
/// (not reachable from local branches, tags, other refs or backups) go into backup refs
/// `refs/project/backup/remote-...` (one per tip commit, in `backups`), so undo
/// finds them even after a `gc`. The remote-tracking branches are removed with a transaction (`update-ref
/// --stdin`, with the values read); then the configuration (a problem there goes into `warnings`). Unlike
/// `git remote remove`, refs outside `refs/remotes/<name>/` written by a custom
/// refspec (e.g. `refs/pull/*`) stay.
/// `edit.undo` puts back remote-tracking branches, configuration and upstream, only if in the meantime nobody
/// recreated a remote with that name (otherwise [`EngineError::UndoConflict`]); an upstream
/// that in the meantime was set elsewhere stays, with a warning.
///
/// Result: `{"remote", "removed": RemoteSnapshot (masked URLs), "upstreams_removed":
/// [branch], "push_remotes_removed": [branch], "push_default_removed": bool, "backups":
/// [string], "warnings": [string], "remotes": [as remote.list], "refs": [RefInfo],
/// "can_undo", "can_redo", "next_undo", "next_redo"}`.
pub(super) fn remote_remove(s: &mut Session, p: Value) -> Out {
    const ID: &str = "remote.remove";
    let RemoveParams { name, confirm } = parse_params(ID, p)?;
    check_remote_name(ID, &name)?;
    let (repo, journal) = s.repo_and_journal()?;
    let remotes = repo.remotes()?;
    if !remotes.contains(&name) {
        let known = if remotes.is_empty() {
            msg!("engine-remote-none-configured")
        } else {
            msg!("engine-remote-configured", names = names(&remotes))
        };
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-remove-unknown",
            name = name.escape_debug(),
            known = known
        )));
    }
    let status = repo.status()?;
    check_no_operation(ID, msg!("engine-busy-remote-remove"), status.operation)?;
    let snapshot = repo.remote_snapshot(&name)?;
    if snapshot.config.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-remove-elsewhere",
            name = name.escape_debug()
        )));
    }
    let elsewhere = repo.remote_config_elsewhere(&name)?;
    if !elsewhere.is_empty() {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-remove-partly-elsewhere",
            name = name.escape_debug(),
            elsewhere = elsewhere.join("; ")
        )));
    }
    let promisor = truthy(
        repo.config_values(&format!("remote.{name}.promisor"))?
            .last(),
    );
    let partial = repo
        .config_values("extensions.partialClone")?
        .contains(&name);
    if promisor || partial {
        return Err(EngineError::NotAllowed(msg!(
            "engine-remote-remove-promisor",
            remote = name.escape_debug(),
            name = name
        )));
    }
    if !confirm {
        return Err(EngineError::ConfirmRequired {
            command: ID,
            reason: msg!(
                "engine-remote-remove-confirm",
                name = name.escape_debug(),
                summary = removal_summary(&snapshot)
            ),
            expected: None,
        });
    }
    let head = HeadState::from_head(&status.head);
    let reflog = format!("project: remote.remove {name}");
    let outcome = remove_remote(repo, &snapshot, ID, true, reflog)?;
    let entry = JournalEntry::remote_remove(snapshot.clone(), head, &outcome.backups);
    // A part defined elsewhere that escaped the check: everything is put back immediately. If that
    // fails, the removal goes into the journal anyway (the error says so).
    if let Some(still) = ensure_remote_gone(repo, &snapshot, ID) {
        if !still.put_back {
            journal.record(entry);
        }
        return Err(still.error);
    }
    let upstreams = snapshot.upstream_branches();
    let push_remotes = snapshot.push_remote_branches();
    let push_default = snapshot.push_default;
    journal.record(entry);
    let mut out = journal_state(journal);
    out["remote"] = Value::String(name);
    out["removed"] = to_json(&snapshot);
    out["upstreams_removed"] = to_json(upstreams);
    out["push_remotes_removed"] = to_json(push_remotes);
    out["push_default_removed"] = Value::Bool(push_default);
    out["backups"] = to_json(outcome.backups);
    out["warnings"] = to_json(outcome.warnings);
    with_remotes_and_refs(&mut out, repo);
    Ok(out)
}

#[cfg(test)]
#[path = "remotes_tests.rs"]
mod tests;
