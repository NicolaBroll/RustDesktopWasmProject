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
    /// `include`): [`GitBackend::remote_config_elsewhere`](crate::GitBackend) returns them.
    pub fn set_remote_elsewhere(&mut self, name: &str, entries: &[&str]) {
        self.remote_elsewhere.insert(
            name.to_owned(),
            entries.iter().map(|e| (*e).to_owned()).collect(),
        );
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
