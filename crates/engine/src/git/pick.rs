//! Cherry-pick e revert di un commit (`commit.cherry_pick`, `commit.revert`,
//! `cherry_pick.continue`/`abort`, `revert.continue`/`abort`, S31): tipi e controlli comuni ai
//! backend (`impl-docs/git/cherry-pick.md`).
//!
//! Il modello è il merge di S29 ([`super::merge`]): il comando risolve la rev, sceglie il
//! genitore (`mainline` per un commit di merge) e controlla lo stato ([`check_pick_state`]); il
//! backend salva il branch in un backup ed esegue
//! ([`GitBackend::pick`](super::GitBackend::pick)). **Un conflitto lascia l'operazione in
//! corso** (`CHERRY_PICK_HEAD`/`REVERT_HEAD`): si conclude con `*.continue` (un commit con
//! [`CommitRequest::concluding`](super::CommitRequest::concluding)) o si annulla con
//! `*.abort` ([`GitBackend::pick_abort`](super::GitBackend::pick_abort)), con un backup di
//! indice e worktree prima. Un risultato vuoto (commit già applicato) non lascia niente in
//! corso.

use super::{OperationState, check_no_operation, conflicted_paths};
use crate::EngineError;
use project_domain::{Commit, EntryKind, Oid, Operation, RepoPath, Status};
use serde::{Deserialize, Serialize};

/// Cherry-pick o revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickKind {
    /// Applica le modifiche del commit sopra HEAD (`git cherry-pick`), con il suo autore.
    CherryPick,
    /// Applica le modifiche inverse del commit (`git revert`).
    Revert,
}

impl PickKind {
    /// L'operazione in corso quando si ferma.
    pub fn operation(self) -> Operation {
        match self {
            PickKind::CherryPick => Operation::CherryPick,
            PickKind::Revert => Operation::Revert,
        }
    }

    /// Il tipo dell'operazione in corso, se è un cherry-pick o un revert.
    pub fn from_operation(op: Operation) -> Option<Self> {
        match op {
            Operation::CherryPick => Some(PickKind::CherryPick),
            Operation::Revert => Some(PickKind::Revert),
            _ => None,
        }
    }

    /// L'id del comando che avvia l'operazione.
    pub fn command(self) -> &'static str {
        match self {
            PickKind::CherryPick => "commit.cherry_pick",
            PickKind::Revert => "commit.revert",
        }
    }

    /// L'id del comando che conclude l'operazione in corso.
    pub fn continue_command(self) -> &'static str {
        match self {
            PickKind::CherryPick => "cherry_pick.continue",
            PickKind::Revert => "revert.continue",
        }
    }

    /// L'id del comando che annulla l'operazione in corso.
    pub fn abort_command(self) -> &'static str {
        match self {
            PickKind::CherryPick => "cherry_pick.abort",
            PickKind::Revert => "revert.abort",
        }
    }

    /// Il sottocomando di git, usato anche nei nomi dei backup (`cherry-pick-...`,
    /// `revert-abort-...`).
    pub fn git(self) -> &'static str {
        match self {
            PickKind::CherryPick => "cherry-pick",
            PickKind::Revert => "revert",
        }
    }

    /// Il file della cartella git con il commit in ingresso.
    pub fn head_file(self) -> &'static str {
        match self {
            PickKind::CherryPick => "CHERRY_PICK_HEAD",
            PickKind::Revert => "REVERT_HEAD",
        }
    }

    /// Il nome per le descrizioni (`Cherry-pick`, `Revert`).
    pub fn title(self) -> &'static str {
        match self {
            PickKind::CherryPick => "Cherry-pick",
            PickKind::Revert => "Revert",
        }
    }
}

/// Cosa chiede `commit.cherry_pick`/`commit.revert` al backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickRequest {
    pub kind: PickKind,
    /// Il branch di HEAD (nome breve): l'operazione non parte con HEAD staccata.
    pub branch: String,
    /// Il valore del branch letto prima: il backend rifiuta se è cambiato.
    pub expected: Oid,
    /// Il commit da applicare o annullare (la rev già risolta: a git arriva solo l'id).
    pub commit: Oid,
    /// Il genitore rispetto a cui si prendono le modifiche: quello scelto con `mainline` per un
    /// commit di merge, l'unico altrimenti, `None` per un commit radice.
    pub parent: Option<Oid>,
    /// `-m <n>` per un commit di merge (1 = primo genitore).
    pub mainline: Option<usize>,
    /// Il messaggio del commit, come lo scrive git ([`pick_message`]): il `CliBackend` lascia
    /// che lo scriva git (`--no-edit`), il `MemoryBackend` usa questo.
    pub message: String,
}

/// Il genitore scelto per `commit` con `mainline`: un commit di merge lo vuole (1 = primo
/// genitore), un commit con un genitore solo (o radice) no. Errori come
/// [`EngineError::InvalidParams`] di `id`.
pub fn pick_parent(
    id: &'static str,
    commit: &Commit,
    mainline: Option<usize>,
) -> Result<Option<Oid>, EngineError> {
    let invalid = |reason: String| EngineError::InvalidParams { id, reason };
    match (commit.parents.len(), mainline) {
        (0 | 1, None) => Ok(commit.parents.first().copied()),
        (0 | 1, Some(_)) => Err(invalid(format!(
            "`mainline` vale solo per un commit di merge, e {} non lo è",
            super::short(&commit.id)
        ))),
        (n, None) => Err(invalid(format!(
            "{} è un commit di merge con {n} genitori: indicare con `mainline` (da 1 a {n}) \
             il genitore rispetto a cui prendere le modifiche (di solito 1, il branch in cui \
             si è unito)",
            super::short(&commit.id)
        ))),
        (n, Some(m)) if m == 0 || m > n => Err(invalid(format!(
            "`mainline` deve essere fra 1 e {n} (i genitori di {}), non {m}",
            super::short(&commit.id)
        ))),
        (_, Some(m)) => Ok(Some(commit.parents[m - 1])),
    }
}

/// Il messaggio del commit come lo scrive git con `--no-edit` (senza `-x`): per il
/// cherry-pick quello del commit; per il revert `Revert "<oggetto>"` (o `Reapply "..."` per il
/// revert di un revert, come git dalla 2.43) e `This reverts commit <id>.` (con `, reversing
/// changes made to <genitore>.` per un commit di merge).
pub fn pick_message(kind: PickKind, commit: &Commit, parent: Option<&Oid>, merge: bool) -> String {
    match kind {
        PickKind::CherryPick => {
            if commit.body.is_empty() {
                commit.summary.clone()
            } else {
                format!("{}\n\n{}", commit.summary, commit.body)
            }
        }
        PickKind::Revert => {
            let title = match commit
                .summary
                .strip_prefix("Revert \"")
                .and_then(|s| s.strip_suffix('"'))
            {
                Some(inner) => format!("Reapply \"{inner}\""),
                None => format!("Revert \"{}\"", commit.summary),
            };
            match parent {
                Some(parent) if merge => format!(
                    "{title}\n\nThis reverts commit {}, reversing\nchanges made to {parent}.",
                    commit.id
                ),
                _ => format!("{title}\n\nThis reverts commit {}.", commit.id),
            }
        }
    }
}

/// I controlli prima di `commit.cherry_pick`/`commit.revert`, uguali per i backend, sullo
/// stato letto ora ([`EngineError::NotAllowed`], niente cambia):
///
/// - un'operazione in corso (anche un altro cherry-pick);
/// - file in conflitto;
/// - **qualunque** modifica ai file tracciati (in stage o no), come per un commit di merge: un
///   cherry-pick fermo in conflitto mescolerebbe le modifiche locali con le sue, e l'abort non
///   sa sempre ricostruirle. Mai autostash. I non tracciati vanno bene.
pub fn check_pick_state(status: &Status, kind: PickKind) -> Result<(), EngineError> {
    let what = match kind {
        PickKind::CherryPick => "inizierebbe un cherry-pick",
        PickKind::Revert => "inizierebbe un revert",
    };
    check_no_operation(kind.command(), what, status.operation)?;
    let unmerged = conflicted_paths(status);
    if !unmerged.is_empty() {
        return Err(EngineError::NotAllowed(format!(
            "ci sono file in conflitto ({}): risolverli prima di `{}`; niente modificato",
            quoted(&unmerged),
            kind.command()
        )));
    }
    let changed: Vec<RepoPath> = status
        .entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                EntryKind::Ordinary(_) | EntryKind::Renamed { .. } | EntryKind::Copied { .. }
            ) && (e.index.is_change() || e.worktree.is_change())
        })
        .map(|e| e.path.clone())
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    Err(EngineError::NotAllowed(format!(
        "ci sono modifiche locali ai file tracciati ({}): committarle o metterle da parte \
         (`stash.push`) e riprovare `{}` (niente autostash); niente modificato",
        quoted(&changed),
        kind.command()
    )))
}

/// Il controllo comune a `*.continue` e `*.abort`: deve essere in corso un'operazione `kind`
/// di un commit solo. Un sequencer con più commit avviato da terminale (`.git/sequencer`,
/// [`OperationState::sequencer`]) è rifiutato: `git commit` lo lascerebbe a metà e `git
/// cherry-pick --abort` riporterebbe indietro anche i commit già applicati della sequenza.
pub fn check_picking(
    command: &str,
    kind: PickKind,
    status: &Status,
    state: &OperationState,
) -> Result<(), EngineError> {
    super::merge::check_in_progress(command, kind.operation(), status)?;
    if state.sequencer {
        return Err(EngineError::NotAllowed(format!(
            "è in corso una sequenza di più commit avviata da terminale (`.git/sequencer`): \
             `{command}` la lascerebbe a metà o ne annullerebbe anche i commit già fatti, \
             niente modificato. Continuarla o annullarla da terminale (`git {0} --continue`, \
             `git {0} --skip` o `git {0} --abort`)",
            kind.git()
        )));
    }
    Ok(())
}

/// L'errore di un cherry-pick o revert che non cambierebbe niente (commit già applicato, o
/// già annullato), uguale nei due backend: [`EngineError::NotAllowed`].
pub fn pick_empty_error(kind: PickKind, commit: &Oid) -> EngineError {
    let what = match kind {
        PickKind::CherryPick => "le sue modifiche sono già nel branch",
        PickKind::Revert => "le sue modifiche non sono (più) nel branch",
    };
    EngineError::NotAllowed(format!(
        "`{}` di {} non cambierebbe niente ({what}): nessun commit creato, niente in corso, \
         niente modificato",
        kind.command(),
        super::short(commit)
    ))
}

fn quoted(paths: &[RepoPath]) -> String {
    paths
        .iter()
        .map(|p| format!("`{p}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::Signature;

    fn commit(parents: usize, summary: &str) -> Commit {
        let sig = Signature {
            name: "A".into(),
            email: "a@x".into(),
            time: 0,
            offset_min: 0,
        };
        Commit {
            id: Oid::from_bytes([9; Oid::LEN]),
            parents: (0..parents)
                .map(|i| Oid::from_bytes([i as u8 + 1; Oid::LEN]))
                .collect(),
            author: sig.clone(),
            committer: sig,
            summary: summary.into(),
            body: String::new(),
        }
    }

    #[test]
    fn mainline_rules() {
        let id = "commit.revert";
        assert!(pick_parent(id, &commit(0, "r"), None).unwrap().is_none());
        assert_eq!(
            pick_parent(id, &commit(1, "x"), None).unwrap(),
            Some(Oid::from_bytes([1; Oid::LEN]))
        );
        assert_eq!(
            pick_parent(id, &commit(2, "m"), Some(2)).unwrap(),
            Some(Oid::from_bytes([2; Oid::LEN]))
        );
        for (parents, mainline) in [(1, Some(1)), (2, None), (2, Some(0)), (2, Some(3))] {
            let err = pick_parent(id, &commit(parents, "c"), mainline).unwrap_err();
            assert!(
                matches!(err, EngineError::InvalidParams { .. }),
                "{parents} {mainline:?}: {err}"
            );
        }
    }

    #[test]
    fn messages_like_git() {
        let c = commit(1, "Aggiunge a");
        let id = c.id;
        assert_eq!(
            pick_message(PickKind::Revert, &c, c.parents.first(), false),
            format!("Revert \"Aggiunge a\"\n\nThis reverts commit {id}.")
        );
        let r = commit(1, "Revert \"Aggiunge a\"");
        assert!(
            pick_message(PickKind::Revert, &r, None, false).starts_with("Reapply \"Aggiunge a\"")
        );
        let m = commit(2, "Merge");
        let p = m.parents[0];
        assert_eq!(
            pick_message(PickKind::Revert, &m, Some(&p), true),
            format!(
                "Revert \"Merge\"\n\nThis reverts commit {id}, reversing\nchanges made to {p}."
            )
        );
        let mut c = commit(1, "Titolo");
        c.body = "Corpo".into();
        assert_eq!(
            pick_message(PickKind::CherryPick, &c, None, false),
            "Titolo\n\nCorpo"
        );
    }
}
