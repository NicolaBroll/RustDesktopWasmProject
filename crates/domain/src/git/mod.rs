//! Tipi del client git: identificatori, commit, ref, HEAD, corsie del grafo.
//!
//! Solo dati: i parser dell'output di git (log, ref, stato) arriveranno accanto a questi tipi
//! e lavoreranno su `&[u8]`.

mod graph;
mod oid;

#[cfg(test)]
mod graph_tests;
#[cfg(test)]
mod tests;

pub use graph::{GraphLayout, GraphRow, GraphSegment, layout_graph, layout_graph_nodes};
pub use oid::Oid;

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
    /// Prima riga del messaggio.
    pub summary: String,
    /// Resto del messaggio dopo la riga vuota; può essere vuoto.
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
    /// Nome completo del ref remoto, es. `refs/remotes/origin/main`.
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
    pub target: Oid,
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
