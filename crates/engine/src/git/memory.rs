//! Repository finto in memoria: per i test dei comandi e per la versione web.

mod branch;
mod commit;
mod conflict;
mod discard;
mod merge;
mod remote;
mod remote_admin;
mod restore;
mod stash;
mod sync;
mod write;

pub use discard::{StashedFile, WorktreeStash};
use merge::MemoryMerge;
pub use remote::FetchFailure;
use remote::MemoryRemote;
use stash::MemoryStash;

use super::line_diff;
use super::{
    BranchDeleted, CommitOutcome, CommitRequest, ConfigEntry, ConflictResolution, ConflictResolved,
    ConflictVersions, DiffSpec, DiscardOutcome, DiscardedFile, FetchOutcome, FetchRequest,
    GitBackend, HeadState, LineSelection, LogQuery, MergeAborted, MergeOutcome, MergeRequest,
    OperationState, PullOutcome, PullRequest, PullSettings, PushOutcome, PushRequest, RefRestore,
    RemoteSnapshot, RepoOpener, RestoreOutcome, StagedVersion, StashApplied, StashApply, StashDrop,
    StashDropped, StashEntry, StashPush, UpstreamConfig, WorktreeRestore,
};
use crate::{EngineError, JobContext};
use project_domain::{
    Change, ChangedFile, Commit, CommitDetails, Diff, EntryKind, FileDiff, Head, Oid, Operation,
    RefInfo, RefKind, RepoPath, Status, StatusEntry, Upstream,
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
    /// File cambiati da ogni commit rispetto al primo genitore; un commit assente non cambia
    /// niente.
    changes: BTreeMap<Oid, Vec<ChangedFile>>,
    /// Nome completo → destinazione; l'ordine della mappa è quello di `git for-each-ref`.
    refs: BTreeMap<String, RefEntry>,
    /// Branch locale → branch remoto, entrambi nomi completi.
    upstreams: BTreeMap<String, String>,
    head: HeadRef,
    entries: Vec<StatusEntry>,
    operation: Option<Operation>,
    /// Contenuto degli oggetti blob, per i diff.
    blobs: BTreeMap<Oid, Vec<u8>>,
    /// Contenuto dei file nel worktree, per `diff.worktree`.
    worktree: BTreeMap<RepoPath, Vec<u8>>,
    /// Backup dell'indice salvati da stage e unstage: nome del ref e versioni salvate.
    backups: Vec<(String, Vec<StagedVersion>)>,
    /// Ref di backup dei commit (riscritti da `amend`, abbandonati da undo/redo): nome e commit.
    commit_backups: Vec<(String, Oid)>,
    /// Backup del worktree salvati da `worktree.discard`.
    worktree_backups: Vec<WorktreeStash>,
    /// Nome ed email di chi committa (`user.name`/`user.email`); `None` = non configurata.
    identity: Option<(String, String)>,
    /// Remoti configurati, per nome.
    remotes: BTreeMap<String, MemoryRemote>,
    /// La configurazione di pull (`pull.rebase`, `pull.ff`), uguale per tutti i branch.
    pull_settings: PullSettings,
    /// Se c'è, un hook `pre-push` che rifiuta ogni push con questo messaggio.
    pre_push_failure: Option<String>,
    /// Altre chiavi della configurazione ([`GitBackend::config_values`]).
    config: BTreeMap<String, Vec<String>>,
    /// I "server" dei remoti tolti, per URL: un remoto aggiunto con lo stesso URL li ritrova.
    detached_remotes: BTreeMap<String, MemoryRemote>,
    /// Voci `remote.<nome>.*` "fuori dal file del repo" ([`MemoryBackend::set_remote_elsewhere`]).
    remote_elsewhere: BTreeMap<String, Vec<String>>,
    /// Remoti che `remotes()` elenca anche dopo la rimozione
    /// ([`MemoryBackend::keep_remote_listed`]).
    listed_anyway: BTreeSet<String>,
    /// Quante `restore_refs` riescono ancora prima di fallire
    /// ([`MemoryBackend::fail_restore_refs_after`]); `None`: sempre.
    restore_budget: Option<usize>,
    /// Gli stash, dal più recente (`stash@{0}`); `refs/stash` punta al primo.
    stashes: Vec<MemoryStash>,
    /// Gli stash eliminati, per id: `stash_store` (l'undo di un drop) li ritrova interi.
    dropped_stashes: BTreeMap<Oid, MemoryStash>,
    /// Se c'è, ogni `stash_drop` fallisce con questo messaggio
    /// ([`MemoryBackend::fail_stash_drop`]).
    stash_drop_failure: Option<String>,
    /// Il merge in corso (`MERGE_HEAD`, `MERGE_MSG`), con `operation` = `Merge`.
    merge_state: Option<MemoryMerge>,
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
            changes: BTreeMap::new(),
            refs: BTreeMap::new(),
            upstreams: BTreeMap::new(),
            head: HeadRef::Branch("main".to_owned()),
            entries: Vec::new(),
            operation: None,
            blobs: BTreeMap::new(),
            worktree: BTreeMap::new(),
            backups: Vec::new(),
            commit_backups: Vec::new(),
            worktree_backups: Vec::new(),
            identity: None,
            remotes: BTreeMap::new(),
            pull_settings: PullSettings::default(),
            pre_push_failure: None,
            config: BTreeMap::new(),
            detached_remotes: BTreeMap::new(),
            remote_elsewhere: BTreeMap::new(),
            listed_anyway: BTreeSet::new(),
            restore_budget: None,
            stashes: Vec::new(),
            dropped_stashes: BTreeMap::new(),
            stash_drop_failure: None,
            merge_state: None,
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

    /// File cambiati dal commit `id` rispetto al primo genitore (o all'albero vuoto per un
    /// commit radice), nell'ordine in cui li darebbe git. Li restituisce
    /// [`GitBackend::show_commit`]; senza questa chiamata il commit non cambia nessun file.
    pub fn set_changes(&mut self, id: Oid, files: Vec<ChangedFile>) {
        self.changes.insert(id, files);
    }

    /// Contenuto del blob `id`, usato dai diff dei file che lo hanno come id. Un file il cui
    /// contenuto non è noto compare nei diff come binario (senza hunk).
    pub fn set_blob(&mut self, id: Oid, content: impl Into<Vec<u8>>) {
        self.blobs.insert(id, content.into());
    }

    /// Contenuto del file `path` nel worktree, per [`DiffSpec::Worktree`]: serve per le voci
    /// dello stato modificate fuori stage (per i file eliminati no).
    pub fn set_worktree_file(&mut self, path: impl Into<RepoPath>, content: impl Into<Vec<u8>>) {
        self.worktree.insert(path.into(), content.into());
    }

    /// I backup dell'indice salvati da stage e unstage, in ordine: nome del ref e versioni in
    /// stage salvate (i contenuti sono nei blob).
    pub fn backups(&self) -> &[(String, Vec<StagedVersion>)] {
        &self.backups
    }

    /// I ref di backup dei commit, in ordine: nome del ref e commit. Li creano `amend`
    /// (`refs/project/backup/amend-<n>`, il commit riscritto) e `edit.undo`/`edit.redo`
    /// (`refs/project/backup/undo-<n>`/`redo-<n>`, i commit che non sarebbero più
    /// raggiungibili).
    pub fn commit_backups(&self) -> &[(String, Oid)] {
        &self.commit_backups
    }

    /// Identità di chi committa, come `user.name` e `user.email`. Senza (il default di
    /// [`MemoryBackend::new`]) un commit dà errore, come git senza identità.
    pub fn set_identity(&mut self, name: &str, email: &str) {
        self.identity = Some((name.to_owned(), email.to_owned()));
    }

    /// Il contenuto del blob `id`, se noto.
    pub fn blob(&self, id: Oid) -> Option<&[u8]> {
        self.blobs.get(&id).map(Vec::as_slice)
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

    /// Operazione in corso (merge, rebase, ...), `None` se nessuna. Un merge impostato così
    /// non ha `MERGE_HEAD` (il commit lo rifiuta); un merge vero lo lascia `merge.start` fermo
    /// in conflitto.
    pub fn set_operation(&mut self, operation: Option<Operation>) {
        self.operation = operation;
        self.merge_state = None;
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
        // I backup dei commit sono ref anche loro (`refs/project/backup/...`).
        if let Some((_, id)) = self.commit_backups.iter().find(|(name, _)| name == rev) {
            return Ok(*id);
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
        self.reachable_from([start])
    }

    fn reachable_from(&self, starts: impl IntoIterator<Item = Oid>) -> BTreeSet<Oid> {
        let mut seen = BTreeSet::new();
        let mut stack: Vec<Oid> = starts.into_iter().collect();
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

/// Un lato di un file in un diff: modo, id e contenuto (se noto).
#[derive(Clone, Copy)]
struct Side<'a> {
    mode: u32,
    oid: Oid,
    content: Option<&'a [u8]>,
}

/// Id finto del contenuto di un file del worktree (git lo calcola con `hash-object`).
fn content_oid(content: &[u8]) -> Oid {
    // FNV-1a: deterministico e diverso per contenuti diversi (in pratica).
    let hash = content.iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    MemoryBackend::fake_oid(hash)
}

/// Il [`FileDiff`] di un file da `old` (assente per un file aggiunto) a `new` (assente per un
/// file eliminato), con i campi come li legge `parse_diff` dall'output di git.
fn file_diff<'a>(
    change: Change,
    old_path: &RepoPath,
    new_path: &RepoPath,
    similarity: Option<u8>,
    old: Option<Side<'a>>,
    new: Option<Side<'a>>,
) -> FileDiff {
    let zero = Oid::from_bytes([0; Oid::LEN]);
    let mut file = FileDiff {
        old_path: old_path.clone(),
        new_path: new_path.clone(),
        change,
        similarity,
        old_mode: old.map(|s| s.mode),
        new_mode: new.map(|s| s.mode),
        old_oid: Some(old.map_or(zero, |s| s.oid)),
        new_oid: Some(new.map_or(zero, |s| s.oid)),
        binary: false,
        hunks: Vec::new(),
    };
    if let (Some(o), Some(n)) = (old, new)
        && o.oid == n.oid
    {
        // Contenuto invariato (rinomina esatta, cambio di solo modo): git non scrive la riga
        // `index`, e i modi solo se cambiano.
        file.old_oid = None;
        file.new_oid = None;
        if o.mode == n.mode {
            file.old_mode = None;
            file.new_mode = None;
        }
        return file;
    }
    let content = |side: Option<Side<'a>>| -> Option<&'a [u8]> {
        match side {
            None => Some(&[][..]),
            Some(s) if s.oid == zero => Some(&[][..]),
            Some(s) => s.content,
        }
    };
    match (content(old), content(new)) {
        // Come git: binario se c'è un NUL nei primi 8000 byte.
        (Some(o), Some(n)) if !is_binary(o) && !is_binary(n) => {
            file.hunks = line_diff::hunks(o, n);
        }
        _ => file.binary = true,
    }
    file
}

fn is_binary(content: &[u8]) -> bool {
    content.iter().take(8000).any(|&b| b == 0)
}

fn unmerged(path: &RepoPath) -> FileDiff {
    FileDiff {
        old_path: path.clone(),
        new_path: path.clone(),
        change: Change::Unmerged,
        similarity: None,
        old_mode: None,
        new_mode: None,
        old_oid: None,
        new_oid: None,
        binary: false,
        hunks: Vec::new(),
    }
}

/// Il tipo di modifica di un file nel diff: un cambio di tipo è una modifica (git lo scrive
/// come due sezioni, ma qui basta una).
fn diff_change(change: Change) -> Change {
    match change {
        Change::TypeChanged => Change::Modified,
        other => other,
    }
}

impl MemoryBackend {
    fn side(&self, mode: u32, oid: Oid) -> Option<Side<'_>> {
        (mode != 0).then(|| Side {
            mode,
            oid,
            content: self.blobs.get(&oid).map(Vec::as_slice),
        })
    }

    /// I file del commit `rev` rispetto al primo genitore, dai file di `show_commit`.
    fn commit_diff(&self, rev: &str) -> Result<Vec<FileDiff>, EngineError> {
        let details = self.show_commit(rev)?;
        Ok(details
            .files
            .iter()
            .map(|f| {
                file_diff(
                    diff_change(f.change),
                    f.old_path(),
                    &f.path,
                    f.score,
                    self.side(f.old_mode, f.old_oid),
                    self.side(f.new_mode, f.new_oid),
                )
            })
            .collect())
    }

    /// Indice rispetto a HEAD, dalle voci dello stato.
    fn index_diff(&self) -> Vec<FileDiff> {
        let mut files = Vec::new();
        for entry in &self.entries {
            let (info, original, score) = match &entry.kind {
                EntryKind::Ordinary(info) => (info, None, None),
                EntryKind::Renamed {
                    info,
                    original,
                    score,
                }
                | EntryKind::Copied {
                    info,
                    original,
                    score,
                } => (info, Some(original), Some(*score)),
                EntryKind::Unmerged(_) => {
                    files.push(unmerged(&entry.path));
                    continue;
                }
                EntryKind::Untracked | EntryKind::Ignored => continue,
            };
            if !entry.index.is_change() {
                continue;
            }
            let old = self.side(info.mode_head, info.oid_head);
            let new = match entry.index {
                Change::Deleted => None,
                _ => self.side(info.mode_index, info.oid_index),
            };
            files.push(file_diff(
                diff_change(entry.index),
                original.unwrap_or(&entry.path),
                &entry.path,
                score,
                old,
                new,
            ));
        }
        files
    }

    /// Worktree rispetto all'indice, dalle voci dello stato e dai file del worktree.
    fn worktree_diff(&self) -> Vec<FileDiff> {
        let mut files = Vec::new();
        for entry in &self.entries {
            let info = match &entry.kind {
                EntryKind::Ordinary(info)
                | EntryKind::Renamed { info, .. }
                | EntryKind::Copied { info, .. } => info,
                EntryKind::Unmerged(_) => {
                    files.push(unmerged(&entry.path));
                    continue;
                }
                EntryKind::Untracked | EntryKind::Ignored => continue,
            };
            if !entry.worktree.is_change() {
                continue;
            }
            // Un file aggiunto fuori stage (`git add -N`) non ha contenuto nell'indice.
            let old = match entry.worktree {
                Change::Added => None,
                _ => self.side(info.mode_index, info.oid_index),
            };
            let new = match entry.worktree {
                Change::Deleted => None,
                _ => {
                    let content = self.worktree.get(&entry.path).map(Vec::as_slice);
                    Some(Side {
                        mode: info.mode_worktree,
                        oid: content.map_or_else(|| MemoryBackend::fake_oid(u64::MAX), content_oid),
                        content,
                    })
                }
            };
            files.push(file_diff(
                diff_change(entry.worktree),
                &entry.path,
                &entry.path,
                None,
                old,
                new,
            ));
        }
        files
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
        query.check()?;
        let starts: Vec<Oid> = if query.all {
            // Come `--branches --remotes --tags HEAD`: niente stash né backup (con `stashes` le
            // basi degli stash, non gli stash).
            self.refs
                .iter()
                .filter(|(name, _)| [HEADS, REMOTES, TAGS].iter().any(|p| name.starts_with(p)))
                .map(|(_, entry)| entry.target)
                .chain(self.head_target())
                .chain(
                    self.stashes
                        .iter()
                        .filter(|_| query.stashes)
                        .filter_map(|s| s.parents.first().copied()),
                )
                .collect()
        } else {
            match &query.rev {
                Some(rev) => vec![self.resolve(rev)?],
                None => self.head_target().into_iter().collect(),
            }
        };
        let wanted = query.skip.saturating_add(query.limit.unwrap_or(usize::MAX));
        let reachable = self.reachable_from(starts);

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

    fn show_commit(&self, rev: &str) -> Result<CommitDetails, EngineError> {
        let id = self.resolve(rev)?;
        // Un ref può puntare a un commit che il repo non ha (come in un clone superficiale).
        let commit = self
            .commits
            .get(&id)
            .ok_or_else(|| EngineError::UnknownRev(rev.to_owned()))?;
        Ok(CommitDetails {
            commit: commit.clone(),
            compared_to: commit.parents.first().copied(),
            files: self.changes.get(&id).cloned().unwrap_or_default(),
        })
    }

    fn diff(&self, spec: &DiffSpec) -> Result<Diff, EngineError> {
        let mut files = match spec {
            DiffSpec::Worktree { .. } => self.worktree_diff(),
            DiffSpec::Index { .. } => self.index_diff(),
            DiffSpec::Commit { rev, .. } => self.commit_diff(rev)?,
        };
        if let Some(path) = spec.path() {
            files.retain(|f| f.new_path == *path || f.old_path == *path);
        }
        Ok(Diff { files })
    }

    fn stage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        self.stage_paths(paths)
    }

    fn unstage(&mut self, paths: &[RepoPath]) -> Result<Option<String>, EngineError> {
        self.unstage_paths(paths)
    }

    fn apply_lines(&mut self, selection: &LineSelection) -> Result<Option<String>, EngineError> {
        self.apply_line_patch(selection)
    }

    fn commit(&mut self, request: &CommitRequest) -> Result<CommitOutcome, EngineError> {
        self.create_commit(request)
    }

    fn conflict_versions(&self, path: &RepoPath) -> Result<ConflictVersions, EngineError> {
        self.conflict_files(path)
    }

    fn resolve_conflict(
        &mut self,
        request: &ConflictResolution,
    ) -> Result<ConflictResolved, EngineError> {
        self.resolve_file(request)
    }

    fn restore_refs(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        if let Some(budget) = self.restore_budget.as_mut() {
            if *budget == 0 {
                return Err(EngineError::Git {
                    command: "update-ref".to_owned(),
                    stderr: "fallimento simulato (fail_restore_refs_after)".to_owned(),
                });
            }
            *budget -= 1;
        }
        Ok(RestoreOutcome {
            backups: self.restore(request)?,
            warnings: Vec::new(),
        })
    }

    fn discard(
        &mut self,
        paths: &[RepoPath],
        expected: &[DiscardedFile],
    ) -> Result<DiscardOutcome, EngineError> {
        self.discard_paths(paths, expected)
    }

    fn restore_worktree(
        &mut self,
        request: &WorktreeRestore,
    ) -> Result<RestoreOutcome, EngineError> {
        self.restore_discarded(request)?;
        Ok(RestoreOutcome::default())
    }
    fn unmerged_count(&self, tip: &Oid, into: &[Oid]) -> Result<usize, EngineError> {
        Ok(self.unmerged(tip, into))
    }

    fn remotes(&self) -> Result<Vec<String>, EngineError> {
        let mut names = self.remote_names();
        names.extend(self.listed_anyway.iter().cloned());
        names.sort();
        names.dedup();
        Ok(names)
    }

    fn fetch(
        &mut self,
        request: &FetchRequest,
        job: &JobContext,
    ) -> Result<FetchOutcome, EngineError> {
        self.fetch_remote(request, job)
    }

    fn create_branch(&mut self, name: &str, target: &Oid) -> Result<(), EngineError> {
        self.create_local_branch(name, target)
    }

    fn upstream(&self, branch: &str) -> Result<Option<UpstreamConfig>, EngineError> {
        Ok(self.upstream_config(branch))
    }

    fn pull_settings(&self, _branch: &str) -> Result<PullSettings, EngineError> {
        Ok(self.pull_settings.clone())
    }

    fn config_values(&self, key: &str) -> Result<Vec<String>, EngineError> {
        Ok(self.config_of(key))
    }

    fn missing_commits(
        &self,
        tip: &Oid,
        exclude: &[Oid],
        limit: usize,
    ) -> Result<Vec<Commit>, EngineError> {
        self.commits_missing(tip, exclude, limit)
    }

    fn pull(&mut self, request: &PullRequest) -> Result<PullOutcome, EngineError> {
        self.pull_upstream(request)
    }

    fn reset_branch(&mut self, request: &RefRestore) -> Result<RestoreOutcome, EngineError> {
        self.reset_keep(request)
    }

    fn push(
        &mut self,
        request: &PushRequest,
        job: &JobContext,
    ) -> Result<PushOutcome, EngineError> {
        self.push_branch(request, job)
    }

    fn checkout(&mut self, target: &HeadState) -> Result<RestoreOutcome, EngineError> {
        self.checkout_head(target)
    }

    fn delete_branch(&mut self, name: &str, expected: &Oid) -> Result<BranchDeleted, EngineError> {
        self.delete_local_branch(name, expected)
    }

    fn rename_branch(
        &mut self,
        from: &str,
        to: &str,
        expected: &Oid,
    ) -> Result<RestoreOutcome, EngineError> {
        self.rename_local_branch(from, to, expected)
    }

    fn restore_branch_config(
        &mut self,
        name: &str,
        entries: &[ConfigEntry],
        remove: bool,
    ) -> Vec<String> {
        self.set_branch_config(name, entries, remove)
    }

    fn remote_snapshot(&self, name: &str) -> Result<RemoteSnapshot, EngineError> {
        Ok(self.snapshot_of(name))
    }

    fn create_remote(&mut self, name: &str, url: &str) -> Result<(), EngineError> {
        self.create_memory_remote(name, url)
    }

    fn remove_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
        self.drop_remote_config(snapshot)
    }

    fn write_remote_config(&mut self, snapshot: &RemoteSnapshot) -> Vec<String> {
        self.put_remote_config(snapshot)
    }

    fn remote_config_elsewhere(&self, name: &str) -> Result<Vec<String>, EngineError> {
        Ok(self.remote_elsewhere.get(name).cloned().unwrap_or_default())
    }

    fn stashes(&self) -> Result<Vec<StashEntry>, EngineError> {
        Ok(self.stash_list())
    }

    fn stash_push(&mut self, request: &StashPush) -> Result<StashEntry, EngineError> {
        self.push_stash(request)
    }

    fn stash_apply(&mut self, request: &StashApply) -> Result<StashApplied, EngineError> {
        self.apply_stash(request)
    }

    fn stash_drop(&mut self, request: &StashDrop) -> Result<StashDropped, EngineError> {
        self.drop_stash(request)
    }

    fn stash_store(&mut self, id: &Oid, message: &str) -> Result<(), EngineError> {
        self.store_stash(id, message)
    }

    fn merge(&mut self, request: &MergeRequest) -> Result<MergeOutcome, EngineError> {
        self.merge_commit(request)
    }

    fn merge_abort(&mut self) -> Result<MergeAborted, EngineError> {
        self.abort_merge()
    }

    fn operation_state(&self) -> Result<OperationState, EngineError> {
        Ok(self.operation_info())
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
