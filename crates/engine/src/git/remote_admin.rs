//! Remote management (`remote.list`, `remote.add`, `remote.remove`, S27): types and checks
//! shared by the backends, and the removal/restoration of a remote used by the commands and the journal
//! (`impl-docs/git/remotes.md`).
//!
//! A remote is made of three parts, which [`RemoteSnapshot`] captures:
//!
//! - its configuration in the repo file (`remote.<name>.*`: URL, refspec, ...);
//! - its remote branches (`refs/remotes/<name>/...`, excluding those of another remote named
//!   `<name>/something`) and the symbolic ref `refs/remotes/<name>/HEAD`;
//! - the entries of others that name it ("dependents"): the upstream of local branches
//!   (`branch.<b>.remote` and `branch.<b>.merge`), `branch.<b>.pushRemote` and
//!   `remote.pushDefault`, always from the repo file.
//!
//! Removing a remote ([`remove_remote`]) removes the remote branches with a single transaction
//! (`GitBackend::restore_refs`, with the expected values and the backups of the commits that would no longer be
//! reachable), then the configuration; restoring it ([`restore_remote`]) does the opposite,
//! only if in the meantime nobody recreated a remote with that name.

use super::{ConfigEntry, GitBackend, HeadState, RefMove, RefRestore, RestoreOutcome};
use super::{check_no_operation, check_remote_name};
use crate::{EngineError, Message, MessageArg};
use project_domain::Oid;
use serde::{Deserialize, Serialize, Serializer};
use std::collections::BTreeSet;

pub use project_domain::REMOTES_PREFIX;

/// A remote branch of a remote: full name and commit.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RemoteRef {
    pub name: String,
    pub target: Oid,
}

/// What `remote.remove` removes (and the journal puts back): see the module.
///
/// JSON: `{"name", "config": [{"key", "value"}], "refs": [{"name", "target"}], "head": string |
/// null, "upstreams": [{"key", "value"}], "push_default": bool}`. In the JSON the configuration
/// values are masked as in `remote.list` ([`mask_url`]): the in-memory journal
/// keeps the real values, needed by undo, but `edit.history` does not show passwords and tokens.
/// For this reason it is not deserialized: a `RemoteSnapshot` read from JSON would have masked
/// URLs and would not be of use to put the remote back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RemoteSnapshot {
    /// The name of the remote.
    pub name: String,
    /// `remote.<name>.*` of the repo file, in file order.
    #[serde(serialize_with = "masked_entries")]
    pub config: Vec<ConfigEntry>,
    /// The non-symbolic remote branches, in name order.
    pub refs: Vec<RemoteRef>,
    /// Where `refs/remotes/<name>/HEAD` points (full name), if it is a symbolic ref.
    pub head: Option<String>,
    /// `branch.<b>.remote` and `branch.<b>.merge` of the branches that have the remote as upstream,
    /// and `branch.<b>.pushRemote` that name it, in file order.
    pub upstreams: Vec<ConfigEntry>,
    /// `remote.pushDefault` (repo file) equals this remote.
    pub push_default: bool,
}

fn masked_entries<S: Serializer>(entries: &[ConfigEntry], s: S) -> Result<S::Ok, S::Error> {
    let masked: Vec<ConfigEntry> = entries.iter().map(masked_entry).collect();
    masked.serialize(s)
}

/// The entry with credentials masked ([`mask_url`]) in the value: not just `*.url` and
/// `*.pushurl`, any value that is a URL with credentials (e.g. `remote.x.proxy`).
pub fn masked_entry(entry: &ConfigEntry) -> ConfigEntry {
    ConfigEntry {
        key: entry.key.clone(),
        value: entry.value.as_deref().map(|v| mask_url(v).0),
    }
}

impl RemoteSnapshot {
    /// True if nothing exists of the remote: neither configuration nor remote branches.
    pub fn is_absent(&self) -> bool {
        self.config.is_empty() && self.refs.is_empty() && self.head.is_none()
    }

    /// The differences between `self` (the expected state) and `now`, one per line, for
    /// [`EngineError::UndoConflict`]; empty if equal. Configuration and dependents are
    /// compared as sets (the file order may change when they are rewritten).
    pub fn differences(&self, now: &RemoteSnapshot) -> Vec<Message> {
        let mut out = Vec::new();
        let sorted = |entries: &[ConfigEntry]| {
            let mut v: Vec<(String, String)> = entries
                .iter()
                // A key without a value is rewritten as `true`: it is the same thing for git.
                .map(|e| {
                    let value = e.value.clone().unwrap_or_else(|| "true".to_owned());
                    (e.key.to_ascii_lowercase(), value)
                })
                .collect();
            v.sort();
            v
        };
        if sorted(&self.config) != sorted(&now.config) {
            out.push(msg!("engine-remote-config-changed", name = self.name));
        }
        let (old, new): (BTreeSet<&RemoteRef>, BTreeSet<&RemoteRef>) =
            (self.refs.iter().collect(), now.refs.iter().collect());
        for r in old.difference(&new) {
            let current = now.refs.iter().find(|n| n.name == r.name);
            out.push(match current {
                Some(c) => msg!(
                    "engine-ref-changed",
                    name = r.name,
                    expected = r.target,
                    now = c.target
                ),
                None => msg!("engine-remote-ref-gone", name = r.name, expected = r.target),
            });
        }
        for r in new.difference(&old) {
            if !self.refs.iter().any(|o| o.name == r.name) {
                out.push(msg!("engine-remote-ref-new", name = r.name, now = r.target));
            }
        }
        if self.head != now.head {
            let head = |h: &Option<String>| match h {
                Some(h) => msg!("engine-ref-value", id = h),
                None => msg!("engine-remote-head-none"),
            };
            out.push(msg!(
                "engine-remote-head-changed",
                name = self.name,
                expected = head(&self.head),
                now = head(&now.head)
            ));
        }
        if sorted(&self.upstreams) != sorted(&now.upstreams) {
            out.push(msg!("engine-remote-upstreams-changed", name = self.name));
        }
        if self.push_default != now.push_default {
            out.push(msg!("engine-remote-push-default-changed"));
        }
        out
    }

    /// The local branches (short names) that have the remote as upstream (`branch.<b>.remote`), in
    /// name order.
    pub fn upstream_branches(&self) -> Vec<String> {
        self.dependent_branches("remote")
    }

    /// The local branches (short names) with `branch.<b>.pushRemote` equal to the remote, in
    /// name order.
    pub fn push_remote_branches(&self) -> Vec<String> {
        self.dependent_branches("pushremote")
    }

    fn dependent_branches(&self, var: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .upstreams
            .iter()
            .filter_map(|e| split_branch_key(&e.key))
            .filter(|(_, v)| v == var)
            .map(|(b, _)| b.to_owned())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The URL (`remote.<name>.url`) as written, if any.
    pub fn url(&self) -> Option<&str> {
        self.config
            .iter()
            .find(|e| e.key.to_ascii_lowercase().ends_with(".url"))
            .and_then(|e| e.value.as_deref())
    }

    /// The same remote without the dependents (for the undo and redo of `remote.add`, which does not
    /// touch them).
    fn without_dependents(&self) -> RemoteSnapshot {
        RemoteSnapshot {
            upstreams: Vec::new(),
            push_default: false,
            ..self.clone()
        }
    }
}

/// `branch.<b>.<var>` → (`b`, `var` in lowercase); `None` for another key.
pub fn split_branch_key(key: &str) -> Option<(&str, String)> {
    let rest = key.strip_prefix("branch.")?;
    let (branch, var) = rest.rsplit_once('.')?;
    (!branch.is_empty()).then(|| (branch, var.to_ascii_lowercase()))
}

/// Among the `branch.<b>.(remote|merge|pushRemote)` entries of the configuration (`entries`), those
/// that depend on the remote `name`: `remote` and `merge` of the branches with `branch.<b>.remote =
/// <name>`, and the `pushRemote` equal to `name`. In the order of `entries`.
pub fn remote_dependents(name: &str, entries: &[ConfigEntry]) -> Vec<ConfigEntry> {
    let tracking: BTreeSet<&str> = entries
        .iter()
        .filter_map(|e| {
            let (branch, var) = split_branch_key(&e.key)?;
            (var == "remote" && e.value.as_deref() == Some(name)).then_some(branch)
        })
        .collect();
    entries
        .iter()
        .filter(|e| match split_branch_key(&e.key) {
            Some((branch, var)) if var == "remote" || var == "merge" => tracking.contains(branch),
            Some((_, var)) if var == "pushremote" => e.value.as_deref() == Some(name),
            _ => false,
        })
        .cloned()
        .collect()
}

/// True if the ref `refname` is a remote branch of the remote `remote`: it is under
/// `refs/remotes/<remote>/` and not under `refs/remotes/<other>/` of another configured remote
/// named `<remote>/...` (remote names may contain `/`).
pub fn owned_by(remote: &str, remotes: &[String], refname: &str) -> bool {
    let prefix = format!("{REMOTES_PREFIX}{remote}/");
    if !refname.starts_with(&prefix) {
        return false;
    }
    !remotes.iter().any(|other| {
        other.len() > remote.len()
            && other.starts_with(&format!("{remote}/"))
            && refname.starts_with(&format!("{REMOTES_PREFIX}{other}/"))
    })
}

/// The name of a **new** remote (`remote.add`): like [`check_remote_name`] and in addition it does not
/// start with `-` (git would accept it after `--end-of-options`, but then every hand-written
/// command would take it for an option). Otherwise [`EngineError::InvalidParams`].
pub fn check_new_remote_name(id: &'static str, name: &str) -> Result<(), EngineError> {
    check_remote_name(id, name)?;
    if name.starts_with('-') {
        return Err(EngineError::InvalidParams {
            id,
            reason: msg!("engine-remote-name-dash", name = name.escape_debug()),
        });
    }
    Ok(())
}

/// The URL of a new remote (`remote.add`). Policy (before passing it to git, after
/// `--end-of-options`):
///
/// - not empty, without leading or trailing spaces and without control characters (newline, NUL);
/// - does not start with `-` (an `--upload-pack=...` or `-oProxyCommand=...` must never be able to
///   become a git or ssh option);
/// - no transports that run commands or use descriptors: `ext::`/`ext://` and
///   `fd::`/`fd://`, in any letter case (git allows them if `protocol.ext.allow` says so;
///   never from here). Other helpers (`<transport>::<address>`) stay allowed;
/// - a host or user starting with `-` (`ssh://-oProxyCommand=x/r`, `ssh://-oX@h/r`,
///   `user@-x:r`, `ssh://[-x]/r`) is rejected.
///
/// Otherwise [`EngineError::InvalidParams`]. Credentials in the URL are accepted (git
/// saves them in the configuration, in clear text): the command says so in a warning.
pub fn check_remote_url(id: &'static str, url: &str) -> Result<(), EngineError> {
    let reason = if url.trim().is_empty() {
        Some(msg!("engine-remote-url-empty"))
    } else if url.chars().any(char::is_control) {
        Some(msg!("engine-remote-url-control"))
    } else if url.trim() != url {
        Some(msg!("engine-remote-url-spaces"))
    } else if url.starts_with('-') {
        Some(msg!("engine-remote-url-dash"))
    } else if let Some(transport) = forbidden_transport(url) {
        Some(msg!("engine-remote-url-transport", transport = transport))
    } else if dash_in_authority(url) {
        Some(msg!("engine-remote-url-authority-dash"))
    } else {
        None
    };
    match reason {
        Some(reason) => Err(EngineError::InvalidParams {
            id,
            reason: msg!(
                "engine-with-name",
                reason = reason,
                name = mask_url(url).0.escape_debug().to_string()
            ),
        }),
        None => Ok(()),
    }
}

/// `ext` or `fd` if the URL uses one of those transports (`ext::...`, `ext://...`), also inside
/// another helper.
fn forbidden_transport(url: &str) -> Option<&'static str> {
    let lower = url.to_ascii_lowercase();
    for transport in ["ext", "fd"] {
        if lower.starts_with(&format!("{transport}::"))
            || lower.starts_with(&format!("{transport}://"))
        {
            return Some(transport);
        }
    }
    // `<helper>::<address>`: the address is checked too.
    let (scheme, rest) = url.split_once("::")?;
    scheme_like(scheme)
        .then(|| forbidden_transport(rest))
        .flatten()
}

/// True if `s` can be a URL scheme (`[A-Za-z][A-Za-z0-9+.-]*`).
fn scheme_like(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
}

/// User and host of a URL `scheme://[user@]host...` or of an scp-like address
/// `[user@]host:...` (without `/` before the `:`), with the `[]` of an IPv6 host removed; `None` for
/// a path.
fn split_user(authority: &str) -> (Option<&str>, &str) {
    match authority.rsplit_once('@') {
        Some((user, host)) => (Some(user), host),
        None => (None, authority),
    }
}

fn authority_of(url: &str) -> Option<(Option<&str>, &str)> {
    let url = match url.split_once("::") {
        Some((scheme, rest)) if scheme_like(scheme) => rest,
        _ => url,
    };
    let (user, host) = if let Some((scheme, rest)) = url.split_once("://")
        && scheme_like(scheme)
    {
        split_user(rest.split(['/', '?', '#']).next().unwrap_or_default())
    } else {
        let colon = url.find(':')?;
        if url[..colon].contains('/') {
            return None;
        }
        split_user(&url[..colon])
    };
    Some((user, host.trim_start_matches('[')))
}

/// True if the user or host of the URL starts with `-` (it would become an ssh option).
fn dash_in_authority(url: &str) -> bool {
    authority_of(url).is_some_and(|(user, host)| {
        host.starts_with('-') || user.is_some_and(|u| u.starts_with('-'))
    })
}

/// The URL without credentials, to display, and whether it had any. Policy: in a URL
/// `scheme://user:password@host/...` the password becomes `***` (`https://nicola:***@host/r`);
/// in an `http(s)://` or `ftp(s)://` URL with only the user the user also becomes `***`
/// (`https://***@github.com/r`: there the "user" is often a token). A user without password in
/// `ssh://`, `git://` or scp-like (`git@host:r`) stays: it is not a secret. It also applies inside a
/// helper (`<transport>::<url>`). Everything else stays as is.
pub fn mask_url(url: &str) -> (String, bool) {
    if let Some((scheme, rest)) = url.split_once("::")
        && scheme_like(scheme)
    {
        let (inner, masked) = mask_url(rest);
        return (format!("{scheme}::{inner}"), masked);
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_owned(), false);
    };
    if !scheme_like(scheme) {
        return (url.to_owned(), false);
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return (url.to_owned(), false);
    };
    let web = matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ftp" | "ftps"
    );
    let shown = match userinfo.split_once(':') {
        Some((user, _)) => format!("{user}:***"),
        None if web => "***".to_owned(),
        None => return (url.to_owned(), false),
    };
    (format!("{scheme}://{shown}@{host}{tail}"), true)
}

/// Removes the remote captured in `snapshot` (`remote.remove`, undo of `remote.add`, redo of
/// `remote.remove`), only if right now it is exactly so ([`RemoteSnapshot::differences`]:
/// otherwise [`EngineError::UndoConflict`] for `edit.*`, [`EngineError::NotAllowed`] for the
/// other commands, and nothing changes). With `dependents` it also removes the saved upstream, `pushRemote` and
/// `remote.pushDefault` (for `remote.add` not: it did not create them).
///
/// The remote branches are removed with [`GitBackend::restore_refs`] (a transaction with the expected
/// values; the commits that would no longer be reachable from other refs go first in
/// `refs/project/backup/<remote|undo|redo>-...`, in the result), then the configuration: a
/// problem there is a warning, because the refs are already removed.
pub fn remove_remote(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    command: &'static str,
    dependents: bool,
    reflog: String,
) -> Result<RestoreOutcome, EngineError> {
    let status = repo.status()?;
    check_no_operation(command, msg!("engine-busy-remote-remove"), status.operation)?;
    let now = repo.remote_snapshot(&snapshot.name)?;
    let differences = snapshot.differences(&now);
    if !differences.is_empty() {
        return Err(changed(command, &snapshot.name, differences));
    }
    let mut outcome = RestoreOutcome::default();
    if !snapshot.refs.is_empty() {
        let head = HeadState::from_head(&status.head);
        outcome = repo.restore_refs(&RefRestore {
            command,
            head_expected: head.clone(),
            head_target: head,
            refs: snapshot
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: Some(r.target),
                    target: None,
                })
                .collect(),
            reflog,
        })?;
    }
    let target = if dependents {
        snapshot.clone()
    } else {
        snapshot.without_dependents()
    };
    outcome.warnings.extend(repo.remove_remote_config(&target));
    Ok(outcome)
}

/// Puts back the remote captured in `snapshot` (undo of `remote.remove`, redo of
/// `remote.add`), only if right now nothing exists of the remote (neither among the configured remotes nor
/// configuration nor remote branches: otherwise [`EngineError::UndoConflict`] and nothing changes).
/// First the remote branches (a transaction that creates them only if they do not exist), then the
/// configuration. With `dependents` also upstream, `pushRemote` and `remote.pushDefault`, but
/// only where the repo does not have a new one and the branch still exists: the others stay as they are,
/// with a warning.
pub fn restore_remote(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    command: &'static str,
    dependents: bool,
    reflog: String,
) -> Result<(RestoreOutcome, RemoteSnapshot), EngineError> {
    let status = repo.status()?;
    check_no_operation(
        command,
        msg!("engine-busy-remote-restore"),
        status.operation,
    )?;
    let now = repo.remote_snapshot(&snapshot.name)?;
    let configured = repo.remotes()?.contains(&snapshot.name);
    if configured || !now.is_absent() {
        let mut refs = vec![msg!("engine-remote-exists-again", name = snapshot.name)];
        refs.extend(
            now.refs
                .iter()
                .map(|r| msg!("engine-remote-ref-exists", name = r.name)),
        );
        return Err(changed(command, &snapshot.name, refs));
    }
    let (outcome, written, _) = put_back(
        repo,
        snapshot,
        &status.head,
        command,
        dependents,
        reflog,
        true,
    )?;
    Ok((outcome, written))
}

/// Puts back remote branches and configuration of `snapshot` without first checking that the remote
/// is not there (the refs are created anyway only if they do not exist). Returns the outcome and what
/// was actually rewritten (the skipped dependents are not there). With `strict` an error on the
/// refs stops everything before writing (undo and redo: nothing changes); without it (the emergency
/// restore of [`ensure_remote_gone`]) it becomes a warning and the configuration is written
/// anyway.
fn put_back(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    head: &project_domain::Head,
    command: &'static str,
    dependents: bool,
    reflog: String,
    strict: bool,
) -> Result<(RestoreOutcome, RemoteSnapshot, bool), EngineError> {
    let mut outcome = RestoreOutcome::default();
    let mut complete = true;
    if !snapshot.refs.is_empty() {
        let head = HeadState::from_head(head);
        let restored = repo.restore_refs(&RefRestore {
            command,
            head_expected: head.clone(),
            head_target: head,
            refs: snapshot
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: None,
                    target: Some(r.target),
                })
                .collect(),
            reflog,
        });
        match restored {
            Ok(o) => outcome = o,
            Err(e) if strict => return Err(e),
            Err(e) => {
                complete = false;
                outcome.warnings.push(msg!(
                    "engine-remote-refs-not-put-back",
                    name = snapshot.name,
                    error = e
                ));
            }
        }
    }
    let mut target = snapshot.without_dependents();
    if dependents {
        match free_dependents(repo, snapshot) {
            Ok((upstreams, push_default, warnings)) => {
                target.upstreams = upstreams;
                target.push_default = push_default;
                outcome.warnings.extend(warnings);
            }
            Err(e) if strict => return Err(e),
            Err(e) => {
                complete = false;
                outcome
                    .warnings
                    .push(msg!("engine-remote-upstreams-not-put-back", error = e));
            }
        }
    }
    outcome.warnings.extend(repo.write_remote_config(&target));
    Ok((outcome, target, complete))
}

/// Outcome of [`ensure_remote_gone`] when git still sees the remote after the removal.
#[derive(Debug)]
pub struct RemoteStillListed {
    /// The error to return ([`EngineError::NotAllowed`]).
    pub error: EngineError,
    /// True if the remote was fully put back (configuration, remote branches, upstream): the
    /// removal does not go in the journal. False if the restore failed or it is not known whether
    /// git sees the remote: the removal goes in the journal, so `edit.undo` and the backups remain.
    pub put_back: bool,
}

/// After `remote.remove`: if git still sees the remote (defined also outside the repo file,
/// discovered too late) it is put back immediately as it was; `None` if it really disappeared.
pub fn ensure_remote_gone(
    repo: &mut dyn GitBackend,
    snapshot: &RemoteSnapshot,
    command: &'static str,
) -> Option<RemoteStillListed> {
    let name = snapshot.name.escape_debug().to_string();
    match repo.remotes() {
        Ok(remotes) if !remotes.contains(&snapshot.name) => return None,
        Ok(_) => {}
        Err(e) => {
            return Some(RemoteStillListed {
                error: EngineError::NotAllowed(msg!(
                    "engine-remote-removed-unreadable",
                    name = name,
                    error = e
                )),
                put_back: false,
            });
        }
    }
    let reflog = format!("project: {command} undone, restoring {}", snapshot.name);
    let restored = repo
        .head()
        .and_then(|head| put_back(repo, snapshot, &head, command, true, reflog, false));
    let (how, put_back) = match restored {
        Ok((_, _, true)) => (msg!("engine-remote-put-back"), true),
        Ok((outcome, _, false)) => (
            msg!(
                "engine-remote-put-back-partly",
                warnings = MessageArg::Joined(outcome.warnings, "; ".into())
            ),
            false,
        ),
        Err(e) => (msg!("engine-remote-not-put-back", error = e), false),
    };
    Some(RemoteStillListed {
        error: EngineError::NotAllowed(msg!("engine-remote-still-listed", name = name, how = how)),
        put_back,
    })
}

/// The dependents of `snapshot` that can be rewritten: those of branches that still exist
/// and meanwhile have no other upstream / pushRemote, and `remote.pushDefault` if it is now not
/// there. The others, one warning each.
fn free_dependents(
    repo: &dyn GitBackend,
    snapshot: &RemoteSnapshot,
) -> Result<(Vec<ConfigEntry>, bool, Vec<Message>), EngineError> {
    let refs = repo.refs()?;
    let mut warnings = Vec::new();
    let mut skipped: BTreeSet<(String, bool)> = BTreeSet::new();
    let mut upstreams = Vec::new();
    for entry in &snapshot.upstreams {
        let Some((branch, var)) = split_branch_key(&entry.key) else {
            continue;
        };
        let push = var == "pushremote";
        if skipped.contains(&(branch.to_owned(), push)) {
            continue;
        }
        let exists = refs.iter().any(|r| r.name == super::branch_ref(branch));
        let taken = if push {
            !repo
                .config_values(&format!("branch.{branch}.pushRemote"))?
                .is_empty()
        } else {
            repo.upstream(branch)?.is_some()
        };
        if !exists || taken {
            let what = if push { "push-remote" } else { "upstream" };
            warnings.push(if exists {
                msg!(
                    "engine-remote-upstream-taken",
                    branch = branch,
                    what = what,
                    name = snapshot.name
                )
            } else {
                msg!(
                    "engine-remote-upstream-branch-gone",
                    branch = branch,
                    what = what,
                    name = snapshot.name
                )
            });
            skipped.insert((branch.to_owned(), push));
            continue;
        }
        upstreams.push(entry.clone());
    }
    let mut push_default = snapshot.push_default;
    if push_default && !repo.config_values("remote.pushDefault")?.is_empty() {
        warnings.push(msg!(
            "engine-remote-push-default-taken",
            name = snapshot.name
        ));
        push_default = false;
    }
    Ok((upstreams, push_default, warnings))
}

fn changed(command: &'static str, name: &str, refs: Vec<Message>) -> EngineError {
    if command.starts_with("edit.") {
        EngineError::UndoConflict { command, refs }
    } else {
        EngineError::NotAllowed(msg!(
            "engine-remote-changed",
            name = name.escape_debug(),
            command = command,
            refs = MessageArg::Joined(refs, "; ".into())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, value: &str) -> ConfigEntry {
        ConfigEntry {
            key: key.to_owned(),
            value: Some(value.to_owned()),
        }
    }

    #[test]
    fn masking() {
        let cases = [
            (
                "https://nicola:tok@host/r.git",
                "https://nicola:***@host/r.git",
                true,
            ),
            (
                "https://ghp_x@github.com/r",
                "https://***@github.com/r",
                true,
            ),
            ("http://u:p@h:8080/x?y@z", "http://u:***@h:8080/x?y@z", true),
            ("ssh://git@host/r", "ssh://git@host/r", false),
            ("ssh://git:pw@host/r", "ssh://git:***@host/r", true),
            ("git@github.com:a/b.git", "git@github.com:a/b.git", false),
            ("/srv/r.git", "/srv/r.git", false),
            ("https://host/path@x", "https://host/path@x", false),
            (
                "persistent-https::https://u:p@h/r",
                "persistent-https::https://u:***@h/r",
                true,
            ),
            ("https://a:b:c@h@x/r", "https://a:***@x/r", true),
            ("http://u:pw@proxy:3128", "http://u:***@proxy:3128", true),
        ];
        for (url, shown, masked) in cases {
            assert_eq!(mask_url(url), (shown.to_owned(), masked), "{url}");
        }
    }

    #[test]
    fn urls() {
        for ok in [
            "https://example.com/r.git",
            "git@github.com:a/b.git",
            "../r.git",
            "/srv/with space/r.git",
            "file:///srv/r",
            "ssh://git@host:22/r",
            "persistent-https::https://h/r",
        ] {
            check_remote_url("remote.add", ok).unwrap();
        }
        for bad in [
            "",
            "  ",
            " https://h/r",
            "-x",
            "--upload-pack=touch /tmp/x",
            "ext::sh -c touch% /tmp/x",
            "EXT::sh",
            "ext://sh",
            "fd::3",
            "x::ext::sh",
            "ssh://-oProxyCommand=x/r",
            "git@-oProxyCommand=x:r",
            "ssh://-oProxyCommand=x@h/r",
            "ssh://[-oProxyCommand=x]/r",
            "https://h/r\n",
            "a\0b",
        ] {
            let err = check_remote_url("remote.add", bad).unwrap_err();
            assert!(matches!(err, EngineError::InvalidParams { .. }), "{bad:?}");
        }
        // The error does not show the password.
        let err = check_remote_url("remote.add", "ext::https://u:segreto@h").unwrap_err();
        assert!(!err.to_string().contains("segreto"), "{err}");
    }

    #[test]
    fn new_names() {
        check_new_remote_name("remote.add", "fork/a").unwrap();
        for bad in ["-x", "--upload-pack=x", "", "a b", "a..b"] {
            assert!(check_new_remote_name("remote.add", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dependents_and_ownership() {
        let entries = [
            entry("branch.main.remote", "origin"),
            entry("branch.main.merge", "refs/heads/main"),
            entry("branch.a.b.remote", "fork"),
            entry("branch.a.b.merge", "refs/heads/x"),
            entry("branch.a.b.pushremote", "origin"),
            entry("branch.c.pushremote", "fork"),
        ];
        let deps = remote_dependents("origin", &entries);
        assert_eq!(
            deps,
            [entries[0].clone(), entries[1].clone(), entries[4].clone()]
        );
        let snapshot = RemoteSnapshot {
            name: "origin".into(),
            upstreams: deps,
            ..RemoteSnapshot::default()
        };
        assert_eq!(snapshot.upstream_branches(), ["main"]);
        assert_eq!(snapshot.push_remote_branches(), ["a.b"]);

        let remotes = vec!["fork".to_owned(), "fork/a".to_owned()];
        assert!(owned_by("fork", &remotes, "refs/remotes/fork/main"));
        assert!(!owned_by("fork", &remotes, "refs/remotes/fork/a/main"));
        assert!(owned_by("fork/a", &remotes, "refs/remotes/fork/a/main"));
        assert!(!owned_by("fork", &remotes, "refs/remotes/forked/main"));
    }

    #[test]
    fn differences_and_masked_json() {
        let a = RemoteSnapshot {
            name: "o".into(),
            config: vec![entry("remote.o.url", "https://u:p@h/r")],
            refs: vec![RemoteRef {
                name: "refs/remotes/o/main".into(),
                target: Oid::from_bytes([1; Oid::LEN]),
            }],
            ..RemoteSnapshot::default()
        };
        assert!(a.differences(&a).is_empty());
        let mut b = a.clone();
        b.refs[0].target = Oid::from_bytes([2; Oid::LEN]);
        b.push_default = true;
        assert_eq!(a.differences(&b).len(), 2, "{:?}", a.differences(&b));
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["config"][0]["value"], "https://u:***@h/r");
        assert_eq!(a.url(), Some("https://u:p@h/r"));
    }
}
