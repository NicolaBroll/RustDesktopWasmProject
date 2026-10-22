//! Tipi del client git: identificatori, commit, ref, HEAD, corsie del grafo.
//!
//! Solo dati e funzioni pure: i parser dell'output di git lavorano su `&[u8]`
//! (il log, [`parse_log`], lo stato, [`parse_status`], i ref, [`parse_refs`], i file cambiati
//! da un commit, [`parse_diff_tree`], e il diff, [`parse_diff`]); [`build_patch`] costruisce le
//! patch per lo stage di singole righe.

mod diff;
mod diff_tree;
mod graph;
mod log;
mod oid;
mod patch;
mod path;
mod quote;
mod refs;
mod status;
mod text;

#[cfg(test)]
mod diff_tests;
#[cfg(test)]
mod diff_tree_tests;
#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod log_tests;
#[cfg(test)]
mod patch_tests;
#[cfg(test)]
mod refs_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod tests;

pub use diff::{DIFF_ARGS, Diff, FileDiff, Hunk, Line, LineKind, NO_NEWLINE_MARKER, parse_diff};
pub use diff_tree::{ChangedFile, CommitDetails, DIFF_TREE_ARGS, parse_diff_tree};
pub use graph::{GraphLayout, GraphRow, GraphSegment, layout_graph, layout_graph_nodes};
pub use log::{LOG_ARGS, LOG_FIELDS, LOG_FORMAT, parse_log};
pub use oid::Oid;
pub use patch::{APPLY_ARGS, LineRef, PatchDirection, build_patch};
pub use path::RepoPath;
pub use refs::{REFS_ARGS, REFS_FIELDS, REFS_FORMAT, parse_refs};
pub use status::{STATUS_ARGS, parse_status};
pub use text::ByteText;

use serde::{Deserialize, Serialize};

/// Autore o committer di un commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    /// Nome come scritto nel commit (convertito con `from_utf8_lossy` se non UTF-8).
    pub name: String,
    pub email: String,
    /// Secondi dall'epoca Unix (UTC).
    pub time: i64,
    /// Fuso orario di chi ha firmato, in minuti rispetto a UTC (es. `120` per +0200).
    pub offset_min: i32,
}

/// Un commit letto dal log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub id: Oid,
    /// Genitori in ordine: vuoto per un commit radice, due o più per un merge.
    pub parents: Vec<Oid>,
    pub author: Signature,
    pub committer: Signature,
    /// Oggetto: il primo paragrafo del messaggio con le righe unite da uno spazio (`%s` di git).
    pub summary: String,
    /// Resto del messaggio dopo il primo paragrafo (`%b` di git), senza gli a capo finali;
    /// può essere vuoto.
    pub body: String,
}

impl Commit {
    /// Vero se il commit ha più di un genitore.
    pub fn is_merge(&self) -> bool {
        self.parents.len() > 1
    }
}

/// Tipo di un ref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefKind {
    /// `refs/heads/*`
    LocalBranch,
    /// `refs/remotes/*`
    RemoteBranch,
    /// `refs/tags/*`
    Tag,
    /// `refs/stash`
    Stash,
}

/// Branch remoto seguito da un branch locale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Upstream {
    /// Nome dell'upstream nella forma data da git: completo in [`RefInfo`]
    /// (es. `refs/remotes/origin/main`), breve in [`Status`] (es. `origin/main`, come in
    /// `# branch.upstream` di `git status`).
    pub name: String,
    /// Commit locali non presenti nell'upstream. `None` se git non lo sa (es. upstream sparito).
    pub ahead: Option<u32>,
    /// Commit dell'upstream non presenti in locale. `None` se git non lo sa.
    pub behind: Option<u32>,
}

/// Un ref (branch, tag, stash) e il commit a cui punta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefInfo {
    /// Nome completo, es. `refs/heads/main`.
    pub name: String,
    pub kind: RefKind,
    /// Il commit a cui punta il ref. Per un tag annotato è l'oggetto puntato dal tag (di solito
    /// un commit; un tag su un albero o un blob punta a quello), non il tag.
    pub target: Oid,
    /// Solo per i tag annotati: l'id dell'oggetto tag, cioè il valore del ref (serve, ad esempio,
    /// come valore atteso in `git update-ref -d <ref> <vecchio>`). `None` per i tag leggeri e gli
    /// altri ref.
    pub tag_object: Option<Oid>,
    /// Solo per i branch locali che ne hanno uno.
    pub upstream: Option<Upstream>,
}

impl RefInfo {
    /// Nome senza il prefisso del tipo (`refs/heads/`, `refs/remotes/`, `refs/tags/`), per la UI.
    /// Se il prefisso non c'è restituisce il nome completo.
    pub fn short_name(&self) -> &str {
        let prefix = match self.kind {
            RefKind::LocalBranch => "refs/heads/",
            RefKind::RemoteBranch => "refs/remotes/",
            RefKind::Tag => "refs/tags/",
            RefKind::Stash => "refs/",
        };
        self.name.strip_prefix(prefix).unwrap_or(&self.name)
    }
}

/// Dove punta HEAD.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Head {
    /// Su un branch. `name` è il nome breve (es. `main`, come in `git status --branch`);
    /// `target` è `None` se il branch non ha ancora commit (repo vuoto).
    Branch { name: String, target: Option<Oid> },
    /// HEAD staccata su un commit.
    Detached(Oid),
}

impl Head {
    /// Il commit di HEAD, se esiste.
    pub fn target(&self) -> Option<Oid> {
        match self {
            Head::Branch { target, .. } => *target,
            Head::Detached(oid) => Some(*oid),
        }
    }
}

/// Lo stato del repo: HEAD, upstream del branch corrente e file modificati.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub head: Head,
    /// Upstream del branch corrente, con il nome breve (es. `origin/main`). `None` se il branch
    /// non ne ha uno o HEAD è staccata. `ahead`/`behind` sono `None` se il ref dell'upstream
    /// non esiste più.
    pub upstream: Option<Upstream>,
    /// Una voce per file, nell'ordine di git. Vuoto se il worktree è pulito.
    pub entries: Vec<StatusEntry>,
    /// Operazione in corso. [`parse_status`] la lascia sempre a `None` (l'output di
    /// `git status --porcelain=v2` non la riporta): la riempie il backend guardando in `.git`
    /// (`MERGE_HEAD`, `rebase-merge/`, `CHERRY_PICK_HEAD`, ...).
    pub operation: Option<Operation>,
}

impl Status {
    /// Vero se non c'è niente da mettere in stage o da committare: nessuna voce tranne i file
    /// ignorati.
    pub fn is_clean(&self) -> bool {
        self.entries
            .iter()
            .all(|e| matches!(e.kind, EntryKind::Ignored))
    }

    /// Vero se almeno un file è in conflitto.
    pub fn has_conflicts(&self) -> bool {
        self.entries.iter().any(StatusEntry::is_conflict)
    }
}

/// Operazione in corso nel repo, da concludere o annullare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
    /// `git am` fermo a metà (`.git/rebase-apply/applying`). Aggiunta dopo le altre.
    Am,
}

/// Un file nello stato del repo.
///
/// `index` e `worktree` sono le lettere `X` e `Y` di git: la modifica in stage (HEAD → indice)
/// e quella non in stage (indice → worktree). Per i file non tracciati e ignorati valgono
/// entrambe [`Change::Untracked`] o [`Change::Ignored`], come `??` e `!!` di `git status -s`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusEntry {
    /// Percorso attuale (per una rinomina, quello nuovo).
    pub path: RepoPath,
    pub index: Change,
    pub worktree: Change,
    pub kind: EntryKind,
}

impl StatusEntry {
    /// Vero se il file ha modifiche in stage (esclusi i conflitti).
    pub fn is_staged(&self) -> bool {
        !self.is_conflict() && self.index.is_change()
    }

    /// Vero se il file ha modifiche nel worktree non ancora in stage (esclusi conflitti, file
    /// non tracciati e ignorati). Include i file aggiunti con `git add -N` (`.A`).
    pub fn is_unstaged(&self) -> bool {
        !self.is_conflict() && self.worktree.is_change()
    }

    /// Vero se il file è in conflitto (record `u`).
    pub fn is_conflict(&self) -> bool {
        matches!(self.kind, EntryKind::Unmerged(_))
    }

    /// Vero se il file non è tracciato.
    pub fn is_untracked(&self) -> bool {
        matches!(self.kind, EntryKind::Untracked)
    }

    /// Percorso di partenza di una rinomina o copia.
    pub fn original_path(&self) -> Option<&RepoPath> {
        match &self.kind {
            EntryKind::Renamed { original, .. } | EntryKind::Copied { original, .. } => {
                Some(original)
            }
            _ => None,
        }
    }

    /// Stato del sottomodulo, se il percorso è un sottomodulo.
    pub fn submodule(&self) -> Option<SubmoduleStatus> {
        match &self.kind {
            EntryKind::Ordinary(info)
            | EntryKind::Renamed { info, .. }
            | EntryKind::Copied { info, .. } => info.submodule,
            EntryKind::Unmerged(info) => info.submodule,
            EntryKind::Untracked | EntryKind::Ignored => None,
        }
    }
}

/// Una lettera `X` o `Y` dello stato di git.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Change {
    /// `.`
    Unmodified,
    /// `M`
    Modified,
    /// `T`: cambiato il tipo (es. da file a link simbolico).
    TypeChanged,
    /// `A`
    Added,
    /// `D`
    Deleted,
    /// `R`
    Renamed,
    /// `C`
    Copied,
    /// `U`: in conflitto.
    Unmerged,
    /// `?`
    Untracked,
    /// `!`
    Ignored,
}

impl Change {
    /// La lettera usata da git.
    pub fn letter(self) -> char {
        match self {
            Change::Unmodified => '.',
            Change::Modified => 'M',
            Change::TypeChanged => 'T',
            Change::Added => 'A',
            Change::Deleted => 'D',
            Change::Renamed => 'R',
            Change::Copied => 'C',
            Change::Unmerged => 'U',
            Change::Untracked => '?',
            Change::Ignored => '!',
        }
    }

    /// Vero per una modifica vera e propria: né `.`, né `U`, né `?`, né `!`.
    pub fn is_change(self) -> bool {
        !matches!(
            self,
            Change::Unmodified | Change::Unmerged | Change::Untracked | Change::Ignored
        )
    }
}

/// Il tipo di voce, con i dati propri di ciascun record di `git status --porcelain=v2`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    /// Record `1`: file tracciato modificato.
    Ordinary(TrackedInfo),
    /// Record `2` con `R`: rinominato da `original`; `score` è la somiglianza (0–100).
    Renamed {
        info: TrackedInfo,
        original: RepoPath,
        score: u8,
    },
    /// Record `2` con `C`: copiato da `original` (solo con `status.renames=copies`).
    Copied {
        info: TrackedInfo,
        original: RepoPath,
        score: u8,
    },
    /// Record `u`: in conflitto.
    Unmerged(UnmergedInfo),
    /// Record `?`.
    Untracked,
    /// Record `!` (solo con `--ignored`).
    Ignored,
}

/// Modi e id di un file tracciato. I modi sono numeri ottali (es. `0o100644`, `0o120000` per un
/// link, `0o160000` per un sottomodulo, `0` se il file non c'è); gli id sono zero se l'oggetto
/// non esiste (es. `oid_head` di un file aggiunto).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackedInfo {
    /// `Some` se il percorso è un sottomodulo.
    pub submodule: Option<SubmoduleStatus>,
    pub mode_head: u32,
    pub mode_index: u32,
    pub mode_worktree: u32,
    pub oid_head: Oid,
    pub oid_index: Oid,
}

/// Dati di un file in conflitto: gli stadi 1 (base), 2 (nostro) e 3 (loro) dell'indice.
/// Uno stadio assente ha modo `0` e id zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnmergedInfo {
    pub conflict: Conflict,
    pub submodule: Option<SubmoduleStatus>,
    pub mode_base: u32,
    pub mode_ours: u32,
    pub mode_theirs: u32,
    pub mode_worktree: u32,
    pub oid_base: Oid,
    pub oid_ours: Oid,
    pub oid_theirs: Oid,
}

/// Tipo di conflitto, dal codice XY di un record `u`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Conflict {
    /// `DD`
    BothDeleted,
    /// `AU`
    AddedByUs,
    /// `UD`
    DeletedByThem,
    /// `UA`
    AddedByThem,
    /// `DU`
    DeletedByUs,
    /// `AA`
    BothAdded,
    /// `UU`
    BothModified,
}

/// Stato di un sottomodulo (campo `S<c><m><u>`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SubmoduleStatus {
    /// `C`: il commit del sottomodulo è cambiato.
    pub commit_changed: bool,
    /// `M`: il sottomodulo ha modifiche ai file tracciati.
    pub modified: bool,
    /// `U`: il sottomodulo ha file non tracciati.
    pub untracked: bool,
}
