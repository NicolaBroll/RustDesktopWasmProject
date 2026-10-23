//! Diario delle operazioni per `edit.undo` e `edit.redo` (`impl-docs/ARCHITETTURA-git.md` §7).
//!
//! Ogni comando che sposta ref o HEAD (oggi `commit.create`, normale e `amend`, e i comandi
//! `branch.*`; poi merge, rebase, reset) lascia una [`JournalEntry`] con HEAD e i ref coinvolti prima e dopo.
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
//! - **`worktree.discard` entra nel diario** anche se non sposta ref: la voce ha il backup del
//!   worktree ([`JournalEntry::worktree`], il commit di `git stash create`) e `edit.undo` rimette
//!   i file com'erano ([`GitBackend::restore_worktree`]), solo se nel frattempo non sono
//!   cambiati; `edit.redo` li scarta di nuovo, solo se sono ancora come li ha lasciati l'undo.
//! - **Branch.** `branch.create` e `branch.delete` si annullano spostando il ref (eliminato o
//!   ricreato sul commit di prima, solo se nessuno l'ha toccato). `branch.checkout` e
//!   `branch.rename` invece si annullano con un altro checkout ([`GitBackend::checkout`]: mai
//!   forzato, rifiutato se sovrascriverebbe modifiche) e un'altra rinomina
//!   ([`GitBackend::rename_branch`]), che aggiornano anche worktree, reflog e configurazione.
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
    CommitOutcome, ConfigEntry, DiscardOutcome, EngineError, GitBackend, HeadState, RefMove,
    RefRestore, RestoreOutcome, WorktreeBackup, WorktreeRestore, branch_short, check_no_operation,
    discard_description,
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
/// "backup": string | null, "undo_backups": [string], "worktree": WorktreeBackup | null}`.
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
    /// Solo per `worktree.discard`: il backup del worktree (commit di `git stash create`) e i
    /// file scartati. Con questo campo undo e redo rimettono i file, non spostano ref.
    #[serde(default)]
    pub worktree: Option<WorktreeBackup>,
    /// Solo per `branch.delete`: la configurazione `branch.<nome>.*` tolta con il branch, che
    /// l'undo riscrive (se nel frattempo il repo non ne ha una nuova) e il redo toglie di nuovo.
    #[serde(default)]
    pub branch_config: Vec<ConfigEntry>,
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
            worktree: None,
            branch_config: Vec::new(),
        })
    }

    /// La voce di un `worktree.discard` riuscito, con HEAD com'era (non si sposta) e il backup.
    pub fn discard(outcome: &DiscardOutcome, head: HeadState) -> Self {
        Self {
            id: 0,
            command: "worktree.discard".to_owned(),
            description: discard_description(&outcome.files),
            head: HeadChange {
                before: head.clone(),
                after: head,
            },
            refs: Vec::new(),
            backup: Some(outcome.backup.clone()),
            undo_backups: Vec::new(),
            worktree: Some(WorktreeBackup {
                stash: outcome.stash,
                files: outcome.files.clone(),
                raw: outcome.raw,
            }),
            branch_config: Vec::new(),
        }
    }

    /// La voce di un `branch.create` riuscito: il branch `branch` (nome completo) creato su
    /// `target`, HEAD com'era. L'undo lo elimina (solo se vale ancora `target`), il redo lo
    /// ricrea (solo se non esiste).
    pub fn branch_create(branch: &str, target: Oid, head: HeadState) -> Self {
        Self::branch_entry(
            "branch.create",
            format!(
                "Crea branch `{}` su {}",
                branch_short(branch),
                short(&target)
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: None,
                after: Some(target),
            }],
            None,
        )
    }

    /// La voce di un `branch.delete` riuscito: il branch `branch` (nome completo) che valeva
    /// `target`, con il ref di backup. L'undo lo ricrea su `target` (solo se nel frattempo
    /// nessuno ha creato un branch con lo stesso nome), il redo lo elimina di nuovo (solo se
    /// vale ancora `target`).
    pub fn branch_delete(
        branch: &str,
        target: Oid,
        head: HeadState,
        backup: String,
        config: Vec<ConfigEntry>,
    ) -> Self {
        let mut entry = Self::branch_entry(
            "branch.delete",
            format!(
                "Elimina branch `{}` (era {})",
                branch_short(branch),
                short(&target)
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(target),
                after: None,
            }],
            Some(backup),
        );
        entry.branch_config = config;
        entry
    }

    /// La voce di un `branch.rename` riuscito: `from` diventato `to` (nomi completi) su
    /// `target`; HEAD prima e dopo (cambia se era il branch di HEAD). Undo e redo rinominano
    /// di nuovo ([`GitBackend::rename_branch`]), così reflog e configurazione seguono il branch.
    pub fn branch_rename(from: &str, to: &str, target: Oid, head: HeadChange) -> Self {
        Self::branch_entry(
            "branch.rename",
            format!(
                "Rinomina branch `{}` in `{}`",
                branch_short(from),
                branch_short(to)
            ),
            head,
            vec![
                RefChange {
                    name: from.to_owned(),
                    before: Some(target),
                    after: None,
                },
                RefChange {
                    name: to.to_owned(),
                    before: None,
                    after: Some(target),
                },
            ],
            None,
        )
    }

    /// La voce di un `branch.checkout` riuscito: HEAD da `before` a `after`, nessun ref
    /// spostato. Undo e redo rifanno un checkout sicuro ([`GitBackend::checkout`]), che non
    /// sovrascrive mai modifiche locali.
    pub fn checkout(before: HeadState, after: HeadState) -> Self {
        let name = |h: &HeadState| match h {
            HeadState::Branch(b) => format!("`{}`", branch_short(b)),
            HeadState::Detached(oid) => format!("{} (HEAD staccata)", short(oid)),
        };
        Self::branch_entry(
            "branch.checkout",
            format!("Checkout di {} (da {})", name(&after), name(&before)),
            HeadChange { before, after },
            Vec::new(),
            None,
        )
    }

    fn branch_entry(
        command: &str,
        description: String,
        head: HeadChange,
        refs: Vec<RefChange>,
        backup: Option<String>,
    ) -> Self {
        Self {
            id: 0,
            command: command.to_owned(),
            description,
            head,
            refs,
            backup,
            undo_backups: Vec::new(),
            worktree: None,
            branch_config: Vec::new(),
        }
    }

    /// La richiesta al backend per annullare (`redo == false`) o rifare uno scarto; `None` se
    /// la voce non è di `worktree.discard`.
    pub fn worktree_request(&self, redo: bool) -> Option<WorktreeRestore> {
        let backup = self.worktree.as_ref()?;
        Some(WorktreeRestore {
            command: if redo { "edit.redo" } else { "edit.undo" },
            redo,
            stash: backup.stash,
            files: backup.files.clone(),
            raw: backup.raw,
        })
    }

    /// Esegue undo o redo della voce sul backend: i file per uno scarto, i ref altrimenti.
    ///
    /// Per uno scarto HEAD deve essere quella registrata (`head.after` per l'undo,
    /// `head.before` per il redo): i file si confrontano con l'indice, ma l'operazione
    /// appartiene a quello stato del repo; con HEAD spostata (un checkout, un commit da
    /// terminale) [`EngineError::UndoConflict`] e niente cambia.
    fn apply(&self, repo: &mut dyn GitBackend, redo: bool) -> Result<RestoreOutcome, EngineError> {
        if let Some(request) = self.worktree_request(redo) {
            let expected = if redo {
                &self.head.before
            } else {
                &self.head.after
            };
            let now = HeadState::from_head(&repo.head()?);
            if now != *expected {
                return Err(EngineError::UndoConflict {
                    command: request.command,
                    refs: vec![format!("HEAD: doveva essere {expected}, ora è {now}")],
                });
            }
            return repo.restore_worktree(&request);
        }
        let request = if redo {
            self.redo_request()
        } else {
            self.undo_request()
        };
        match self.command.as_str() {
            "branch.checkout" => {
                Self::check_unchanged(repo, &request, "cambierebbe branch")?;
                repo.checkout(&request.head_target)
            }
            "branch.rename" => {
                Self::check_unchanged(repo, &request, "rinominerebbe un branch")?;
                // Da chi esiste ora (expected) a chi deve esistere dopo (target).
                let from = request.refs.iter().find(|r| r.target.is_none());
                let to = request.refs.iter().find(|r| r.expected.is_none());
                match (from, to) {
                    (
                        Some(RefMove {
                            name: from,
                            expected: Some(oid),
                            ..
                        }),
                        Some(RefMove { name: to, .. }),
                    ) => repo.rename_branch(branch_short(from), branch_short(to), oid),
                    _ => Err(EngineError::NotAllowed(format!(
                        "voce del diario non valida: `{}`, niente modificato",
                        self.description
                    ))),
                }
            }
            "branch.delete" => {
                let mut outcome = repo.restore_refs(&request)?;
                // Il ref è già ricreato / eliminato: la configurazione dà solo avvisi.
                if let Some(r) = self.refs.first() {
                    outcome.warnings.extend(repo.restore_branch_config(
                        branch_short(&r.name),
                        &self.branch_config,
                        redo,
                    ));
                }
                Ok(outcome)
            }
            _ => repo.restore_refs(&request),
        }
    }

    /// Per undo e redo di checkout e rinomina, che non passano da
    /// [`GitBackend::restore_refs`]: gli stessi controlli ([`check_no_operation`], HEAD e i ref
    /// come li ha lasciati l'operazione, altrimenti [`EngineError::UndoConflict`]) prima di
    /// chiamare il backend, che ricontrolla i ref subito prima di scrivere.
    fn check_unchanged(
        repo: &dyn GitBackend,
        request: &RefRestore,
        what: &str,
    ) -> Result<(), EngineError> {
        check_no_operation(request.command, what, repo.status()?.operation)?;
        let refs = repo.refs()?;
        let head = HeadState::from_head(&repo.head()?);
        let conflicts = request.conflicts(&head, |name| {
            Ok(refs.iter().find(|r| r.name == name).map(|r| r.target))
        })?;
        if conflicts.is_empty() {
            Ok(())
        } else {
            Err(EngineError::UndoConflict {
                command: request.command,
                refs: conflicts,
            })
        }
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
    /// ([`JournalEntry::undo_request`]) o, per uno scarto, di rimettere i file del backup
    /// ([`JournalEntry::worktree_request`]). Se riesce la voce passa fra quelle annullate, con i
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
        let outcome = entry.apply(repo, false)?;
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
        let outcome = entry.apply(repo, true)?;
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
            worktree: None,
            branch_config: Vec::new(),
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
                "undo_backups": [],
                "worktree": null,
                "branch_config": []
            })
        );
    }
}
