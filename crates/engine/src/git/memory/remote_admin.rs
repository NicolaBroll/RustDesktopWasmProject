//! Gestione dei remoti sul [`MemoryBackend`] (`remote.list`, `remote.add`, `remote.remove`,
//! S27).
//!
//! La "configurazione" di un remoto in memoria è il suo URL (`remote.<nome>.url`), il refspec
//! (`remote.<nome>.fetch`, quello di default se non impostato con
//! [`MemoryBackend::set_config`]) e le altre chiavi `remote.<nome>.*` di `set_config`; i
//! dipendenti sono gli upstream ([`MemoryBackend::set_upstream`]), le chiavi
//! `branch.<b>.pushRemote` e `remote.pushDefault`. Il repo in memoria non ha ref simbolici:
//! `head` è sempre `None`.
//!
//! Il remoto in memoria è anche il "server" (i suoi branch e commit): togliendolo, il server
//! resta da parte per URL, e un remoto aggiunto o rimesso con lo stesso URL lo ritrova (così un
//! fetch dopo `remote.add` di un URL già visto scarica i suoi branch).

use super::remote::MemoryRemote;
use super::{HEADS, MemoryBackend, REMOTES};
use crate::EngineError;
use crate::git::{ConfigEntry, RemoteRef, RemoteSnapshot, owned_by, split_branch_key};

impl MemoryBackend {
    /// `remotes()` continua a elencare `name` anche dopo la rimozione, come un remoto definito
    /// anche fuori dal file del repo che sfugge al controllo prima della conferma.
    pub fn keep_remote_listed(&mut self, name: &str) {
        self.listed_anyway.insert(name.to_owned());
    }

    /// Le prossime `n` chiamate a `restore_refs` riescono, poi falliscono tutte.
    pub fn fail_restore_refs_after(&mut self, n: usize) {
        self.restore_budget = Some(n);
    }

    /// Simula voci `remote.<name>.*` definite fuori dal file del repo (configurazione globale,
    /// `include`): [`GitBackend::remote_config_elsewhere`](crate::GitBackend) le restituisce.
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
            return Err(EngineError::NotAllowed(format!(
                "esiste già un remoto `{name}`, niente modificato"
            )));
        }
        let mut remote = self.detached_remotes.remove(url).unwrap_or_default();
        remote.url = url.to_owned();
        self.remotes.insert(name.to_owned(), remote);
        Ok(())
    }

    pub(super) fn drop_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
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

    pub(super) fn put_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
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
                        None => warnings.push(format!(
                            "upstream di `{branch}` senza `merge`: non riscritto nel repo in \
                             memoria"
                        )),
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
