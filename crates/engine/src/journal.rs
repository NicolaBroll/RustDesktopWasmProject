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
//! - **`remote.pull` entra nel diario** quando sposta il branch ([`JournalEntry::pull`]): per il
//!   branch attivo undo e redo passano da [`GitBackend::reset_branch`] (come `git reset --keep`:
//!   aggiornano indice e worktree, rifiutano se un file da ripristinare ha modifiche locali), per
//!   un altro branch spostano solo il ref. Il fetch e il push no.
//! - **`merge.start` e `merge.continue` entrano nel diario** quando il merge si conclude
//!   ([`JournalEntry::merge`]): undo e redo come il pull del branch attivo. Un merge fermo in
//!   conflitto no: il branch non si è mosso, e finché il merge è in corso undo e redo sono
//!   rifiutati; `merge.abort` nemmeno (il recupero è il suo backup di indice e worktree).
//! - **`remote.add` e `remote.remove` entrano nel diario** ([`JournalEntry::remote_add`],
//!   [`JournalEntry::remote_remove`]): la voce tiene il remoto ([`RemoteSnapshot`]:
//!   configurazione, branch remoti, upstream che lo usano); undo e redo lo tolgono o lo
//!   rimettono, solo se nel frattempo non è cambiato o non è stato ricreato.
//! - **`stash.push` e `stash.drop` entrano nel diario** ([`JournalEntry::stash_push`],
//!   [`JournalEntry::stash_drop`], con lo stash in [`JournalEntry::stash`]): l'undo di un push è
//!   il `pop` di quello stash (stessa HEAD, stash ancora nell'elenco), il redo un altro push con
//!   gli stessi parametri; l'undo di un drop rimette lo stash in cima all'elenco
//!   (`git stash store`), il redo lo elimina di nuovo. `stash.apply` e `stash.pop` no: cambiano
//!   solo indice e worktree, e lo stash applicato resta nell'elenco o in un backup (vedi
//!   [`StashRecord`]).
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

/// I comandi le cui voci spostano il branch di HEAD con indice e worktree
/// ([`JournalEntry::moves_worktree`]).
const WORKTREE_COMMANDS: &[&str] = &[
    "remote.pull",
    "merge.start",
    "merge.continue",
    "commit.cherry_pick",
    "commit.revert",
    "cherry_pick.continue",
    "revert.continue",
];
use crate::{
    CommitOutcome, ConfigEntry, DiscardOutcome, EngineError, GitBackend, HeadState, LogQuery,
    RefMove, RefRestore, RemoteSnapshot, RestoreOutcome, StashDrop, StashEntry, StashPush,
    StashRecord, WorktreeBackup, WorktreeRestore, branch_short, check_no_operation,
    discard_description, find_stash, pop_stash, remove_remote, restore_remote,
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
    /// Solo per `remote.add` e `remote.remove`: il remoto com'era dopo l'aggiunta / prima
    /// della rimozione (configurazione, branch remoti, upstream che lo usano). Undo e redo lo
    /// tolgono o lo rimettono ([`remove_remote`], [`restore_remote`]), solo se nel frattempo non
    /// è cambiato / non è stato ricreato. Nel JSON gli URL sono mascherati, quindi non si
    /// rilegge dal JSON (`skip_deserializing`). Dopo un undo che ha saltato degli upstream
    /// (reimpostati altrove) la voce tiene solo quello che è stato rimesso.
    #[serde(default, skip_deserializing)]
    pub remote: Option<RemoteSnapshot>,
    /// Solo per `stash.push` e `stash.drop`: lo stash (id, messaggio, base e, per il push, i
    /// parametri). Undo e redo lo rimettono o lo tolgono dall'elenco ([`StashRecord`]); dopo il
    /// redo di un push la voce ha l'id dello stash nuovo.
    #[serde(default)]
    pub stash: Option<StashRecord>,
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
            remote: None,
            stash: None,
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
            remote: None,
            stash: None,
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

    /// La voce di un `remote.pull` riuscito che ha spostato il branch `branch` (nome completo)
    /// da `before` ad `after`; `head` è HEAD (uguale prima e dopo: il branch stesso se era il
    /// branch attivo), `strategy` `fast_forward`/`merge`/`rebase`, `upstream` il branch remoto
    /// (nome breve), `backup` il ref del branch prima del pull. Undo e redo del branch attivo
    /// passano da [`GitBackend::reset_branch`] (aggiornano anche indice e worktree, come
    /// `git reset --keep`); di un altro branch da [`GitBackend::restore_refs`].
    pub fn pull(
        branch: &str,
        before: Oid,
        after: Oid,
        head: HeadState,
        strategy: &str,
        upstream: &str,
        backup: String,
    ) -> Self {
        let how = match strategy {
            "fast_forward" => "fast-forward".to_owned(),
            "rebase" => format!("rebase su {upstream}"),
            _ => format!("merge di {upstream}"),
        };
        Self::branch_entry(
            "remote.pull",
            format!(
                "Pull di `{}` ({how}: da {} a {})",
                branch_short(branch),
                short(&before),
                short(&after)
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(before),
                after: Some(after),
            }],
            Some(backup),
        )
    }

    /// La voce di un merge concluso (`merge.start` riuscito, o `merge.continue`, `command`):
    /// il branch di HEAD `branch` (nome completo) da `before` ad `after` (il commit di merge o
    /// quello del fast-forward), HEAD uguale prima e dopo; `strategy` `fast_forward`/`merge`,
    /// `source` il nome di quello che si è unito (`feature`, `origin/x`, un id), `backup` il ref
    /// del branch prima del merge (se salvato). Undo e redo come per un pull del branch attivo:
    /// [`GitBackend::reset_branch`] (come `git reset --keep`).
    #[allow(clippy::too_many_arguments)]
    pub fn merge(
        command: &str,
        branch: &str,
        before: Oid,
        after: Oid,
        head: HeadState,
        strategy: &str,
        source: &str,
        backup: Option<String>,
    ) -> Self {
        let how = match strategy {
            "fast_forward" => "fast-forward",
            _ => "commit di merge",
        };
        Self::branch_entry(
            command,
            format!(
                "Merge di {source} in `{}` ({how}: da {} a {})",
                branch_short(branch),
                short(&before),
                short(&after)
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(before),
                after: Some(after),
            }],
            backup,
        )
    }

    /// La voce di un cherry-pick o revert concluso (`commit.cherry_pick`, `commit.revert`,
    /// `cherry_pick.continue`, `revert.continue`): il branch `branch` (nome completo) da
    /// `before` ad `after` (un commit nuovo sopra `before`), HEAD uguale prima e dopo; `source`
    /// il commit applicato o annullato, `backup` il ref del branch prima (se salvato). Undo e
    /// redo come il merge: [`GitBackend::reset_branch`] (come `git reset --keep`).
    #[allow(clippy::too_many_arguments)]
    pub fn pick(
        command: &str,
        kind: crate::PickKind,
        branch: &str,
        before: Oid,
        after: Oid,
        head: HeadState,
        source: &str,
        backup: Option<String>,
    ) -> Self {
        Self::branch_entry(
            command,
            format!(
                "{} di {source} su `{}` (da {} a {})",
                kind.title(),
                branch_short(branch),
                short(&before),
                short(&after)
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            vec![RefChange {
                name: branch.to_owned(),
                before: Some(before),
                after: Some(after),
            }],
            backup,
        )
    }

    /// Vero se la voce sposta il branch di HEAD con indice e worktree (un pull del branch
    /// attivo, un merge, un cherry-pick o revert): undo e redo usano
    /// [`GitBackend::reset_branch`].
    fn moves_worktree(&self) -> bool {
        WORKTREE_COMMANDS.contains(&self.command.as_str())
            && matches!(&self.head.after, HeadState::Branch(b) if self.refs.len() == 1 && self.refs[0].name == *b)
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
            remote: None,
            stash: None,
        }
    }

    /// La voce di un `remote.add` riuscito: `snapshot` è il remoto subito dopo (con i branch
    /// remoti del fetch, se fatto), `head` HEAD (non cambia). L'undo toglie configurazione e
    /// branch remoti solo se sono ancora esattamente così (e nessun branch ha preso il remoto
    /// come upstream); il redo li rimette solo se il remoto non c'è.
    pub fn remote_add(snapshot: RemoteSnapshot, head: HeadState, fetched: bool) -> Self {
        let description = if fetched {
            format!(
                "Aggiungi remoto `{}` (con fetch: {} branch remoti)",
                snapshot.name,
                snapshot.refs.len()
            )
        } else {
            format!("Aggiungi remoto `{}`", snapshot.name)
        };
        Self::remote_entry("remote.add", description, head, snapshot, false)
    }

    /// La voce di un `remote.remove` riuscito: `snapshot` è il remoto com'era prima, `backup`
    /// il primo ref di backup dei commit che restavano solo nei branch remoti (gli altri in
    /// `undo_backups`). L'undo lo rimette (branch remoti, configurazione, upstream) solo se
    /// nessuno ha ricreato un remoto con quel nome; il redo lo toglie di nuovo solo se è ancora
    /// com'era.
    pub fn remote_remove(snapshot: RemoteSnapshot, head: HeadState, backups: &[String]) -> Self {
        let upstreams = snapshot.upstream_branches().len();
        let mut description = format!(
            "Rimuovi remoto `{}` ({} branch remoti",
            snapshot.name,
            snapshot.refs.len()
        );
        if upstreams > 0 {
            description.push_str(&format!(", upstream di {upstreams} branch"));
        }
        description.push(')');
        let mut entry = Self::remote_entry("remote.remove", description, head, snapshot, true);
        entry.backup = backups.first().cloned();
        entry.undo_backups = backups.iter().skip(1).cloned().collect();
        entry
    }

    fn remote_entry(
        command: &str,
        description: String,
        head: HeadState,
        snapshot: RemoteSnapshot,
        removed: bool,
    ) -> Self {
        let refs = snapshot
            .refs
            .iter()
            .map(|r| RefChange {
                name: r.name.clone(),
                before: removed.then_some(r.target),
                after: (!removed).then_some(r.target),
            })
            .collect();
        let mut entry = Self::branch_entry(
            command,
            description,
            HeadChange {
                before: head.clone(),
                after: head,
            },
            refs,
            None,
        );
        entry.remote = Some(snapshot);
        entry
    }

    /// La voce di un `stash.push` riuscito: lo stash creato (`stash`), HEAD (non cambia) e i
    /// parametri, per il redo. Undo = `pop` dello stash con l'indice, redo = un altro push.
    pub fn stash_push(stash: &StashEntry, head: HeadState, request: StashPush) -> Self {
        let mut entry = Self::branch_entry(
            "stash.push",
            format!(
                "Metti da parte le modifiche (stash {} «{}»)",
                short(&stash.id),
                stash.message
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            Vec::new(),
            None,
        );
        entry.stash = Some(StashRecord {
            id: stash.id,
            message: stash.message.clone(),
            base: stash.base,
            push: Some(request),
        });
        entry
    }

    /// La voce di uno `stash.drop` riuscito: lo stash eliminato, con il ref di backup che ne
    /// tiene il commit. Undo = di nuovo nell'elenco, in cima (`git stash store`); redo = drop.
    pub fn stash_drop(stash: &StashEntry, head: HeadState, backup: Option<String>) -> Self {
        let mut entry = Self::branch_entry(
            "stash.drop",
            format!(
                "Elimina {} ({} «{}»)",
                stash.name,
                short(&stash.id),
                stash.message
            ),
            HeadChange {
                before: head.clone(),
                after: head,
            },
            Vec::new(),
            backup,
        );
        entry.stash = Some(StashRecord {
            id: stash.id,
            message: stash.message.clone(),
            base: stash.base,
            push: None,
        });
        entry
    }

    /// Undo e redo di `stash.push` e `stash.drop` (vedi [`StashRecord`]). Prima i controlli,
    /// che senza scrivere niente danno [`EngineError::UndoConflict`] se lo stash o HEAD non
    /// sono più come li ha lasciati l'operazione (o l'undo).
    fn apply_stash(
        &mut self,
        repo: &mut dyn GitBackend,
        redo: bool,
    ) -> Result<RestoreOutcome, EngineError> {
        let command = if redo { "edit.redo" } else { "edit.undo" };
        let Some(record) = self.stash.clone() else {
            return Err(EngineError::NotAllowed(format!(
                "voce del diario non valida: `{}`, niente modificato",
                self.description
            )));
        };
        let conflict = |what: String| EngineError::UndoConflict {
            command,
            refs: vec![what],
        };
        let present = find_stash(&repo.stashes()?, &record.id).is_some();
        let short_id = short(&record.id);
        if self.command == "stash.drop" {
            if redo {
                if !present {
                    return Err(conflict(format!(
                        "lo stash {short_id} non è più nell'elenco"
                    )));
                }
                // Il backup del drop di solito c'è già; se non tiene più lo stash (tolto o
                // spostato a mano), uno nuovo prima di eliminarlo.
                let kept = self.backup.as_deref().is_some_and(|b| {
                    repo.log(&LogQuery::page(0, 1).from_rev(b))
                        .ok()
                        .and_then(|c| c.first().map(|c| c.id))
                        == Some(record.id)
                });
                let dropped = repo.stash_drop(&StashDrop {
                    command,
                    id: record.id,
                    backup: (!kept).then_some("redo"),
                })?;
                return Ok(RestoreOutcome {
                    backups: dropped.backup.into_iter().collect(),
                    warnings: dropped.warnings,
                });
            }
            if present {
                return Err(conflict(format!("lo stash {short_id} è già nell'elenco")));
            }
            repo.stash_store(&record.id, &record.message)?;
            return Ok(RestoreOutcome::default());
        }

        // `stash.push`: la HEAD della voce, sul commit su cui lo stash è stato creato (così
        // il `pop` dell'undo non può andare in conflitto e il redo mette da parte rispetto
        // allo stesso commit).
        check_no_operation(command, "cambierebbe gli stash", repo.status()?.operation)?;
        let head = repo.head()?;
        let state = HeadState::from_head(&head);
        let expected = if redo {
            &self.head.before
        } else {
            &self.head.after
        };
        if state != *expected || head.target() != Some(record.base) {
            return Err(conflict(format!(
                "HEAD: doveva essere {expected} sul commit {}, ora è {state}{}",
                short(&record.base),
                head.target()
                    .map(|t| format!(" sul commit {}", short(&t)))
                    .unwrap_or_default()
            )));
        }
        if redo {
            let push = record.push.clone().unwrap_or_default();
            let stash = repo.stash_push(&push)?;
            self.stash = Some(StashRecord {
                id: stash.id,
                message: stash.message,
                ..record
            });
            return Ok(RestoreOutcome::default());
        }
        if !present {
            return Err(conflict(format!(
                "lo stash {short_id} non è più nell'elenco"
            )));
        }
        let popped = pop_stash(repo, command, record.id, true, Some("undo")).map_err(|e| match e {
            // L'undo usa sempre `restore_index`: non può "riprovare senza".
            EngineError::NotAllowed(m) if m.starts_with(crate::INDEX_NOT_RESTORED) => {
                EngineError::NotAllowed(format!(
                    "l'undo non può rimettere lo stash {short_id} con la divisione fra in stage e \
                     no: l'indice attuale non è più quello da cui è partito (succede con un push \
                     con `paths` quando anche altri file erano in stage: git mette nello stash \
                     tutto l'indice). Niente modificato, la voce resta nel diario; per riavere le \
                     modifiche, `stash.pop` senza `restore_index` (tutto torna fuori stage)"
                ))
            }
            other => other,
        })?;
        let applied = popped.applied;
        if let Some(e) = popped.drop_error {
            return Err(EngineError::Git {
                command: "stash".to_owned(),
                stderr: format!(
                    "l'undo ha applicato lo stash {short_id} (le modifiche sono di nuovo nei \
                     file) ma non è riuscito a toglierlo dall'elenco ({e}): non ripetere l'undo; \
                     eliminare lo stash con `stash.drop`. La voce resta nel diario"
                ),
            });
        }
        let Some(dropped) = popped.dropped else {
            return Err(EngineError::Git {
                command: "stash".to_owned(),
                stderr: format!(
                    "l'undo ha applicato lo stash {short_id} ma restano file in conflitto ({}): \
                     lo stash resta nell'elenco; risolvere i conflitti con `index.stage`",
                    applied.conflicts.join(", ")
                ),
            });
        };
        let mut warnings = applied.warnings;
        warnings.extend(dropped.warnings);
        Ok(RestoreOutcome {
            backups: dropped.backup.into_iter().collect(),
            warnings,
        })
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
    fn apply(
        &mut self,
        repo: &mut dyn GitBackend,
        redo: bool,
    ) -> Result<RestoreOutcome, EngineError> {
        if let Some(request) = self.worktree_request(redo) {
            // Come per i ref: mai a metà di un merge, rebase... (riscriverebbe file del merge).
            check_no_operation(
                request.command,
                "riscriverebbe file",
                repo.status()?.operation,
            )?;
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
        if self.stash.is_some() {
            return self.apply_stash(repo, redo);
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
            "remote.add" | "remote.remove" => {
                let Some(snapshot) = self.remote.clone() else {
                    return Err(EngineError::NotAllowed(format!(
                        "voce del diario non valida: `{}`, niente modificato",
                        self.description
                    )));
                };
                let dependents = self.command == "remote.remove";
                // Undo di un'aggiunta e redo di una rimozione tolgono il remoto.
                if redo == dependents {
                    remove_remote(repo, &snapshot, request.command, dependents, request.reflog)
                } else {
                    let (outcome, written) = restore_remote(
                        repo,
                        &snapshot,
                        request.command,
                        dependents,
                        request.reflog,
                    )?;
                    // Undo di una rimozione con upstream saltati: il redo deve confrontare (e
                    // togliere) solo quello che è stato davvero rimesso.
                    if dependents {
                        self.remote = Some(written);
                    }
                    Ok(outcome)
                }
            }
            _ if self.moves_worktree() => {
                Self::check_unchanged(repo, &request, "sposterebbe il branch")?;
                repo.reset_branch(&request)
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
        let entry = self.done.last_mut().ok_or_else(|| {
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
        let entry = self.undone.last_mut().ok_or_else(|| {
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
            remote: None,
            stash: None,
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
                "branch_config": [],
                "remote": null,
                "stash": null
            })
        );
    }
}
