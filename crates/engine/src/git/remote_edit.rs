//! Changing a remote in place (`remote.set_url`, `remote.rename`, S59): the journal record and
//! the undo/redo shared by the backends (`impl-docs/git/remotes.md`).
//!
//! Unlike `remote.remove` + `remote.add`, these keep the remote-tracking branches and the
//! upstreams of the local branches. Undo and redo write the other value back only if the repo
//! still has exactly the one written (otherwise [`EngineError::UndoConflict`], nothing changes).

use super::{GitBackend, RemoteSnapshot, RestoreOutcome, check_no_operation, mask_url};
use crate::{EngineError, Message};
use serde::{Serialize, Serializer};

/// What `remote.set_url` / `remote.rename` changed, kept in the journal entry
/// ([`JournalEntry::remote_change`](crate::JournalEntry::remote_change)).
///
/// JSON: `{"kind": "url", "name", "push", "before": string | null, "after"}` (URLs masked, see
/// [`mask_url`]) or `{"kind": "rename", "from", "to"}`. Not deserialized (masked URLs could not
/// be written back).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteChange {
    /// `remote.<name>.url` (or `pushurl` with `push`) from `before` (`None`: not set) to `after`.
    Url {
        name: String,
        push: bool,
        #[serde(serialize_with = "masked_opt")]
        before: Option<String>,
        #[serde(serialize_with = "masked")]
        after: String,
    },
    /// The remote `from` renamed to `to`.
    Rename { from: String, to: String },
}

fn masked<S: Serializer>(url: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&mask_url(url).0)
}

fn masked_opt<S: Serializer>(url: &Option<String>, s: S) -> Result<S::Ok, S::Error> {
    match url {
        Some(url) => masked(url, s),
        None => s.serialize_none(),
    }
}

/// The configuration key of the URL: `remote.<name>.url`, or `remote.<name>.pushurl` with `push`.
pub fn url_key(name: &str, push: bool) -> String {
    format!("remote.{name}.{}", if push { "pushurl" } else { "url" })
}

/// The values of `remote.<name>.url` (or `pushurl`) in the repo file, from `snapshot`.
pub fn url_values(snapshot: &RemoteSnapshot, push: bool) -> Vec<String> {
    let key = url_key(&snapshot.name, push).to_ascii_lowercase();
    snapshot
        .config
        .iter()
        .filter(|e| e.key.to_ascii_lowercase() == key)
        .map(|e| e.value.clone().unwrap_or_else(|| "true".to_owned()))
        .collect()
}

/// A URL (or none) for the messages, masked.
pub fn shown_url(url: Option<&str>) -> Message {
    match url {
        Some(url) => msg!("engine-ref-value", id = mask_url(url).0),
        None => msg!("engine-remote-url-none"),
    }
}

/// Undo (`redo == false`) or redo of a [`RemoteChange`].
pub fn apply_remote_change(
    repo: &mut dyn GitBackend,
    change: &RemoteChange,
    redo: bool,
) -> Result<RestoreOutcome, EngineError> {
    let command = if redo { "edit.redo" } else { "edit.undo" };
    match change {
        RemoteChange::Url {
            name,
            push,
            before,
            after,
        } => {
            let after = Some(after.clone());
            let (expected, target) = if redo {
                (before, &after)
            } else {
                (&after, before)
            };
            rewrite_url(
                repo,
                command,
                name,
                *push,
                expected.as_deref(),
                target.as_deref(),
            )
        }
        RemoteChange::Rename { from, to } => {
            let (from, to) = if redo { (from, to) } else { (to, from) };
            rename_back(repo, command, from, to)
        }
    }
}

fn conflict(command: &'static str, what: Message) -> EngineError {
    EngineError::UndoConflict {
        command,
        refs: vec![what],
    }
}

/// Writes `target` in the URL key of `name` only if it now holds exactly `expected`.
fn rewrite_url(
    repo: &mut dyn GitBackend,
    command: &'static str,
    name: &str,
    push: bool,
    expected: Option<&str>,
    target: Option<&str>,
) -> Result<RestoreOutcome, EngineError> {
    if !repo.remotes()?.iter().any(|r| r == name) {
        return Err(conflict(
            command,
            msg!("engine-undo-remote-gone", name = name.escape_debug()),
        ));
    }
    let now = url_values(&repo.remote_snapshot(name)?, push);
    let want: Vec<String> = expected.map(str::to_owned).into_iter().collect();
    if now != want {
        let shown = if now.is_empty() {
            msg!("engine-remote-url-none")
        } else {
            msg!(
                "engine-ref-value",
                id = now
                    .iter()
                    .map(|u| mask_url(u).0)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Err(conflict(
            command,
            msg!(
                "engine-undo-remote-url-changed",
                key = url_key(name, push),
                expected = shown_url(expected),
                now = shown
            ),
        ));
    }
    repo.set_remote_url(name, push, target)?;
    let mut warnings = Vec::new();
    let written = url_values(&repo.remote_snapshot(name)?, push);
    let want: Vec<String> = target.map(str::to_owned).into_iter().collect();
    if written != want {
        warnings.push(msg!(
            "engine-remote-url-not-as-written",
            key = url_key(name, push),
            expected = shown_url(target),
            now = shown_url(written.first().map(String::as_str))
        ));
    }
    Ok(RestoreOutcome {
        backups: Vec::new(),
        warnings,
    })
}

/// Renames `from` (which must exist) to `to` (which must not) for undo / redo.
fn rename_back(
    repo: &mut dyn GitBackend,
    command: &'static str,
    from: &str,
    to: &str,
) -> Result<RestoreOutcome, EngineError> {
    check_no_operation(
        command,
        msg!("engine-busy-remote-rename"),
        repo.status()?.operation,
    )?;
    let remotes = repo.remotes()?;
    if !remotes.iter().any(|r| r == from) {
        return Err(conflict(
            command,
            msg!("engine-undo-remote-gone", name = from.escape_debug()),
        ));
    }
    // Remote-tracking branches already under `to` are allowed only if none has the name of one
    // being moved: after a `git remote rename` stopped midway (it moves the configuration, then
    // the refs in one transaction) the old refs are still there, and renaming back must work.
    let target = repo.remote_snapshot(to)?;
    let before = repo.remote_snapshot(from)?;
    let from_prefix = format!("{}{from}/", super::REMOTES_PREFIX);
    let to_prefix = format!("{}{to}/", super::REMOTES_PREFIX);
    let collides = before.refs.iter().any(|r| {
        r.name.strip_prefix(&from_prefix).is_some_and(|rest| {
            let name = format!("{to_prefix}{rest}");
            target.refs.iter().any(|t| t.name == name)
        })
    });
    // Upstreams or `pushDefault` naming `to` would be taken over by the renamed remote, unless
    // its remote-tracking branches are still there too: a rename stopped midway leaves both on
    // the old name, and renaming back must give them back to it.
    let settings = !target.upstreams.is_empty() || target.push_default;
    if remotes.iter().any(|r| r == to)
        || !target.config.is_empty()
        || (settings && target.refs.is_empty())
        || collides
    {
        return Err(conflict(
            command,
            msg!("engine-undo-remote-taken", name = to.escape_debug()),
        ));
    }
    if let Some(other) = nested_remote(&remotes, from, to) {
        return Err(conflict(
            command,
            msg!(
                "engine-remote-rename-nested",
                name = from.escape_debug(),
                other = other.escape_debug()
            ),
        ));
    }
    let elsewhere = repo.remote_config_elsewhere(from)?;
    if !elsewhere.is_empty() {
        return Err(conflict(
            command,
            msg!(
                "engine-undo-remote-elsewhere",
                name = from.escape_debug(),
                elsewhere = elsewhere.join("; ")
            ),
        ));
    }
    let mut warnings = repo.rename_remote(from, to)?;
    let after = repo.remote_snapshot(to)?;
    warnings.extend(rename_warnings(&before, &after));
    Ok(RestoreOutcome {
        backups: Vec::new(),
        warnings,
    })
}

/// A configured remote (or `to`) whose name nests with `from` or `to` (`a` and `a/b`): `git remote
/// rename` moves every ref under `refs/remotes/<from>/`, also those of `<from>/b`, so such a rename
/// is refused. `None` if there is none.
pub fn nested_remote(remotes: &[String], from: &str, to: &str) -> Option<String> {
    let nests =
        |a: &str, b: &str| a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"));
    if nests(from, to) {
        return Some(to.to_owned());
    }
    remotes
        .iter()
        .find(|r| r.as_str() != from && (nests(r, from) || nests(r, to)))
        .cloned()
}

/// After a rename from `before` (the old remote) to `after` (the new one): what git did not
/// carry over, one warning each — remote-tracking branches missing or on another commit, upstreams,
/// `pushRemote` and `remote.pushDefault` (repo file) that do not name the new remote.
pub fn rename_warnings(before: &RemoteSnapshot, after: &RemoteSnapshot) -> Vec<Message> {
    let mut out = Vec::new();
    let old_prefix = format!("{}{}/", super::REMOTES_PREFIX, before.name);
    let new_prefix = format!("{}{}/", super::REMOTES_PREFIX, after.name);
    for r in &before.refs {
        let Some(rest) = r.name.strip_prefix(&old_prefix) else {
            continue;
        };
        let name = format!("{new_prefix}{rest}");
        if !after
            .refs
            .iter()
            .any(|n| n.name == name && n.target == r.target)
        {
            out.push(msg!(
                "engine-remote-rename-ref-missed",
                name = name,
                old = r.name
            ));
        }
    }
    let upstreams = after.upstream_branches();
    for branch in before.upstream_branches() {
        if !upstreams.contains(&branch) {
            out.push(msg!(
                "engine-remote-rename-upstream-missed",
                branch = branch,
                to = after.name
            ));
        }
    }
    let push = after.push_remote_branches();
    for branch in before.push_remote_branches() {
        if !push.contains(&branch) {
            out.push(msg!(
                "engine-remote-rename-push-remote-missed",
                branch = branch,
                to = after.name
            ));
        }
    }
    if before.push_default && !after.push_default {
        out.push(msg!(
            "engine-remote-rename-push-default-missed",
            to = after.name
        ));
    }
    out
}
