//! Pull e push sul [`MemoryBackend`] (`remote.pull`, `remote.push`, S26), con gli stessi
//! controlli del `CliBackend`.
//!
//! Il merge in memoria lavora file per file (un file cambiato in modo diverso dai due lati è un
//! conflitto, anche se git saprebbe unire le righe); il rebase riapplica i commit locali senza
//! merge, dal più vecchio, saltando quelli che non cambiano più niente. Un conflitto non cambia
//! niente (come il `CliBackend`, che annulla merge e rebase). L'undo di un pull del branch
//! attivo ([`MemoryBackend::reset_keep`]) aggiorna indice e worktree come il checkout, con gli
//! stessi rifiuti. Il push copia i commit nel remoto in memoria e aggiorna il branch remoto
//! locale; [`MemoryBackend::fail_pre_push`] simula un hook `pre-push` che rifiuta.

use super::commit::{Tree, tree_changes};
use super::{HEADS, HeadRef, MemoryBackend, REMOTES, RefEntry};
use crate::git::{
    BACKUP_PREFIX, LogQuery, PullMode, PullOperation, PullOutcome, PullRequest, PullSettings,
    PushKind, PushOutcome, PushRejection, PushRequest, RefRestore, RestoreOutcome, UpstreamConfig,
    check_pull_state, short, split_message,
};
use crate::{EngineError, FetchFailure, GitBackend, JobContext, Progress};
use project_domain::{Commit, Oid, Signature};
use std::collections::BTreeSet;

/// Il merge a tre vie di `ours` e `theirs` da `base`, file per file; i file in conflitto se
/// cambiati in modo diverso dai due lati.
fn merge_trees(base: &Tree, ours: &Tree, theirs: &Tree) -> Result<Tree, Vec<String>> {
    let mut paths: Vec<_> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();
    paths.sort();
    paths.dedup();
    let mut out = Tree::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let (b, o, t) = (base.get(path), ours.get(path), theirs.get(path));
        let result = if o == t || t == b {
            o
        } else if o == b {
            t
        } else {
            conflicts.push(path.to_string());
            continue;
        };
        if let Some(entry) = result {
            out.insert(path.clone(), *entry);
        }
    }
    if conflicts.is_empty() {
        Ok(out)
    } else {
        Err(conflicts)
    }
}

impl MemoryBackend {
    /// La configurazione di pull (`pull.rebase`, `pull.ff`) per tutti i branch.
    pub fn set_pull_settings(&mut self, settings: PullSettings) {
        self.pull_settings = settings;
    }

    /// I valori della chiave di configurazione `key` (es. `remote.pushDefault`,
    /// `branch.<nome>.pushRemote`, `remote.<nome>.mirror`); vuoto la toglie.
    pub fn set_config(&mut self, key: &str, values: &[&str]) {
        if values.is_empty() {
            self.config.remove(key);
        } else {
            self.config.insert(
                key.to_owned(),
                values.iter().map(|v| (*v).to_owned()).collect(),
            );
        }
    }

    /// [`GitBackend::config_values`]: le chiavi impostate con [`MemoryBackend::set_config`];
    /// `remote.<nome>.url` è l'URL del remoto ([`MemoryBackend::add_remote`]) e
    /// `remote.<nome>.fetch` di un remoto senza valori ha il refspec di default
    /// (`+refs/heads/*:refs/remotes/<nome>/*`).
    pub(super) fn config_of(&self, key: &str) -> Vec<String> {
        if let Some(values) = self.config.get(key) {
            return values.clone();
        }
        if let Some(remote) = key
            .strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".url"))
            .and_then(|name| self.remotes.get(name))
        {
            return vec![remote.url.clone()];
        }
        key.strip_prefix("remote.")
            .and_then(|rest| rest.strip_suffix(".fetch"))
            .filter(|name| self.remotes.contains_key(*name))
            .map(|name| vec![format!("+refs/heads/*:{REMOTES}{name}/*")])
            .unwrap_or_default()
    }

    /// Un hook `pre-push` che rifiuta ogni push con `message` (`None`: nessun hook).
    pub fn fail_pre_push(&mut self, message: Option<&str>) {
        self.pre_push_failure = message.map(str::to_owned);
    }

    pub(super) fn upstream_config(&self, branch: &str) -> Option<UpstreamConfig> {
        let tracking = self.upstreams.get(&format!("{HEADS}{branch}"))?;
        let rest = tracking.strip_prefix(REMOTES)?;
        // Il remoto con il nome più lungo che è un prefisso (`fork/a` prima di `fork`).
        let remote = self
            .remotes
            .keys()
            .filter(|r| {
                rest.strip_prefix(r.as_str())
                    .is_some_and(|x| x.starts_with('/'))
            })
            .max_by_key(|r| r.len());
        let (remote, merge) = match remote {
            Some(r) => (r.clone(), &rest[r.len() + 1..]),
            None => {
                let (r, b) = rest.split_once('/')?;
                (r.to_owned(), b)
            }
        };
        Some(UpstreamConfig {
            remote,
            merge: format!("{HEADS}{merge}"),
            tracking: Some(tracking.clone()),
        })
    }

    pub(super) fn commits_missing(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError> {
        let excluded: BTreeSet<Oid> = exclude.iter().flat_map(|e| self.reachable(*e)).collect();
        Ok(self
            .log(&LogQuery::all().from_rev(tip.to_string()))?
            .into_iter()
            .filter(|c| !excluded.contains(&c.id))
            .take(limit)
            .collect())
    }

    /// Il miglior antenato comune di `a` e `b` (il più recente, se sono più d'uno).
    fn merge_base(&self, a: Oid, b: Oid) -> Option<Oid> {
        let common: BTreeSet<Oid> = self
            .reachable(a)
            .intersection(&self.reachable(b))
            .copied()
            .collect();
        common
            .iter()
            .copied()
            .filter(|c| {
                !common
                    .iter()
                    .any(|d| d != c && self.reachable(*d).contains(c))
            })
            .max_by_key(|c| (self.commits[c].committer.time, *c))
    }

    /// Un commit nuovo con l'albero `tree` (i file cambiati sono calcolati rispetto al primo
    /// genitore); autore `author`, o chi committa.
    fn make_commit(
        &mut self,
        parents: Vec<Oid>,
        tree: &Tree,
        author: Option<Signature>,
        summary: String,
        body: String,
    ) -> Result<Oid, EngineError> {
        let (name, email) = self.identity.clone().ok_or_else(|| EngineError::Git {
            command: "merge".to_owned(),
            stderr: "Committer identity unknown: nessuna identità nel repo in memoria \
                     (`MemoryBackend::set_identity`)"
                .to_owned(),
        })?;
        let now = Signature {
            name,
            email,
            time: self.next_time(),
            offset_min: 0,
        };
        let message = if body.is_empty() {
            summary.clone()
        } else {
            format!("{summary}\n\n{body}")
        };
        let id = self.new_commit_id(&parents, &message, tree, now.time);
        let first = parents
            .first()
            .map(|p| self.tree_at(*p))
            .unwrap_or_default();
        self.insert_commit(Commit {
            id,
            parents,
            author: author.unwrap_or_else(|| now.clone()),
            committer: now,
            summary,
            body,
        });
        self.set_changes(id, tree_changes(&first, tree));
        Ok(id)
    }

    fn pull_conflict(
        request: &PullRequest,
        operation: PullOperation,
        files: Vec<String>,
    ) -> EngineError {
        let message = files
            .iter()
            .map(|f| format!("CONFLICT (content): Merge conflict in {f}"))
            .collect::<Vec<_>>()
            .join("\n");
        EngineError::PullConflict {
            branch: request.branch.clone(),
            upstream: request
                .upstream_name
                .strip_prefix(REMOTES)
                .unwrap_or(&request.upstream_name)
                .to_owned(),
            operation,
            files,
            message,
        }
    }

    pub(super) fn pull_upstream(
        &mut self,
        request: &PullRequest,
    ) -> Result<PullOutcome, EngineError> {
        check_pull_state(&self.status()?, &request.mode)?;
        let full = format!("{HEADS}{}", request.branch);
        self.check_value(&full, &request.expected)?;
        let on_head = self.head == HeadRef::Branch(request.branch.clone());
        if on_head != request.current {
            return Err(EngineError::NotAllowed(
                "HEAD è cambiata mentre si preparava il pull: niente modificato, riprovare"
                    .to_owned(),
            ));
        }
        if !request.current && request.mode != PullMode::FastForward {
            return Err(EngineError::NotAllowed(format!(
                "`{}` non è il branch attivo: si può solo fare fast-forward, niente modificato",
                request.branch
            )));
        }
        // Il nuovo valore del branch, calcolato su una copia: un conflitto non cambia niente.
        let mut work = self.clone();
        let target = match request.mode {
            PullMode::FastForward => request.upstream,
            PullMode::Merge { .. } => {
                let base = work
                    .merge_base(request.expected, request.upstream)
                    .map(|b| work.tree_at(b))
                    .unwrap_or_default();
                let merged = merge_trees(
                    &base,
                    &work.tree_at(request.expected),
                    &work.tree_at(request.upstream),
                )
                .map_err(|files| Self::pull_conflict(request, PullOperation::Merge, files))?;
                let (summary, body) = split_message(&request.message);
                work.make_commit(
                    vec![request.expected, request.upstream],
                    &merged,
                    None,
                    summary,
                    body,
                )?
            }
            PullMode::Rebase { .. } => {
                let theirs = work.reachable(request.upstream);
                let mut todo: Vec<Commit> = work
                    .log(&LogQuery::all().from_rev(request.expected.to_string()))?
                    .into_iter()
                    .filter(|c| !theirs.contains(&c.id) && c.parents.len() <= 1)
                    .collect();
                todo.reverse();
                // Il rebase passa per gli alberi dei commit da riapplicare: un file che uno di
                // loro aggiunge (anche se un commit successivo lo toglie) non deve essere al
                // posto di un non tracciato o ignorato, che verrebbe scritto e poi cancellato.
                let mut hit: Vec<String> = Vec::new();
                for commit in &todo {
                    let (_, files) = work.switch_plan(&work.tree_at(commit.id));
                    for file in files {
                        if !hit.contains(&file) {
                            hit.push(file);
                        }
                    }
                }
                if !hit.is_empty() {
                    return Err(EngineError::NotAllowed(format!(
                        "il rebase passerebbe per file che ora esistono nel worktree come non \
                         tracciati o ignorati ({}): git li sovrascriverebbe. Il branch non è \
                         stato toccato; spostarli o rinominarli e riprovare",
                        hit.iter()
                            .map(|f| format!("`{f}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                }
                let mut tip = request.upstream;
                let mut tree = work.tree_at(tip);
                for commit in todo {
                    let base = commit
                        .parents
                        .first()
                        .map(|p| work.tree_at(*p))
                        .unwrap_or_default();
                    let mine = work.tree_at(commit.id);
                    let merged = merge_trees(&base, &tree, &mine).map_err(|files| {
                        Self::pull_conflict(request, PullOperation::Rebase, files)
                    })?;
                    // Già nell'upstream: git lo salta.
                    if merged == tree {
                        continue;
                    }
                    tip = work.make_commit(
                        vec![tip],
                        &merged,
                        Some(commit.author.clone()),
                        commit.summary.clone(),
                        commit.body.clone(),
                    )?;
                    tree = merged;
                }
                tip
            }
        };
        if request.current {
            let new_tree = work.tree_at(target);
            let (differing, files) = work.switch_plan(&new_tree);
            if !files.is_empty() {
                return Err(EngineError::NotAllowed(format!(
                    "il pull sovrascriverebbe modifiche locali o file non tracciati ({}): \
                     committarli, metterli da parte o spostarli e riprovare; il branch non è \
                     stato toccato",
                    files
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            let index = work.switch_index(&differing, &new_tree);
            work.set_ref(full, target, None);
            work.rebase_entries(&index);
        } else {
            work.set_ref(full, target, None);
        }
        let prefix = format!("{BACKUP_PREFIX}pull-");
        let n = work
            .commit_backups
            .iter()
            .filter(|(b, _)| b.starts_with(&prefix))
            .count()
            + 1;
        let backup = format!("{prefix}{n}");
        work.commit_backups.push((backup.clone(), request.expected));
        *self = work;
        Ok(PullOutcome {
            after: target,
            backup,
            warnings: Vec::new(),
        })
    }

    /// [`GitBackend::reset_branch`]: undo e redo di un pull del branch attivo.
    pub(super) fn reset_keep(
        &mut self,
        request: &RefRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        request.check(self.operation)?;
        let (moved, target) = match (request.refs.as_slice(), &request.head_expected) {
            ([r], crate::HeadState::Branch(b)) if r.name == *b && r.target.is_some() => {
                (r.name.clone(), r.target.expect("controllato"))
            }
            _ => {
                return Err(EngineError::NotAllowed(format!(
                    "`{}`: richiesta non valida per il branch attivo, niente modificato",
                    request.command
                )));
            }
        };
        let head = self.head_state();
        let conflicts =
            request.conflicts(&head, |name| Ok(self.refs.get(name).map(|r| r.target)))?;
        if !conflicts.is_empty() {
            return Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            });
        }
        let new_tree = self.tree_at(target);
        let (differing, files) = self.switch_plan(&new_tree);
        if !files.is_empty() {
            return Err(EngineError::NotAllowed(format!(
                "`{}` riporterebbe `{moved}` a {} ma sovrascriverebbe modifiche locali o file \
                 non tracciati ({}): niente modificato; committarli o metterli da parte e \
                 riprovare",
                request.command,
                short(&target),
                files.join(", ")
            )));
        }
        let mut work = self.clone();
        let index = work.switch_index(&differing, &new_tree);
        let backups = work.restore(request)?;
        work.rebase_entries(&index);
        *self = work;
        Ok(RestoreOutcome {
            backups,
            warnings: Vec::new(),
        })
    }

    pub(super) fn push_branch(
        &mut self,
        request: &PushRequest,
        job: &JobContext,
    ) -> Result<PushOutcome, EngineError> {
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
        let reject = |reason, message: &str| EngineError::PushRejected {
            remote: request.remote.clone(),
            branch: request.remote_branch.clone(),
            reason,
            message: message.to_owned(),
        };
        if let Some(message) = &self.pre_push_failure {
            return Err(reject(PushRejection::Hook, message));
        }
        if job.is_cancelled() {
            return Err(EngineError::Cancelled {
                command: "remote.push",
            });
        }
        let local = self
            .refs
            .get(&format!("{HEADS}{}", request.branch))
            .map(|r| r.target)
            .ok_or_else(|| {
                EngineError::NotAllowed(format!("non esiste un branch `{}`", request.branch))
            })?;
        let theirs = remote.branches.get(&request.remote_branch).copied();
        let url = remote.url.clone();
        let mine = self.reachable(local);
        let kind = match (request.lease, theirs) {
            (_, Some(t)) if t == local => PushKind::UpToDate,
            (Some(expected), t) if t != expected => {
                return Err(reject(PushRejection::StaleLease, "[rejected] (stale info)"));
            }
            (Some(_), Some(t)) if !mine.contains(&t) => PushKind::Forced,
            (None, Some(t)) if !mine.contains(&t) => {
                let why = if self.commits.contains_key(&t) {
                    "[rejected] (non-fast-forward)"
                } else {
                    "[rejected] (fetch first)"
                };
                return Err(reject(PushRejection::NonFastForward, why));
            }
            (_, Some(_)) => PushKind::FastForward,
            (_, None) => PushKind::New,
        };
        let backup = request.backup.map(|oid| {
            let prefix = format!("{BACKUP_PREFIX}push-");
            let n = self
                .commit_backups
                .iter()
                .filter(|(b, _)| b.starts_with(&prefix))
                .count()
                + 1;
            let name = format!("{prefix}{n}");
            self.commit_backups.push((name.clone(), oid));
            name
        });
        job.report(&Progress {
            phase: "Writing objects".to_owned(),
            remote: false,
            percent: Some(100),
            current: Some(1),
            total: Some(1),
            done: true,
            text: "Writing objects: 100% (1/1), done.".to_owned(),
        });
        let commits: Vec<Commit> = mine
            .iter()
            .filter_map(|id| self.commits.get(id).cloned())
            .collect();
        let remote = self
            .remotes
            .get_mut(&request.remote)
            .expect("controllato sopra");
        for commit in commits {
            remote.commits.insert(commit.id, commit);
        }
        remote.branches.insert(request.remote_branch.clone(), local);
        let tracking = format!("{REMOTES}{}/{}", request.remote, request.remote_branch);
        if kind != PushKind::UpToDate {
            self.refs.insert(
                tracking.clone(),
                RefEntry {
                    target: local,
                    tag_object: None,
                },
            );
        }
        if request.set_upstream {
            self.upstreams
                .insert(format!("{HEADS}{}", request.branch), tracking);
        }
        let summary = match (kind, theirs) {
            (PushKind::New, _) => "[new branch]".to_owned(),
            (PushKind::UpToDate, _) => "[up to date]".to_owned(),
            (PushKind::Forced, Some(t)) => {
                format!("{}...{} (forced update)", short(&t), short(&local))
            }
            (_, Some(t)) => format!("{}..{}", short(&t), short(&local)),
            (_, None) => String::new(),
        };
        let mut messages = vec![format!("To {url}")];
        if request.set_upstream {
            messages.push(format!(
                "branch '{}' set up to track '{}/{}'.",
                request.branch, request.remote, request.remote_branch
            ));
        }
        Ok(PushOutcome {
            kind,
            summary,
            messages,
            backup,
        })
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
