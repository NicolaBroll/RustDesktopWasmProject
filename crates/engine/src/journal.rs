//! Diario delle operazioni per `edit.undo` e `edit.redo` (`impl-docs/ARCHITETTURA-git.md` §7).
//!
//! Ogni comando che sposta ref (oggi `commit.create`, normale e `amend`; poi branch, checkout,
//! merge, rebase, reset) lascia una [`JournalEntry`] con HEAD e i ref coinvolti prima e dopo.
//! `edit.undo` riporta i ref ai valori "prima" e `edit.redo` ai valori "dopo", **solo** se
//! adesso valgono ancora quello che l'operazione (o l'undo) ha lasciato: il backend lo
//! controlla in modo atomico ([`GitBackend::restore_refs`]) e altrimenti dà
//! [`EngineError::UndoConflict`] senza toccare niente; la voce resta nel diario.
//!
//! Scelte:
//!
//! - **In memoria, per repo aperto.** Il diario vive nella [`Session`](crate::Session) e si
//!   azzera quando si apre un repo (anche lo stesso); non si salva su disco. Le CLI eseguono
//!   una sessione per invocazione: più `--cmd` della stessa invocazione condividono il
//!   diario, invocazioni diverse no. Persisterlo (es. in `.git/project/journal`) vorrebbe dire
//!   gestire più istanze dell'app sullo stesso repo e voci scritte da versioni diverse; non
//!   serve per non perdere dati: i commit annullati restano in ref di backup e ogni
//!   spostamento è nel reflog di git (`project: undo ...`).
//! - **Stage e unstage non entrano nel diario**: non spostano ref. Il loro recupero è il backup
//!   dell'indice (§7). Nemmeno una nuova voce del genere svuota il redo.
//! - **Undo di un commit = `git reset --soft`.** Si sposta solo il ref: indice e worktree non si
//!   toccano, quindi le modifiche del commit annullato ricompaiono in stage rispetto al nuovo
//!   HEAD (e `edit.redo` rifà lo stesso commit, che esiste ancora). Il primo commit di un
//!   branch si annulla cancellando il branch: HEAD resta sul branch, che torna senza commit.
//! - Un commit che dopo l'undo non sarebbe più raggiungibile da nessun ref si salva prima in
//!   `refs/project/backup/undo-...` (§7); se il backup non riesce l'undo non si fa.
//! - Il diario tiene al massimo [`Journal::MAX_ENTRIES`] voci: le più vecchie escono.

use crate::git::short;
use crate::{
    CommitOutcome, EngineError, GitBackend, HeadState, RefMove, RefRestore, RestoreOutcome,
};
use project_domain::Oid;
use serde::{Deserialize, Serialize};

/// Un ref spostato da un'operazione: nome completo, valore prima e dopo (`None` = il ref non
/// esisteva / è stato cancellato).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefChange {
    pub name: String,
    pub before: Option<Oid>,
    pub after: Option<Oid>,
}

/// HEAD prima e dopo un'operazione.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadChange {
    pub before: HeadState,
    pub after: HeadState,
}

/// Una voce del diario: un comando che ha spostato ref.
///
/// JSON: `{"id": n, "command": "commit.create", "description": "Commit a0744b4 «...»",
/// "head": {"before": HeadState, "after": HeadState}, "refs": [{"name", "before", "after"}],
/// "backup": string | null, "undo_backups": [string]}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Progressivo nella sessione (dal primo repo aperto), per riconoscere la voce.
    pub id: u64,
    /// L'id del comando che l'ha prodotta (es. `commit.create`).
    pub command: String,
    /// Descrizione per le persone, es. `Commit a0744b4 «Aggiorna README»`.
    pub description: String,
    pub head: HeadChange,
    /// I ref spostati (per un commit su un branch, il branch; con HEAD staccata nessuno).
    pub refs: Vec<RefChange>,
    /// Il backup salvato dall'operazione stessa (es. il commit riscritto da `amend`).
    pub backup: Option<String>,
    /// I ref di backup creati da undo e redo di questa voce per i commit che altrimenti non
    /// sarebbero più raggiungibili.
    #[serde(default)]
    pub undo_backups: Vec<String>,
}

impl JournalEntry {
    /// La voce di un `commit.create` riuscito; `None` se non ha spostato niente (un `amend` che
    /// ha prodotto lo stesso commit) o se il risultato non è coerente (HEAD staccata senza
    /// commit prima).
    pub fn commit(outcome: &CommitOutcome, amend: bool) -> Option<Self> {
        let new = outcome.commit.id;
        if outcome.previous == Some(new) {
            return None;
        }
        let (head, refs) = match &outcome.branch {
            Some(branch) => (
                HeadChange {
                    before: HeadState::Branch(branch.clone()),
                    after: HeadState::Branch(branch.clone()),
                },
                vec![RefChange {
                    name: branch.clone(),
                    before: outcome.previous,
                    after: Some(new),
                }],
            ),
            None => (
                HeadChange {
                    before: HeadState::Detached(outcome.previous?),
                    after: HeadState::Detached(new),
                },
                Vec::new(),
            ),
        };
        let summary = &outcome.commit.summary;
        let description = match (amend, outcome.previous) {
            (true, Some(old)) => format!("Amend {} «{summary}» (era {})", short(&new), short(&old)),
            _ => format!("Commit {} «{summary}»", short(&new)),
        };
        Some(Self {
            id: 0,
            command: "commit.create".to_owned(),
            description,
            head,
            refs,
            backup: outcome.backup.clone(),
            undo_backups: Vec::new(),
        })
    }

    /// La richiesta al backend per annullare la voce: da "dopo" a "prima".
    pub fn undo_request(&self) -> RefRestore {
        RefRestore {
            command: "edit.undo",
            head_expected: self.head.after.clone(),
            head_target: self.head.before.clone(),
            refs: self
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: r.after,
                    target: r.before,
                })
                .collect(),
            reflog: format!("project: undo {}", self.description),
        }
    }

    /// La richiesta al backend per rifare la voce annullata: da "prima" a "dopo".
    pub fn redo_request(&self) -> RefRestore {
        RefRestore {
            command: "edit.redo",
            head_expected: self.head.before.clone(),
            head_target: self.head.after.clone(),
            refs: self
                .refs
                .iter()
                .map(|r| RefMove {
                    name: r.name.clone(),
                    expected: r.before,
                    target: r.after,
                })
                .collect(),
            reflog: format!("project: redo {}", self.description),
        }
    }
}

/// Il diario di un repo aperto: le voci fatte (l'ultima è quella che `edit.undo` annulla) e
/// quelle annullate (l'ultima è quella che `edit.redo` rifà).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Journal {
    done: Vec<JournalEntry>,
    undone: Vec<JournalEntry>,
    next_id: u64,
}

impl Journal {
    /// Voci tenute al massimo (fatte e annullate insieme); le più vecchie escono.
    pub const MAX_ENTRIES: usize = 100;

    /// Diario vuoto.
    pub fn new() -> Self {
        Self::default()
    }

    /// Diario vuoto che numera le voci da `next_id` (la sessione continua la numerazione
    /// quando apre un altro repo, così un id non indica mai due voci diverse).
    pub(crate) fn starting_at(next_id: u64) -> Self {
        Self {
            next_id,
            ..Self::default()
        }
    }

    pub(crate) fn next_id(&self) -> u64 {
        self.next_id
    }

    /// Registra una nuova operazione e svuota le voci annullate (non si possono più rifare:
    /// partivano da uno stato che la nuova operazione ha cambiato). Restituisce l'id dato alla
    /// voce.
    pub fn record(&mut self, mut entry: JournalEntry) -> u64 {
        self.next_id += 1;
        entry.id = self.next_id;
        self.done.push(entry);
        self.undone.clear();
        if self.done.len() > Self::MAX_ENTRIES {
            let extra = self.done.len() - Self::MAX_ENTRIES;
            self.done.drain(..extra);
        }
        self.next_id
    }

    /// Le voci fatte, dalla più vecchia; l'ultima è quella che `edit.undo` annulla.
    pub fn done(&self) -> &[JournalEntry] {
        &self.done
    }

    /// Le voci annullate; l'ultima è quella che `edit.redo` rifà.
    pub fn undone(&self) -> &[JournalEntry] {
        &self.undone
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    /// Annulla l'ultima voce fatta: chiede al backend di riportare i ref ai valori "prima"
    /// ([`JournalEntry::undo_request`]). Se riesce la voce passa fra quelle annullate, con i
    /// ref di backup eventualmente creati in `undo_backups`; se no (anche per
    /// [`EngineError::UndoConflict`]) resta dov'è e niente è cambiato. Senza voci,
    /// [`EngineError::NotAllowed`]. Restituisce la voce e l'esito (backup creati ora, avvisi).
    pub fn undo(
        &mut self,
        repo: &mut dyn GitBackend,
    ) -> Result<(JournalEntry, RestoreOutcome), EngineError> {
        let entry = self.done.last().ok_or_else(|| {
            EngineError::NotAllowed(
                "niente da annullare: nessuna operazione sui ref nel diario di questa sessione"
                    .to_owned(),
            )
        })?;
        let outcome = repo.restore_refs(&entry.undo_request())?;
        let mut entry = self.done.pop().expect("c'è almeno una voce");
        entry.undo_backups.extend(outcome.backups.iter().cloned());
        self.undone.push(entry.clone());
        Ok((entry, outcome))
    }

    /// Rifà l'ultima voce annullata, come [`Journal::undo`] al contrario
    /// ([`JournalEntry::redo_request`]): i ref devono valere ancora "prima".
    pub fn redo(
        &mut self,
        repo: &mut dyn GitBackend,
    ) -> Result<(JournalEntry, RestoreOutcome), EngineError> {
        let entry = self.undone.last().ok_or_else(|| {
            EngineError::NotAllowed("niente da rifare: nessuna operazione annullata".to_owned())
        })?;
        let outcome = repo.restore_refs(&entry.redo_request())?;
        let mut entry = self.undone.pop().expect("c'è almeno una voce");
        entry.undo_backups.extend(outcome.backups.iter().cloned());
        self.done.push(entry.clone());
        Ok((entry, outcome))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommitRequest, MemoryBackend};

    fn entry(n: u64) -> JournalEntry {
        JournalEntry {
            id: 0,
            command: "commit.create".to_owned(),
            description: format!("voce {n}"),
            head: HeadChange {
                before: HeadState::Branch("refs/heads/main".to_owned()),
                after: HeadState::Branch("refs/heads/main".to_owned()),
            },
            refs: Vec::new(),
            backup: None,
            undo_backups: Vec::new(),
        }
    }

    #[test]
    fn record_numbers_entries_clears_redo_and_keeps_the_newest() {
        let mut j = Journal::new();
        assert!(!j.can_undo() && !j.can_redo());
        assert_eq!(j.record(entry(1)), 1);
        assert_eq!(j.record(entry(2)), 2);
        j.undone.push(entry(9));
        assert!(j.can_redo());
        j.record(entry(3));
        assert!(!j.can_redo(), "una nuova operazione svuota il redo");
        for n in 0..Journal::MAX_ENTRIES as u64 {
            j.record(entry(10 + n));
        }
        assert_eq!(j.done().len(), Journal::MAX_ENTRIES);
        assert_eq!(j.done()[0].description, "voce 10");
        assert_eq!(Journal::starting_at(7).record(entry(1)), 8);
    }

    #[test]
    fn entry_of_a_commit() {
        let mut repo = MemoryBackend::sample();
        let out = repo.commit(&CommitRequest::new("Aggiorna")).unwrap();
        let e = JournalEntry::commit(&out, false).unwrap();
        assert_eq!(e.command, "commit.create");
        assert_eq!(
            e.description,
            format!("Commit {} «Aggiorna»", short(&out.commit.id))
        );
        assert_eq!(
            e.refs,
            [RefChange {
                name: "refs/heads/main".to_owned(),
                before: out.previous,
                after: Some(out.commit.id),
            }]
        );
        let undo = e.undo_request();
        assert_eq!(undo.refs[0].expected, Some(out.commit.id));
        assert_eq!(undo.refs[0].target, out.previous);
        assert!(undo.reflog.starts_with("project: undo Commit "));
        let redo = e.redo_request();
        assert_eq!(redo.refs[0].expected, out.previous);
        assert_eq!(redo.command, "edit.redo");

        // `amend`: descrizione con il commit di prima, e il backup.
        let amend = repo
            .commit(&CommitRequest::new("Aggiorna meglio").amend())
            .unwrap();
        let e = JournalEntry::commit(&amend, true).unwrap();
        assert!(e.description.starts_with("Amend "), "{}", e.description);
        assert!(e.description.contains(&short(&out.commit.id)));
        assert_eq!(e.backup, amend.backup);

        // Un `amend` che non cambia niente non va nel diario.
        let mut same = amend.clone();
        same.previous = Some(same.commit.id);
        assert_eq!(JournalEntry::commit(&same, true), None);

        // HEAD staccata: si sposta HEAD, nessun ref.
        let mut detached = out.clone();
        detached.branch = None;
        let e = JournalEntry::commit(&detached, false).unwrap();
        assert!(e.refs.is_empty());
        assert_eq!(e.head.before, HeadState::Detached(out.previous.unwrap()));
        assert_eq!(e.head.after, HeadState::Detached(out.commit.id));
    }

    #[test]
    fn json_shape() {
        let e = entry(1);
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "id": 0,
                "command": "commit.create",
                "description": "voce 1",
                "head": {
                    "before": { "branch": "refs/heads/main" },
                    "after": { "branch": "refs/heads/main" }
                },
                "refs": [],
                "backup": null,
                "undo_backups": []
            })
        );
    }
}
