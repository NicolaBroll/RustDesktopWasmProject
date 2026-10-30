//! Management of the remotes (S27): `remote.list`, `remote.add`, `remote.remove`. The parts common
//! to the backends (name and URL checks, credential masking, removal and
//! restoration of a remote) are in `git/remote_admin.rs`; here the commands.

use super::remote::{RefUpdate, changes, unexpected};
use super::{JournalState, Out, reread, to_json};
use crate::{
    ConfigEntry, EngineError, FetchRequest, GitBackend, HeadState, JobContext, JournalEntry,
    Message, MessageArg, RemoteRef, RemoteSnapshot, Session, check_new_remote_name,
    check_no_operation, check_remote_name, check_remote_url, default_remote, ensure_remote_gone,
    mask_url, masked_entry, owned_by, remove_remote,
};
use project_domain::{RefInfo, RefKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A configured remote, as `remote.list` gives it (see there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteListEntry {
    /// The name of the remote.
    pub name: String,
    /// The first `remote.<name>.url`, credentials masked; `null` for a remote without a URL.
    pub url: Option<String>,
    /// Where the push goes if it is not `url` (masked); empty = the push goes to `url`.
    pub push_urls: Vec<String>,
    /// The `remote.<name>.fetch` refspecs.
    pub fetch: Vec<String>,
    /// `remote.<name>.mirror`.
    pub mirror: bool,
    /// How many remote-tracking branches `refs/remotes/<name>/...` there are.
    pub branches: usize,
    /// It is the remote `remote.fetch` uses without `remote`.
    pub default: bool,
    /// It is `remote.pushDefault`.
    pub push_default: bool,
    /// A configured URL contained credentials (masked here).
    pub credentials: bool,
}

/// Result of `remote.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteListResult {
    /// The configured remotes, in name order.
    pub remotes: Vec<RemoteListEntry>,
    /// The remote `remote.fetch` uses without `remote`; `null` if ambiguous.
    pub default: Option<String>,
    /// `remote.pushDefault` (the last value, even if it is not a configured remote).
    pub push_default: Option<String>,
}

/// The fetch done by `remote.add` with `"fetch": true` (`fetch` of [`RemoteAddResult`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteAddFetch {
    /// The refs changed by the fetch, in name order.
    pub updated: Vec<RefUpdate>,
    /// The git lines of the fetch that are not progress, in order.
    pub messages: Vec<String>,
    /// Refs outside `refs/remotes/` and `refs/tags/` moved by the fetch.
    pub warnings: Vec<Message>,
}

/// The kind of a failed fetch of `remote.add` (`kind` of [`RemoteAddFetchError`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchErrorKind {
    /// Authentication failed ([`EngineError::Auth`]).
    Auth,
    /// The remote could not be reached ([`EngineError::RemoteUnreachable`]).
    Unreachable,
    /// The fetch was cancelled.
    Cancelled,
    /// Any other error.
    Other,
}

/// The failed fetch of `remote.add` (`fetch_error` of [`RemoteAddResult`]): the remote stays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteAddFetchError {
    /// What kind of error.
    pub kind: FetchErrorKind,
    /// The error text.
    pub message: String,
}

/// Result of `remote.add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteAddResult {
    /// The name of the remote added.
    pub remote: String,
    /// The URL, credentials masked.
    pub url: String,
    /// The URL contained credentials (git saves them in clear text: `warnings` says so).
    pub credentials: bool,
    /// The fetch was requested and succeeded.
    pub fetched: bool,
    /// The fetch done; `null` without `"fetch": true` or if it failed.
    pub fetch: Option<RemoteAddFetch>,
    /// Why the fetch failed; `null` otherwise.
    pub fetch_error: Option<RemoteAddFetchError>,
    /// Warnings (credentials in the URL).
    pub warnings: Vec<Message>,
    /// The remotes re-read after the add (as in `remote.list`); `null` if they could not be
    /// re-read (see `remotes_error`).
    pub remotes: Option<Vec<RemoteListEntry>>,
    /// Why the remotes could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remotes_error: Option<String>,
    /// The refs re-read after the add; `null` if they could not be re-read (see `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

/// What `remote.remove` removed (`removed` of [`RemoteRemoveResult`]): the JSON of
/// [`RemoteSnapshot`], with the configuration values masked. It is a separate type because a
/// `RemoteSnapshot` is not deserialized (masked URLs could not put the remote back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovedRemote {
    /// The name of the remote.
    pub name: String,
    /// `remote.<name>.*` of the repo file, in file order, values masked ([`masked_entry`]).
    pub config: Vec<ConfigEntry>,
    /// The non-symbolic remote-tracking branches removed, in name order.
    pub refs: Vec<RemoteRef>,
    /// Where `refs/remotes/<name>/HEAD` pointed (full name), if it was a symbolic ref.
    pub head: Option<String>,
    /// `branch.<b>.remote`/`merge`/`pushRemote` removed, in file order.
    pub upstreams: Vec<ConfigEntry>,
    /// `remote.pushDefault` was this remote.
    pub push_default: bool,
}

impl RemovedRemote {
    /// The masked view of `snapshot` (the same JSON as its `Serialize`).
    fn of(snapshot: &RemoteSnapshot) -> Self {
        Self {
            name: snapshot.name.clone(),
            config: snapshot.config.iter().map(masked_entry).collect(),
            refs: snapshot.refs.clone(),
            head: snapshot.head.clone(),
            upstreams: snapshot.upstreams.clone(),
            push_default: snapshot.push_default,
        }
    }
}

/// Result of `remote.remove`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteRemoveResult {
    /// The name of the remote removed.
    pub remote: String,
    /// What was removed (configuration masked).
    pub removed: RemovedRemote,
    /// The local branches that lost their upstream.
    pub upstreams_removed: Vec<String>,
    /// The local branches that lost their `pushRemote`.
    pub push_remotes_removed: Vec<String>,
    /// `remote.pushDefault` was removed.
    pub push_default_removed: bool,
    /// The backup refs `refs/project/backup/remote-...` of the commits only in the remote.
    pub backups: Vec<String>,
    /// Problems while removing the configuration.
    pub warnings: Vec<Message>,
    /// The remotes re-read after the removal (as in `remote.list`); `null` if they could not
    /// be re-read (see `remotes_error`).
    pub remotes: Option<Vec<RemoteListEntry>>,
    /// Why the remotes could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remotes_error: Option<String>,
    /// The refs re-read after the removal; `null` if they could not be re-read (see
    /// `refs_error`).
    pub refs: Option<Vec<RefInfo>>,
    /// Why the refs could not be re-read; absent when they were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refs_error: Option<String>,
    /// The journal after the command.
    #[serde(flatten)]
    pub journal: JournalState,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ListParams {}

/// Parameters of `remote.add`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteAddParams {
    /// The name of the new remote.
    pub name: String,
    /// Its URL.
    pub url: String,
    /// Then download from it.
    #[serde(default)]
    pub fetch: bool,
}

/// Parameters of `remote.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RemoteRemoveParams {
    /// The remote.
    pub name: String,
    /// Always required to remove.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub confirm: bool,
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
fn remote_items(repo: &dyn GitBackend) -> Result<RemoteListResult, EngineError> {
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
        items.push(RemoteListEntry {
            name: name.clone(),
            url,
            push_urls,
            fetch,
            mirror,
            branches,
            default: default.as_ref() == Some(name),
            push_default: push_default.as_ref() == Some(name),
            credentials,
        });
    }
    Ok(RemoteListResult {
        remotes: items,
        default,
        push_default,
    })
}

/// The remotes and refs re-read after a write, as the fields `remotes`, `remotes_error`,
/// `refs`, `refs_error` (see [`reread`]); the remotes first.
type Reread = (
    Option<Vec<RemoteListEntry>>,
    Option<String>,
    Option<Vec<RefInfo>>,
    Option<String>,
);

/// Re-reads remotes and refs after a write (see [`Reread`]).
fn remotes_and_refs(repo: &dyn GitBackend) -> Reread {
    let (remotes, remotes_error) = reread(remote_items(repo).map(|list| list.remotes));
    let (refs, refs_error) = reread(repo.refs());
    (remotes, remotes_error, refs, refs_error)
}

/// The configured remotes, in name order (read-only).
///
/// Result: [`RemoteListResult`].
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
pub(super) fn remote_list(s: &mut Session, ListParams {}: ListParams) -> Out {
    remote_items(s.repo()?).map(to_json)
}

impl RemoteAddFetchError {
    /// The `fetch_error` of a failed fetch.
    fn of(e: &EngineError) -> Self {
        let kind = match e {
            EngineError::Auth { .. } => FetchErrorKind::Auth,
            EngineError::RemoteUnreachable { .. } => FetchErrorKind::Unreachable,
            EngineError::Cancelled { .. } => FetchErrorKind::Cancelled,
            _ => FetchErrorKind::Other,
        };
        Self {
            kind,
            message: e.to_string(),
        }
    }
}

/// Fetch of the whole remote `remote` (like `remote.fetch`).
fn fetch_all(
    repo: &mut dyn GitBackend,
    remote: &str,
    job: &JobContext,
    command: &'static str,
) -> Result<RemoteAddFetch, EngineError> {
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
    Ok(RemoteAddFetch {
        updated,
        messages: outcome.messages,
        warnings,
    })
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
/// Result: [`RemoteAddResult`].
pub(super) fn remote_add(
    s: &mut Session,
    RemoteAddParams { name, url, fetch }: RemoteAddParams,
) -> Out {
    const ID: &str = "remote.add";
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
    let (fetch_part, fetch_error) = if fetch {
        match fetch_all(repo, &name, &job, ID) {
            Ok(part) => (Some(part), None),
            Err(e) => (None, Some(RemoteAddFetchError::of(&e))),
        }
    } else {
        (None, None)
    };
    let fetched = fetch_part.is_some();
    let after = repo.remote_snapshot(&name)?;
    journal.record(JournalEntry::remote_add(after, head, fetched));
    let (remotes, remotes_error, refs, refs_error) = remotes_and_refs(repo);
    Ok(to_json(RemoteAddResult {
        remote: name,
        url: shown,
        credentials,
        fetched,
        fetch: fetch_part,
        fetch_error,
        warnings,
        remotes,
        remotes_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
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
/// Result: [`RemoteRemoveResult`].
pub(super) fn remote_remove(
    s: &mut Session,
    RemoteRemoveParams { name, confirm }: RemoteRemoveParams,
) -> Out {
    const ID: &str = "remote.remove";
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
    let (remotes, remotes_error, refs, refs_error) = remotes_and_refs(repo);
    Ok(to_json(RemoteRemoveResult {
        remote: name,
        removed: RemovedRemote::of(&snapshot),
        upstreams_removed: upstreams,
        push_remotes_removed: push_remotes,
        push_default_removed: push_default,
        backups: outcome.backups,
        warnings: outcome.warnings,
        remotes,
        remotes_error,
        refs,
        refs_error,
        journal: JournalState::of(journal),
    }))
}

#[cfg(test)]
#[path = "remotes_tests.rs"]
mod tests;
