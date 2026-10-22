//! Repository finto in memoria: per i test dei comandi e per la versione web.

use super::{GitBackend, LogQuery, RepoOpener};
use crate::EngineError;
use project_domain::{
    Commit, Head, Oid, Operation, RefInfo, RefKind, Status, StatusEntry, Upstream,
};
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

const HEADS: &str = "refs/heads/";
const REMOTES: &str = "refs/remotes/";
const TAGS: &str = "refs/tags/";

/// Dove punta HEAD: il branch è tenuto per nome, così segue i commit del branch.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HeadRef {
    /// Nome breve del branch, anche se non esiste ancora (repo vuoto).
    Branch(String),
    Detached(Oid),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RefEntry {
    target: Oid,
    tag_object: Option<Oid>,
}

/// Repository costruito in codice, senza processi né file system.
///
/// Si parte da [`MemoryBackend::new`] (repo vuoto su `main`) o da [`MemoryBackend::sample`]
/// (repo di esempio) e si aggiungono commit e ref con i metodi `insert_commit`, `set_*`.
/// Ahead/behind degli upstream sono calcolati dal grafo dei commit a ogni lettura.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryBackend {
    commits: BTreeMap<Oid, Commit>,
    /// Nome completo → destinazione; l'ordine della mappa è quello di `git for-each-ref`.
    refs: BTreeMap<String, RefEntry>,
    /// Branch locale → branch remoto, entrambi nomi completi.
    upstreams: BTreeMap<String, String>,
    head: HeadRef,
    entries: Vec<StatusEntry>,
    operation: Option<Operation>,
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryBackend {
    /// Repo vuoto: HEAD su `main`, che non ha ancora commit; worktree pulito.
    pub fn new() -> Self {
        Self {
            commits: BTreeMap::new(),
            refs: BTreeMap::new(),
            upstreams: BTreeMap::new(),
            head: HeadRef::Branch("main".to_owned()),
            entries: Vec::new(),
            operation: None,
        }
    }

    /// Id finto e deterministico: lo stesso `seed` dà sempre lo stesso id, seed diversi id
    /// diversi (in pratica). Le cifre sembrano casuali, come quelle vere.
    pub fn fake_oid(seed: u64) -> Oid {
        // splitmix64: abbastanza mescolato da non dare id simili per seed vicini.
        let mut state = seed;
        let mut bytes = [0u8; Oid::LEN];
        for chunk in bytes.chunks_mut(8) {
            state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            chunk.copy_from_slice(&z.to_be_bytes()[..chunk.len()]);
        }
        Oid::from_bytes(bytes)
    }

    /// Aggiunge (o sostituisce) un commit. I genitori assenti dal repo vengono ignorati dal log,
    /// come in un clone superficiale.
    pub fn insert_commit(&mut self, commit: Commit) {
        self.commits.insert(commit.id, commit);
    }

    /// Crea o sposta il branch locale `name` (nome breve, es. `main`).
    pub fn set_branch(&mut self, name: &str, target: Oid) {
        self.set_ref(format!("{HEADS}{name}"), target, None);
    }

    /// Crea o sposta il branch remoto `name` (nome breve, es. `origin/main`).
    pub fn set_remote_branch(&mut self, name: &str, target: Oid) {
        self.set_ref(format!("{REMOTES}{name}"), target, None);
    }

    /// Crea o sposta il tag `name` sul commit `target`. `tag_object` è l'id dell'oggetto tag
    /// per un tag annotato, `None` per uno leggero.
    pub fn set_tag(&mut self, name: &str, target: Oid, tag_object: Option<Oid>) {
        self.set_ref(format!("{TAGS}{name}"), target, tag_object);
    }

    /// Imposta lo stash (`refs/stash`) sul commit `target`.
    pub fn set_stash(&mut self, target: Oid) {
        self.set_ref("refs/stash".to_owned(), target, None);
    }

    /// Il branch locale `branch` segue il branch remoto `remote` (nomi brevi, es. `main` e
    /// `origin/main`). Il remoto può anche non esistere: l'upstream risulta sparito.
    pub fn set_upstream(&mut self, branch: &str, remote: &str) {
        self.upstreams
            .insert(format!("{HEADS}{branch}"), format!("{REMOTES}{remote}"));
    }

    /// HEAD sul branch `name` (nome breve), che può non esistere ancora.
    pub fn set_head_branch(&mut self, name: &str) {
        self.head = HeadRef::Branch(name.to_owned());
    }

    /// HEAD staccata sul commit `target`.
    pub fn detach_head(&mut self, target: Oid) {
        self.head = HeadRef::Detached(target);
    }

    /// Voci dello stato, nell'ordine in cui le darebbe git.
    pub fn set_entries(&mut self, entries: Vec<StatusEntry>) {
        self.entries = entries;
    }

    /// Operazione in corso (merge, rebase, ...), `None` se nessuna.
    pub fn set_operation(&mut self, operation: Option<Operation>) {
        self.operation = operation;
    }

    fn set_ref(&mut self, name: String, target: Oid, tag_object: Option<Oid>) {
        self.refs.insert(name, RefEntry { target, tag_object });
    }

    fn head_target(&self) -> Option<Oid> {
        match &self.head {
            HeadRef::Branch(name) => self.refs.get(&format!("{HEADS}{name}")).map(|r| r.target),
            HeadRef::Detached(oid) => Some(*oid),
        }
    }

    /// Risolve una rev come `git rev-parse`, nell'ordine di git: `HEAD`, nome completo,
    /// `refs/<rev>`, tag, branch locale, branch remoto, poi id (anche abbreviato).
    fn resolve(&self, rev: &str) -> Result<Oid, EngineError> {
        let unknown = || EngineError::UnknownRev(rev.to_owned());
        if rev == "HEAD" {
            return self.head_target().ok_or_else(unknown);
        }
        let candidates = [
            rev.to_owned(),
            format!("refs/{rev}"),
            format!("{TAGS}{rev}"),
            format!("{HEADS}{rev}"),
            format!("{REMOTES}{rev}"),
        ];
        if let Some(entry) = candidates.iter().find_map(|name| self.refs.get(name)) {
            return Ok(entry.target);
        }
        let hex = rev.to_ascii_lowercase();
        if (4..=Oid::HEX_LEN).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let mut matches = self
                .commits
                .keys()
                .filter(|id| id.to_string().starts_with(&hex));
            // Un prefisso ambiguo non si risolve, come in git.
            if let (Some(id), None) = (matches.next(), matches.next()) {
                return Ok(*id);
            }
        }
        Err(unknown())
    }

    /// Commit raggiungibili da `start` (compreso), solo quelli presenti nel repo.
    fn reachable(&self, start: Oid) -> BTreeSet<Oid> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            if let Some(commit) = self.commits.get(&id)
                && seen.insert(id)
            {
                stack.extend(commit.parents.iter().copied());
            }
        }
        seen
    }

    fn upstream_of(&self, local: &str) -> Option<Upstream> {
        let remote = self.upstreams.get(local)?;
        let (ahead, behind) = match (self.refs.get(local), self.refs.get(remote)) {
            (Some(local), Some(remote)) => {
                let ours = self.reachable(local.target);
                let theirs = self.reachable(remote.target);
                (
                    Some(count(ours.difference(&theirs).count())),
                    Some(count(theirs.difference(&ours).count())),
                )
            }
            // Il remoto non c'è più: `gone`.
            _ => (None, None),
        };
        Some(Upstream {
            name: remote.clone(),
            ahead,
            behind,
        })
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn kind_of(name: &str) -> RefKind {
    if name.starts_with(HEADS) {
        RefKind::LocalBranch
    } else if name.starts_with(REMOTES) {
        RefKind::RemoteBranch
    } else if name.starts_with(TAGS) {
        RefKind::Tag
    } else {
        RefKind::Stash
    }
}

impl GitBackend for MemoryBackend {
    fn head(&self) -> Result<Head, EngineError> {
        Ok(match &self.head {
            HeadRef::Branch(name) => Head::Branch {
                name: name.clone(),
                target: self.head_target(),
            },
            HeadRef::Detached(oid) => Head::Detached(*oid),
        })
    }

    fn refs(&self) -> Result<Vec<RefInfo>, EngineError> {
        Ok(self
            .refs
            .iter()
            .map(|(name, entry)| {
                let kind = kind_of(name);
                RefInfo {
                    name: name.clone(),
                    kind,
                    target: entry.target,
                    tag_object: entry.tag_object,
                    upstream: match kind {
                        RefKind::LocalBranch => self.upstream_of(name),
                        _ => None,
                    },
                }
            })
            .collect())
    }

    fn log(&self, query: &LogQuery) -> Result<Vec<Commit>, EngineError> {
        let start = match &query.rev {
            Some(rev) => self.resolve(rev)?,
            None => match self.head_target() {
                Some(id) => id,
                None => return Ok(Vec::new()),
            },
        };
        let wanted = query.skip.saturating_add(query.limit.unwrap_or(usize::MAX));
        let reachable = self.reachable(start);

        // Come `git log --date-order`: un commit esce solo dopo tutti i suoi figli; fra quelli
        // pronti, prima il più recente (a parità di data decide l'id, per un ordine stabile).
        let mut children: BTreeMap<Oid, usize> = reachable.iter().map(|id| (*id, 0)).collect();
        for id in &reachable {
            for parent in &self.commits[id].parents {
                if let Some(n) = children.get_mut(parent) {
                    *n += 1;
                }
            }
        }
        let key = |id: Oid| (self.commits[&id].committer.time, id);
        let mut ready: BinaryHeap<(i64, Oid)> = children
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(id, _)| key(*id))
            .collect();
        let mut out = Vec::new();
        while out.len() < wanted
            && let Some((_, id)) = ready.pop()
        {
            let commit = &self.commits[&id];
            for parent in &commit.parents {
                if let Some(n) = children.get_mut(parent) {
                    *n -= 1;
                    if *n == 0 {
                        ready.push(key(*parent));
                    }
                }
            }
            out.push(commit);
        }
        Ok(out.into_iter().skip(query.skip).cloned().collect())
    }

    fn status(&self) -> Result<Status, EngineError> {
        let upstream = match &self.head {
            HeadRef::Branch(name) => self.upstream_of(&format!("{HEADS}{name}")).map(|up| {
                // `git status` usa il nome breve.
                Upstream {
                    name: up.name.strip_prefix(REMOTES).unwrap_or(&up.name).to_owned(),
                    ..up
                }
            }),
            HeadRef::Detached(_) => None,
        };
        Ok(Status {
            head: self.head()?,
            upstream,
            entries: self.entries.clone(),
            operation: self.operation,
        })
    }
}

/// [`RepoOpener`] su repo in memoria, registrati per percorso. Ogni `open` restituisce una copia
/// indipendente del repo registrato.
#[derive(Debug, Clone, Default)]
pub struct MemoryOpener {
    repos: BTreeMap<String, MemoryBackend>,
}

impl MemoryOpener {
    /// Percorso del repo di esempio in [`MemoryOpener::sample`].
    pub const SAMPLE_PATH: &'static str = "esempio";

    /// Nessun repo registrato.
    pub fn new() -> Self {
        Self::default()
    }

    /// Solo il repo di [`MemoryBackend::sample`], al percorso [`MemoryOpener::SAMPLE_PATH`].
    /// È l'opener della versione web.
    pub fn sample() -> Self {
        let mut opener = Self::new();
        opener.insert(Self::SAMPLE_PATH, MemoryBackend::sample());
        opener
    }

    /// Registra (o sostituisce) il repo al percorso `path`.
    pub fn insert(&mut self, path: impl Into<String>, repo: MemoryBackend) {
        self.repos.insert(path.into(), repo);
    }

    /// Percorsi registrati, in ordine.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.repos.keys().map(String::as_str)
    }
}

impl RepoOpener for MemoryOpener {
    fn open(&self, path: &str) -> Result<Box<dyn GitBackend>, EngineError> {
        match self.repos.get(path) {
            Some(repo) => Ok(Box::new(repo.clone())),
            None => {
                let known: Vec<&str> = self.paths().collect();
                Err(EngineError::RepoNotFound {
                    path: path.to_owned(),
                    reason: if known.is_empty() {
                        "nessun repo in memoria".to_owned()
                    } else {
                        format!("repo in memoria disponibili: {}", known.join(", "))
                    },
                })
            }
        }
    }
}
