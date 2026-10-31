//! Remote management on the [`MemoryBackend`] (`remote.list`, `remote.add`, `remote.remove`,
//! S27).
//!
//! The "configuration" of an in-memory remote is its URL (`remote.<name>.url`), the refspec
//! (`remote.<name>.fetch`, the default one if not set with
//! [`MemoryBackend::set_config`]) and the other `remote.<name>.*` keys of `set_config`; the
//! dependents are the upstreams ([`MemoryBackend::set_upstream`]), the keys
//! `branch.<b>.pushRemote` and `remote.pushDefault`. The in-memory repo has no symbolic refs:
//! `head` is always `None`.
//!
//! The in-memory remote is also the "server" (its branches and commits): when it is removed, the server
//! is set aside by URL, and a remote added or put back with the same URL finds it again (so a
//! fetch after `remote.add` of an already seen URL downloads its branches).

use super::remote::MemoryRemote;
use super::{HEADS, MemoryBackend, REMOTES};
use crate::git::{ConfigEntry, RemoteRef, RemoteSnapshot, owned_by, split_branch_key};
use crate::{EngineError, Message};

impl MemoryBackend {
    /// `remotes()` keeps listing `name` even after the removal, like a remote defined
    /// also outside the repo file that escapes the check before the confirmation.
    pub fn keep_remote_listed(&mut self, name: &str) {
        self.listed_anyway.insert(name.to_owned());
    }

    /// The next `n` calls to `restore_refs` succeed, then they all fail.
    pub fn fail_restore_refs_after(&mut self, n: usize) {
        self.restore_budget = Some(n);
    }

    /// Simulates `remote.<name>.*` entries defined outside the repo file (global configuration,
    /// `include`): [`GitRemotes::remote_config_elsewhere`](crate::GitRemotes::remote_config_elsewhere) returns them.
    pub fn set_remote_elsewhere(&mut self, name: &str, entries: &[&str]) {
        self.remote_elsewhere.insert(
            name.to_owned(),
            entries.iter().map(|e| (*e).to_owned()).collect(),
        );
    }

    /// The next `rename_remote` call (only that one) fails: before writing anything
    /// (`Some(false)`), or after moving the configuration but not the remote-tracking branches
    /// (`Some(true)`, like `git remote rename` stopped by a locked ref); `None`: it succeeds.
    pub fn fail_remote_rename(&mut self, midway: Option<bool>) {
        self.remote_rename_failure = midway;
    }

    /// [`GitRemotes::set_remote_url`](crate::GitRemotes::set_remote_url): the fetch URL is the
    /// remote's (the "server" stays the same), the push URL the `remote.<name>.pushurl` key.
    /// Fails like `git config` with [`MemoryBackend::fail_config_writes`].
    pub(super) fn change_remote_url(
        &mut self,
        name: &str,
        push: bool,
        url: Option<&str>,
    ) -> Result<(), EngineError> {
        if !self.remotes.contains_key(name) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-remote-remove-unknown",
                name = name.escape_debug(),
                known = msg!("engine-remote-none-configured")
            )));
        }
        if let Some(message) = &self.config_write_failure {
            return Err(EngineError::Git {
                command: "remote".to_owned(),
                stderr: message.clone(),
            });
        }
        let key = format!("remote.{name}.pushurl");
        match (push, url) {
            (false, Some(url)) => {
                if let Some(remote) = self.remotes.get_mut(name) {
                    url.clone_into(&mut remote.url);
                }
            }
            (false, None) => {
                return Err(EngineError::NotAllowed(msg!(
                    "engine-remote-memory-needs-url",
                    name = name
                )));
            }
            (true, Some(url)) => {
                self.config.insert(key, vec![url.to_owned()]);
            }
            (true, None) => {
                self.config.remove(&key);
            }
        }
        Ok(())
    }

    /// [`GitRemotes::rename_remote`](crate::GitRemotes::rename_remote), like `git remote rename`:
    /// remote, `remote.<from>.*` keys, remote-tracking branches, upstreams, `pushRemote` and
    /// `remote.pushDefault`.
    pub(super) fn rename_memory_remote(
        &mut self,
        from: &str,
        to: &str,
    ) -> Result<Vec<Message>, EngineError> {
        if self.remotes.contains_key(to) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-remote-exists",
                name = to
            )));
        }
        let failure = self.remote_rename_failure.take();
        if failure == Some(false) {
            return Err(EngineError::Git {
                command: "remote".to_owned(),
                stderr: "error: could not lock config file .git/config".to_owned(),
            });
        }
        let Some(remote) = self.remotes.remove(from) else {
            return Err(EngineError::NotAllowed(msg!(
                "engine-remote-remove-unknown",
                name = from.escape_debug(),
                known = msg!("engine-remote-none-configured")
            )));
        };
        let remotes: Vec<String> = self
            .remotes
            .keys()
            .cloned()
            .chain([from.to_owned()])
            .collect();
        self.remotes.insert(to.to_owned(), remote);
        let old = format!("remote.{from}.");
        let moved: Vec<String> = self
            .config
            .keys()
            .filter(|k| k.strip_prefix(&old).is_some_and(|v| !v.contains('.')))
            .cloned()
            .collect();
        for key in moved {
            if let Some(values) = self.config.remove(&key) {
                let var = &key[old.len()..];
                self.config.insert(format!("remote.{to}.{var}"), values);
            }
        }
        if failure == Some(true) {
            return Err(EngineError::Git {
                command: "remote".to_owned(),
                stderr: format!("error: renaming 'refs/remotes/{from}/main' failed"),
            });
        }
        for (key, values) in &mut self.config {
            let dependent = key == "remote.pushDefault"
                || split_branch_key(key).is_some_and(|(_, var)| var == "pushremote");
            if dependent {
                for v in values.iter_mut().filter(|v| v.as_str() == from) {
                    to.clone_into(v);
                }
            }
        }
        let old_prefix = format!("{REMOTES}{from}/");
        let new_prefix = format!("{REMOTES}{to}/");
        let refs: Vec<String> = self
            .refs
            .keys()
            .filter(|r| owned_by(from, &remotes, r))
            .cloned()
            .collect();
        for name in refs {
            if let Some(entry) = self.refs.remove(&name) {
                let rest = &name[old_prefix.len()..];
                self.refs.insert(format!("{new_prefix}{rest}"), entry);
            }
        }
        for tracking in self.upstreams.values_mut() {
            if owned_by(from, &remotes, tracking) {
                *tracking = format!("{new_prefix}{}", &tracking[old_prefix.len()..]);
            }
        }
        Ok(Vec::new())
    }

    pub(super) fn snapshot_of(&self, name: &str) -> RemoteSnapshot {
        let mut config = Vec::new();
        let prefix = format!("remote.{name}.");
        if let Some(remote) = self.remotes.get(name) {
            config.push(ConfigEntry {
                key: format!("{prefix}url"),
                value: Some(remote.url.clone()),
            });
            for value in self.config_of(&format!("{prefix}fetch")) {
                config.push(ConfigEntry {
                    key: format!("{prefix}fetch"),
                    value: Some(value),
                });
            }
        }
        for (key, values) in &self.config {
            let Some(var) = key.strip_prefix(&prefix) else {
                continue;
            };
            if var.contains('.') || var == "fetch" || var == "url" {
                continue;
            }
            for value in values {
                config.push(ConfigEntry {
                    key: key.clone(),
                    value: Some(value.clone()),
                });
            }
        }
        let remotes = self.remote_names();
        let refs = self
            .refs
            .iter()
            .filter(|(r, _)| owned_by(name, &remotes, r))
            .map(|(r, e)| RemoteRef {
                name: r.clone(),
                target: e.target,
            })
            .collect();
        let mut upstreams = Vec::new();
        for local in self.upstreams.keys() {
            let Some(branch) = local.strip_prefix(HEADS) else {
                continue;
            };
            if let Some(up) = self.upstream_config(branch)
                && up.remote == name
            {
                upstreams.push(ConfigEntry {
                    key: format!("branch.{branch}.remote"),
                    value: Some(up.remote),
                });
                upstreams.push(ConfigEntry {
                    key: format!("branch.{branch}.merge"),
                    value: Some(up.merge),
                });
            }
        }
        for (key, values) in &self.config {
            if split_branch_key(key).is_some_and(|(_, var)| var == "pushremote")
                && values.iter().any(|v| v == name)
            {
                upstreams.push(ConfigEntry {
                    key: key.clone(),
                    value: Some(name.to_owned()),
                });
            }
        }
        let push_default = self
            .config
            .get("remote.pushDefault")
            .is_some_and(|v| v.iter().any(|v| v == name));
        RemoteSnapshot {
            name: name.to_owned(),
            config,
            refs,
            head: None,
            upstreams,
            push_default,
        }
    }

    pub(super) fn create_memory_remote(
        &mut self,
        name: &str,
        url: &str,
    ) -> Result<(), EngineError> {
        if self.remotes.contains_key(name) {
            return Err(EngineError::NotAllowed(msg!(
                "engine-remote-exists",
                name = name
            )));
        }
        let mut remote = self.detached_remotes.remove(url).unwrap_or_default();
        remote.url = url.to_owned();
        self.remotes.insert(name.to_owned(), remote);
        Ok(())
    }

    pub(super) fn drop_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        let name = &snapshot.name;
        if !snapshot.config.is_empty() {
            if let Some(remote) = self.remotes.remove(name) {
                self.detached_remotes.insert(remote.url.clone(), remote);
            }
            let prefix = format!("remote.{name}.");
            self.config
                .retain(|key, _| !key.strip_prefix(&prefix).is_some_and(|v| !v.contains('.')));
        }
        for entry in &snapshot.upstreams {
            let Some((branch, var)) = split_branch_key(&entry.key) else {
                continue;
            };
            match var.as_str() {
                "remote" => {
                    self.upstreams.remove(&format!("{HEADS}{branch}"));
                }
                "pushremote" => remove_value(&mut self.config, &entry.key, name),
                _ => {}
            }
        }
        if snapshot.push_default {
            remove_value(&mut self.config, "remote.pushDefault", name);
        }
        Vec::new()
    }

    pub(super) fn put_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<Message> {
        let name = &snapshot.name;
        let url_key = format!("remote.{name}.url");
        for entry in &snapshot.config {
            let value = entry.value.clone().unwrap_or_else(|| "true".to_owned());
            if entry.key == url_key {
                let mut remote: MemoryRemote =
                    self.detached_remotes.remove(&value).unwrap_or_default();
                remote.url = value;
                self.remotes.insert(name.clone(), remote);
            } else {
                self.config
                    .entry(entry.key.clone())
                    .or_default()
                    .push(value);
            }
        }
        let mut warnings = Vec::new();
        for entry in &snapshot.upstreams {
            let Some((branch, var)) = split_branch_key(&entry.key) else {
                continue;
            };
            match var.as_str() {
                "remote" => {
                    let merge = snapshot
                        .upstreams
                        .iter()
                        .find(|e| e.key == format!("branch.{branch}.merge"))
                        .and_then(|e| e.value.as_deref());
                    match merge {
                        Some(merge) => {
                            let short = merge.strip_prefix(HEADS).unwrap_or(merge);
                            self.upstreams.insert(
                                format!("{HEADS}{branch}"),
                                format!("{REMOTES}{name}/{short}"),
                            );
                        }
                        None => {
                            warnings.push(msg!("engine-remote-memory-no-merge", branch = branch))
                        }
                    }
                }
                "pushremote" => self
                    .config
                    .entry(entry.key.clone())
                    .or_default()
                    .push(name.clone()),
                _ => {}
            }
        }
        if snapshot.push_default {
            self.config
                .entry("remote.pushDefault".to_owned())
                .or_default()
                .push(name.clone());
        }
        warnings
    }
}

fn remove_value(
    config: &mut std::collections::BTreeMap<String, Vec<String>>,
    key: &str,
    value: &str,
) {
    if let Some(values) = config.get_mut(key) {
        values.retain(|v| v != value);
        if values.is_empty() {
            config.remove(key);
        }
    }
}

#[cfg(test)]
#[path = "remote_admin_tests.rs"]
mod tests;
