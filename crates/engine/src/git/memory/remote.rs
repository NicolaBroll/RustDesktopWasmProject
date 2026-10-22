//! Remoti sul [`MemoryBackend`] (`remote.fetch`, S25).
//!
//! Un remoto in memoria ha un nome, un URL (solo descrittivo), i suoi branch e i commit che ha
//! in più rispetto al repo. Il fetch copia nel repo i commit raggiungibili dai branch scaricati
//! e aggiorna `refs/remotes/<remote>/<branch>`; come `git fetch` senza `--prune`, un branch
//! sparito dal remoto resta. Un errore preparato con [`MemoryBackend::fail_fetch`] simula le
//! credenziali rifiutate o un remoto non raggiungibile. L'avanzamento è finto ma con la stessa
//! forma di quello di git (una fase `Receiving objects` da 0 a 100%).

use super::{MemoryBackend, REMOTES, RefEntry};
use crate::git::{FetchOutcome, FetchRequest, short};
use crate::{EngineError, JobContext, Progress};
use project_domain::{Commit, Oid};
use std::collections::BTreeMap;

/// Errore che il prossimo fetch da un remoto in memoria restituisce
/// ([`MemoryBackend::fail_fetch`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchFailure {
    /// [`EngineError::Auth`] con questo messaggio.
    Auth(String),
    /// [`EngineError::RemoteUnreachable`] con questo messaggio.
    Unreachable(String),
}

/// Un remoto del [`MemoryBackend`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MemoryRemote {
    pub(super) url: String,
    /// Nome breve del branch sul remoto → commit.
    pub(super) branches: BTreeMap<String, Oid>,
    /// I commit del remoto (anche quelli che il repo ha già).
    pub(super) commits: BTreeMap<Oid, Commit>,
    pub(super) failure: Option<FetchFailure>,
}

impl MemoryBackend {
    /// Aggiunge il remoto `name` con `url` (o ne cambia l'URL). Senza branch finché non si
    /// chiama [`MemoryBackend::set_branch_on_remote`].
    pub fn add_remote(&mut self, name: &str, url: &str) {
        self.remotes.entry(name.to_owned()).or_default().url = url.to_owned();
    }

    /// L'URL del remoto `name`, se c'è.
    pub fn remote_url(&self, name: &str) -> Option<&str> {
        self.remotes.get(name).map(|r| r.url.as_str())
    }

    /// Sul remoto `remote` (che deve esistere) il branch `branch` (nome breve) punta a
    /// `target`. Il commit va dato con [`MemoryBackend::insert_remote_commit`], se il repo non
    /// lo ha già.
    pub fn set_branch_on_remote(&mut self, remote: &str, branch: &str, target: Oid) {
        self.remote_mut(remote)
            .branches
            .insert(branch.to_owned(), target);
    }

    /// Il commit del branch `branch` sul remoto `remote` (come lo vede il remoto, non il branch
    /// remoto locale), se c'è.
    pub fn branch_on_remote(&self, remote: &str, branch: &str) -> Option<Oid> {
        self.remotes.get(remote)?.branches.get(branch).copied()
    }

    /// Toglie il branch `branch` dal remoto `remote`.
    pub fn remove_branch_on_remote(&mut self, remote: &str, branch: &str) {
        self.remote_mut(remote).branches.remove(branch);
    }

    /// Un commit che il remoto `remote` ha (e il repo forse no): entra nel repo con il fetch
    /// di un branch che lo raggiunge.
    pub fn insert_remote_commit(&mut self, remote: &str, commit: Commit) {
        self.remote_mut(remote).commits.insert(commit.id, commit);
    }

    /// I fetch e i push da `remote` falliscono con `failure` (`None`: tornano a riuscire).
    pub fn fail_fetch(&mut self, remote: &str, failure: Option<FetchFailure>) {
        self.remote_mut(remote).failure = failure;
    }

    fn remote_mut(&mut self, name: &str) -> &mut MemoryRemote {
        self.remotes
            .get_mut(name)
            .unwrap_or_else(|| panic!("remoto `{name}` assente: aggiungerlo con `add_remote`"))
    }

    pub(super) fn remote_names(&self) -> Vec<String> {
        self.remotes.keys().cloned().collect()
    }

    pub(super) fn fetch_remote(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
        let remote = self.remotes.get(&request.remote).ok_or_else(|| {
            EngineError::NotAllowed(format!("non esiste un remoto `{}`", request.remote))
        })?;
        match &remote.failure {
            Some(FetchFailure::Auth(message)) => {
                return Err(EngineError::Auth {
                    remote: request.remote.clone(),
                    message: message.clone(),
                });
            }
            Some(FetchFailure::Unreachable(message)) => {
                return Err(EngineError::RemoteUnreachable {
                    remote: request.remote.clone(),
                    message: message.clone(),
                });
            }
            None => {}
        }
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "remote.fetch",
            });
        }
        let branches: Vec<(String, Oid)> = match &request.branch {
            Some(branch) => match remote.branches.get(branch) {
                Some(target) => vec![(branch.clone(), *target)],
                None => {
                    return Err(EngineError::NotAllowed(format!(
                        "il remoto `{}` non ha il branch `{branch}`, niente scaricato",
                        request.remote
                    )));
                }
            },
            None => remote
                .branches
                .iter()
                .map(|(b, t)| (b.clone(), *t))
                .collect(),
        };

        // Commit raggiungibili dai branch scaricati che il repo non ha ancora.
        let mut new = Vec::new();
        let mut stack: Vec<Oid> = branches.iter().map(|(_, t)| *t).collect();
        while let Some(id) = stack.pop() {
            if self.commits.contains_key(&id) || new.iter().any(|c: &Commit| c.id == id) {
                continue;
            }
            if let Some(commit) = remote.commits.get(&id) {
                stack.extend(commit.parents.iter().copied());
                new.push(commit.clone());
            }
        }
        let total = new.len() as u64;
        for current in 0..=total {
            let percent = (current * 100).checked_div(total).unwrap_or(100);
            let done = current == total;
            let mut text = format!("Receiving objects: {percent:>3}% ({current}/{total})");
            if done {
                text.push_str(", done.");
            }
            job.report(&Progress {
                phase: "Receiving objects".to_owned(),
                remote: false,
                percent: Some(u8::try_from(percent).unwrap_or(100)),
                current: Some(current),
                total: Some(total),
                done,
                text,
            });
        }

        let url = remote.url.clone();
        let remote_name = request.remote.clone();
        let mut messages = vec![format!("From {url}")];
        for commit in new {
            self.commits.insert(commit.id, commit);
        }
        for (branch, target) in branches {
            let full = format!("{REMOTES}{remote_name}/{branch}");
            let line = match self.refs.get(&full) {
                None => Some(format!(
                    " * [new branch]      {branch} -> {remote_name}/{branch}"
                )),
                Some(entry) if entry.target != target => Some(format!(
                    "   {}..{}  {branch} -> {remote_name}/{branch}",
                    short(&entry.target),
                    short(&target)
                )),
                Some(_) => None,
            };
            messages.extend(line);
            self.refs.insert(
                full,
                RefEntry {
                    target,
                    tag_object: None,
                },
            );
        }
        if messages.len() == 1 {
            // Niente di nuovo: git non scrive nemmeno `From`.
            messages.clear();
        }
        Ok(FetchOutcome { messages })
    }
}

#[cfg(test)]
#[path = "remote_tests.rs"]
mod tests;
