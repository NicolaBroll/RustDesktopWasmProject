//! Messaggio di commit nell'area di stage: la bozza (oggetto, corpo, amend), quando il
//! pulsante "Commit" è attivo e perché no, i parametri di `commit.create`, la risposta e gli
//! errori. Solo dati, senza egui: si prova con test unitari.
//!
//! La bozza è stato della UI, non persistente: resta finché il repo è aperto, anche se si
//! seleziona un commit e poi si torna alla riga delle modifiche, e non si perde se il commit
//! fallisce. Si svuota solo dopo un commit riuscito.

use crate::executor::ErrorKind;
use crate::{Action, App, Done, Request};
use project_domain::{Commit, Oid, Operation, Status};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// Oltre questa lunghezza l'oggetto si segnala con un avviso tenue (convenzione di git: le
/// righe del log restano leggibili).
pub const SUBJECT_LIMIT: usize = 72;

/// Il messaggio in scrittura.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitDraft {
    /// Oggetto, su una riga.
    pub subject: String,
    /// Corpo, anche su più righe; può essere vuoto.
    pub body: String,
    /// Riscrive l'ultimo commit invece di crearne uno nuovo.
    amend: bool,
    /// Il messaggio dell'ultimo commit proposto attivando amend con la bozza vuota: se si
    /// disattiva amend senza averlo cambiato, la bozza torna vuota.
    proposed: Option<(String, String)>,
}

impl CommitDraft {
    pub fn amend(&self) -> bool {
        self.amend
    }

    /// Il messaggio da mandare: l'oggetto, poi una riga vuota e il corpo se c'è. Gli spazi in
    /// fondo e le righe vuote in più li toglie git (`commit.cleanup`).
    pub fn message(&self) -> String {
        let subject = self.subject.trim_end();
        let body = self.body.trim_end();
        if body.trim().is_empty() {
            subject.to_owned()
        } else {
            format!("{subject}\n\n{body}")
        }
    }

    /// Vero se il messaggio è vuoto o di soli spazi.
    pub fn is_blank(&self) -> bool {
        self.subject.trim().is_empty() && self.body.trim().is_empty()
    }

    /// Caratteri dell'oggetto (la sua prima riga, senza gli spazi in fondo).
    pub fn subject_len(&self) -> usize {
        self.subject
            .lines()
            .next()
            .unwrap_or("")
            .trim_end()
            .chars()
            .count()
    }

    pub fn subject_too_long(&self) -> bool {
        self.subject_len() > SUBJECT_LIMIT
    }

    /// Attiva o disattiva amend. Attivandolo con la bozza vuota propone il messaggio di
    /// `last` (l'ultimo commit, se è noto); disattivandolo toglie il messaggio proposto se non
    /// è stato cambiato. Restituisce `true` se serve il messaggio dell'ultimo commit e `last`
    /// non c'era: va chiesto e passato poi a [`CommitDraft::propose`].
    pub fn set_amend(&mut self, on: bool, last: Option<&Commit>) -> bool {
        self.amend = on;
        if !on {
            if let Some((subject, body)) = self.proposed.take()
                && subject == self.subject
                && body == self.body
            {
                self.subject.clear();
                self.body.clear();
            }
            return false;
        }
        match last {
            Some(commit) => {
                self.propose(commit);
                false
            }
            None => self.is_blank(),
        }
    }

    /// Propone il messaggio di `commit` (l'ultimo) se amend è attivo e la bozza è vuota.
    pub fn propose(&mut self, commit: &Commit) {
        if self.amend && self.is_blank() {
            self.subject = commit.summary.clone();
            self.body = commit.body.clone();
            self.proposed = Some((self.subject.clone(), self.body.clone()));
        }
    }

    /// Vero se la bozza è il messaggio proposto, non ancora cambiato.
    pub fn is_proposed(&self) -> bool {
        self.proposed
            .as_ref()
            .is_some_and(|(s, b)| *s == self.subject && *b == self.body)
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

/// Perché il pulsante "Commit" è disabilitato; il testo spiega cosa fare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Una scrittura (anche un commit) è in corso, o una conferma è aperta.
    Busy,
    /// File in conflitto.
    Conflicts,
    /// Merge, rebase, cherry-pick o revert in corso.
    Operation(Operation),
    /// Amend senza un commit da riscrivere (branch senza commit).
    NoCommitToAmend,
    /// Niente in stage, senza amend.
    NothingStaged,
    /// Messaggio vuoto o di soli spazi.
    NoMessage,
}

impl Blocked {
    pub fn hint(self) -> String {
        match self {
            Blocked::Busy => tr!("commit-blocked-busy"),
            Blocked::Conflicts => tr!("commit-blocked-conflicts"),
            Blocked::Operation(Operation::Merge) => tr!("commit-blocked-merge"),
            Blocked::Operation(Operation::Rebase) => tr!("commit-blocked-rebase"),
            Blocked::Operation(Operation::CherryPick) => tr!("commit-blocked-cherry-pick"),
            Blocked::Operation(Operation::Revert) => tr!("commit-blocked-revert"),
            Blocked::Operation(Operation::Am) => tr!("commit-blocked-am"),
            Blocked::Operation(_) => tr!("commit-blocked-operation"),
            Blocked::NoCommitToAmend => tr!("commit-blocked-no-commit"),
            Blocked::NothingStaged => tr!("commit-blocked-nothing-staged"),
            Blocked::NoMessage => tr!("commit-blocked-no-message"),
        }
    }
}

/// Ciò che serve per decidere se il pulsante "Commit" è attivo.
#[derive(Debug, Clone, Copy)]
pub struct Gate<'a> {
    pub draft: &'a CommitDraft,
    pub status: Option<&'a Status>,
    /// Quanti file ci sono nella lista "In stage".
    pub staged: usize,
    /// Vero se HEAD punta a un commit.
    pub has_head: bool,
    /// Una scrittura in corso o una conferma aperta.
    pub busy: bool,
}

impl Gate<'_> {
    /// `Ok` se si può committare, altrimenti il primo motivo per cui no.
    pub fn check(&self) -> Result<(), Blocked> {
        if self.busy {
            return Err(Blocked::Busy);
        }
        // Un merge in corso si conclude con `merge.continue` (il riquadro dell'operazione),
        // che entra nel diario come merge; il commit della casella resta per i commit normali.
        match self.status {
            Some(status) if status.has_conflicts() => return Err(Blocked::Conflicts),
            // Durante un bisect si committa normalmente.
            Some(Status {
                operation: Some(op),
                ..
            }) if *op != Operation::Bisect => return Err(Blocked::Operation(*op)),
            _ => {}
        }
        if self.draft.amend() {
            if !self.has_head {
                return Err(Blocked::NoCommitToAmend);
            }
        } else if self.staged == 0 {
            return Err(Blocked::NothingStaged);
        }
        if self.draft.is_blank() {
            return Err(Blocked::NoMessage);
        }
        Ok(())
    }
}

/// Un commit mandato (o da mandare) a `commit.create`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSend {
    pub message: String,
    pub amend: bool,
}

impl CommitSend {
    pub fn new(draft: &CommitDraft) -> Self {
        Self {
            message: draft.message(),
            amend: draft.amend(),
        }
    }

    /// L'id del comando del Registry.
    pub fn command(&self) -> &'static str {
        "commit.create"
    }

    /// I parametri di `commit.create`; `confirm` solo dopo la conferma dell'utente.
    pub fn params(&self, confirm: bool) -> Value {
        let mut params = json!({ "message": self.message });
        if self.amend {
            params["amend"] = json!(true);
        }
        if confirm {
            params["confirm"] = json!(true);
        }
        params
    }

    /// Nome dell'operazione per la barra di stato: "Commit in corso…".
    pub fn label(&self) -> String {
        if self.amend {
            tr!("commit-amend-label")
        } else {
            tr!("commit-label")
        }
    }
}

/// Risultato di `commit.create` (il campo `branch` non serve alla UI: i ref si rileggono).
#[derive(Debug, Clone, Deserialize)]
pub struct CommitResult {
    pub commit: Commit,
    /// HEAD prima del commit (con amend, il commit riscritto).
    pub previous: Option<Oid>,
    /// Con amend, il ref di backup del commit riscritto.
    pub backup: Option<String>,
    /// Con amend, gli altri branch e tag rimasti sul commit riscritto.
    #[serde(default)]
    pub also_on: Vec<String>,
    pub status: Status,
}

impl CommitResult {
    /// Il messaggio per la barra di stato.
    pub fn notice(&self) -> String {
        // Amend che ha prodotto lo stesso commit: il motore non ha cambiato niente.
        if self.previous == Some(self.commit.id) && self.backup.is_none() {
            return tr!("commit-unchanged", id = self.commit.id.short());
        }
        let id = self.commit.id.short();
        let mut text = match (self.previous, &self.backup) {
            (Some(previous), Some(backup)) => tr!(
                "commit-amended",
                id = id,
                previous = previous.short(),
                backup = backup
            ),
            (None, Some(backup)) => tr!("commit-created-backup", id = id, backup = backup),
            (_, None) => tr!("commit-created", id = id),
        };
        if !self.also_on.is_empty() {
            text.push_str(&tr!("commit-also-on", refs = self.also_on.join(", ")));
        }
        text
    }
}

/// Gli altri branch locali e tag che puntano proprio a `head` (nomi completi): con amend
/// restano sul commit vecchio. Solo quelli che puntano direttamente a HEAD, dai ref già letti;
/// l'elenco completo (anche i ref più avanti) arriva in `also_on` dopo l'amend.
pub fn refs_left_behind(
    refs: &[project_domain::RefInfo],
    head: Option<Oid>,
    branch: Option<&str>,
) -> Vec<String> {
    use project_domain::RefKind;
    let Some(head) = head else {
        return Vec::new();
    };
    refs.iter()
        .filter(|r| r.target == head)
        .filter(|r| match r.kind {
            RefKind::LocalBranch => Some(r.name.as_str()) != branch,
            RefKind::Tag => true,
            _ => false,
        })
        .map(|r| r.name.clone())
        .collect()
}

/// Un amend di un commit già pubblicato, in attesa della conferma dell'utente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmAmend {
    pub send: CommitSend,
    /// Il motivo dato dal motore (es. i branch remoti che contengono il commit).
    pub reason: String,
}

/// L'errore dell'ultimo commit, mostrato sotto la casella del messaggio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitError {
    pub message: String,
    /// L'output completo di git (hook, firma, identità), da mostrare in un'area scorrevole.
    pub output: Option<String>,
}

/// La casella del messaggio: la bozza, il commit in corso, la conferma e l'ultimo errore.
#[derive(Debug, Clone, Default)]
pub struct CommitBox {
    pub draft: CommitDraft,
    sending: Option<CommitSend>,
    confirm: Option<ConfirmAmend>,
    error: Option<CommitError>,
}

impl CommitBox {
    /// Il commit in corso, se c'è.
    pub fn sending(&self) -> Option<&CommitSend> {
        self.sending.as_ref()
    }

    pub fn confirm(&self) -> Option<&ConfirmAmend> {
        self.confirm.as_ref()
    }

    pub fn error(&self) -> Option<&CommitError> {
        self.error.as_ref()
    }

    pub fn dismiss_error(&mut self) {
        self.error = None;
    }

    /// Prepara il commit della bozza: restituisce il comando da mandare e i parametri.
    /// `None` se c'è già un commit in corso o una conferma aperta.
    pub fn start(&mut self) -> Option<(CommitSend, Value)> {
        if self.sending.is_some() || self.confirm.is_some() {
            return None;
        }
        let send = CommitSend::new(&self.draft);
        Some(self.begin(send, false))
    }

    /// L'utente ha confermato l'amend: lo stesso commit, con `"confirm": true`.
    pub fn confirmed(&mut self) -> Option<(CommitSend, Value)> {
        let ConfirmAmend { send, .. } = self.confirm.take()?;
        Some(self.begin(send, true))
    }

    pub fn cancel_confirm(&mut self) {
        self.confirm = None;
    }

    fn begin(&mut self, send: CommitSend, confirm: bool) -> (CommitSend, Value) {
        self.error = None;
        let params = send.params(confirm);
        self.sending = Some(send.clone());
        (send, params)
    }

    /// Il commit è riuscito: la bozza si svuota (e amend si disattiva).
    pub fn succeeded(&mut self) {
        self.sending = None;
        self.error = None;
        self.draft.clear();
    }

    /// Il commit è riuscito ma il motore segnala un problema dopo (`PostCommit`): la bozza si
    /// svuota come dopo un successo (il commit non va ripetuto) e l'errore si mostra.
    pub fn done_with_error(&mut self, message: &str) {
        self.succeeded();
        self.error = Some(CommitError {
            message: format!("{message}."),
            output: None,
        });
    }

    /// Il commit è fallito. La bozza resta com'era. Con [`ErrorKind::ConfirmRequired`] si apre
    /// la conferma; gli altri errori si mostrano sotto la casella.
    pub fn failed(&mut self, kind: Option<ErrorKind>, message: &str, detail: Option<String>) {
        let send = self.sending.take();
        match (kind, send) {
            (Some(ErrorKind::ConfirmRequired), Some(send)) => {
                self.confirm = Some(ConfirmAmend {
                    send,
                    reason: detail.unwrap_or_else(|| message.to_owned()),
                });
            }
            _ => self.error = Some(error(kind, message, detail)),
        }
    }
}

/// Il testo dell'errore per l'utente; il messaggio scritto non si perde mai.
fn error(kind: Option<ErrorKind>, message: &str, detail: Option<String>) -> CommitError {
    match kind {
        Some(ErrorKind::Git) => CommitError {
            message: tr!("commit-git-failed"),
            output: Some(
                detail
                    .filter(|out| !out.trim().is_empty())
                    .unwrap_or_else(|| tr!("commit-no-output")),
            ),
        },
        _ => CommitError {
            message: tr!("commit-failed-kept", error = message),
            output: None,
        },
    }
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_commit(&mut self, action: Action) {
        match action {
            Action::SetAmend(on) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if repo.commit.sending().is_some() {
                    return;
                }
                let last = repo.head_commit().cloned();
                let ask = repo.commit.draft.set_amend(on, last.as_ref());
                if ask && let Some(head) = repo.head.target() {
                    let params = json!({ "id": head.to_string() });
                    self.submit("commit.show", params, Request::HeadMessage(generation));
                }
            }
            Action::Commit => self.send_commit(false),
            Action::ConfirmCommit => self.send_commit(true),
            Action::CancelCommit => {
                if let Some(repo) = &mut self.repo {
                    repo.commit.cancel_confirm();
                }
            }
            Action::DismissCommitError => {
                if let Some(repo) = &mut self.repo {
                    repo.commit.dismiss_error();
                }
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Manda il commit della bozza (`confirmed`: quello in attesa di conferma, con
    /// `"confirm": true`), se il pulsante è attivo: mai insieme a un'altra scrittura.
    pub(crate) fn send_commit(&mut self, confirmed: bool) {
        let writing = self.executor.writing().is_some();
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        let request = if confirmed {
            if writing {
                return;
            }
            repo.commit.confirmed()
        } else {
            if repo.commit_gate(writing).check().is_err() {
                return;
            }
            repo.commit.start()
        };
        let Some((send, params)) = request else {
            return;
        };
        repo.write_error = None;
        repo.write_label = Some(send.label());
        self.submit(send.command(), params, Request::Commit(generation));
    }

    /// Risultato di `commit.create`: stato dalla risposta, log e ref riletti; oppure l'errore
    /// (o la conferma) nella casella, con il messaggio scritto conservato.
    pub(crate) fn receive_commit(&mut self, generation: u64, done: Done) {
        let result = done.result.and_then(|value| {
            serde_json::from_value::<CommitResult>(value).map_err(|e| {
                tr!(
                    "commit-unexpected-result",
                    command = &done.command,
                    error = e.to_string()
                )
            })
        });
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match result {
            Ok(result) => {
                repo.commit.succeeded();
                let notice = result.notice();
                let id = result.commit.id;
                repo.set_status(result.status);
                // Con altre modifiche si resta sulla riga delle modifiche; se è sparita si
                // seleziona il nuovo commit appena arriva il log.
                if !repo.worktree_selected {
                    repo.select_after_load = Some(id);
                }
                self.error.clear();
                self.notice = notice;
                self.follow_diff();
                // Il nuovo commit in cima (con amend il vecchio sparisce) e le etichette dei
                // ref spostate.
                self.submit("repo.refs", json!({}), Request::Refs(generation));
                // Il commit è nel diario: Annulla ora lo propone.
                self.submit("edit.history", json!({}), Request::History(generation));
                if let Some(repo) = self.current(generation) {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            Err(e) if done.error_kind == Some(ErrorKind::PostCommit) => {
                // Il commit c'è: si rilegge tutto, e non si ripropone il messaggio.
                log::warn!("{}: {e}", done.command);
                repo.commit.done_with_error(&e);
                self.error = e;
                self.notice.clear();
                self.load();
            }
            Err(e) => {
                log::warn!("{}: {e}", done.command);
                repo.commit
                    .failed(done.error_kind, &e, done.error_detail.clone());
                if done.error_kind == Some(ErrorKind::ConfirmRequired) {
                    self.error.clear();
                } else {
                    self.error = tr!("commit-failed", error = &e);
                }
                self.notice.clear();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, EntryKind, Head, Signature, StatusEntry, UnmergedInfo};

    fn commit(summary: &str, body: &str) -> Commit {
        let sig = Signature {
            name: "Ada Rossi".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 0,
            offset_min: 0,
        };
        Commit {
            id: Oid::from_bytes([7; 20]),
            parents: Vec::new(),
            author: sig.clone(),
            committer: sig,
            summary: summary.to_owned(),
            body: body.to_owned(),
        }
    }

    fn draft(subject: &str, body: &str) -> CommitDraft {
        CommitDraft {
            subject: subject.to_owned(),
            body: body.to_owned(),
            ..CommitDraft::default()
        }
    }

    fn status(entries: Vec<StatusEntry>, operation: Option<Operation>) -> Status {
        Status {
            head: Head::Branch {
                name: "main".to_owned(),
                target: None,
            },
            upstream: None,
            entries,
            operation,
        }
    }

    fn conflict() -> StatusEntry {
        let zero = Oid::from_bytes([0; 20]);
        StatusEntry {
            path: "a.rs".into(),
            index: Change::Unmerged,
            worktree: Change::Unmerged,
            kind: EntryKind::Unmerged(UnmergedInfo {
                conflict: project_domain::Conflict::BothModified,
                submodule: None,
                mode_base: 0o100644,
                mode_ours: 0o100644,
                mode_theirs: 0o100644,
                mode_worktree: 0o100644,
                oid_base: zero,
                oid_ours: zero,
                oid_theirs: zero,
            }),
        }
    }

    #[test]
    fn message_joins_subject_and_body() {
        assert_eq!(draft("Oggetto  ", "").message(), "Oggetto");
        assert_eq!(draft("Oggetto", " \n\n").message(), "Oggetto");
        assert_eq!(
            draft("Oggetto", "Riga 1\n\nRiga 2\n\n").message(),
            "Oggetto\n\nRiga 1\n\nRiga 2"
        );
        // Senza oggetto il corpo passa com'è: git usa la sua prima riga come oggetto.
        assert_eq!(draft("", "solo corpo").message(), "\n\nsolo corpo");
        assert!(draft(" ", "\n\t").is_blank());
        assert!(!draft("", "x").is_blank());
    }

    #[test]
    fn subject_counter_counts_characters() {
        assert_eq!(draft("è già", "").subject_len(), 5);
        assert_eq!(draft("abc   ", "").subject_len(), 3);
        assert!(!draft(&"a".repeat(72), "").subject_too_long());
        assert!(draft(&"à".repeat(73), "").subject_too_long());
    }

    #[test]
    fn amend_proposes_the_last_message() {
        let last = commit("Ultimo commit", "Con un corpo");
        let mut d = CommitDraft::default();
        assert!(!d.set_amend(true, Some(&last)));
        assert!(d.amend());
        assert_eq!(
            (d.subject.as_str(), d.body.as_str()),
            ("Ultimo commit", "Con un corpo")
        );
        assert!(d.is_proposed());
        // Disattivato senza cambiarlo: la bozza torna vuota.
        d.set_amend(false, Some(&last));
        assert!(d.is_blank());
        assert!(!d.amend());

        // Cambiato: resta.
        d.set_amend(true, Some(&last));
        d.subject.push_str(" corretto");
        assert!(!d.is_proposed());
        d.set_amend(false, None);
        assert_eq!(d.subject, "Ultimo commit corretto");

        // Con un messaggio già scritto non si propone niente.
        let mut d = draft("Mio", "");
        d.set_amend(true, Some(&last));
        assert_eq!(d.subject, "Mio");
        assert!(!d.is_proposed());

        // Ultimo commit non caricato: va chiesto, poi proposto.
        let mut d = CommitDraft::default();
        assert!(d.set_amend(true, None));
        d.propose(&last);
        assert_eq!(d.subject, "Ultimo commit");
        // Arrivato dopo che si è disattivato amend: niente.
        let mut d = CommitDraft::default();
        d.set_amend(true, None);
        d.set_amend(false, None);
        d.propose(&last);
        assert!(d.is_blank());
    }

    #[test]
    fn the_button_explains_why_it_is_disabled() {
        let ready = draft("Messaggio", "");
        let clean = status(Vec::new(), None);
        let gate = |d: &CommitDraft, s: &Status, staged, busy| {
            Gate {
                draft: d,
                status: Some(s),
                staged,
                has_head: true,
                busy,
            }
            .check()
        };
        assert_eq!(gate(&ready, &clean, 1, false), Ok(()));
        assert_eq!(gate(&ready, &clean, 1, true), Err(Blocked::Busy));
        assert_eq!(gate(&ready, &clean, 0, false), Err(Blocked::NothingStaged));
        assert_eq!(
            gate(&draft("  ", "\n"), &clean, 1, false),
            Err(Blocked::NoMessage)
        );
        let conflicts = status(vec![conflict()], Some(Operation::Merge));
        assert_eq!(gate(&ready, &conflicts, 1, false), Err(Blocked::Conflicts));
        let rebase = status(Vec::new(), Some(Operation::Rebase));
        assert_eq!(
            gate(&ready, &rebase, 1, false),
            Err(Blocked::Operation(Operation::Rebase))
        );
        let am = status(Vec::new(), Some(Operation::Am));
        assert_eq!(
            gate(&ready, &am, 1, false),
            Err(Blocked::Operation(Operation::Am))
        );
        // Merge risolto: si conclude dal riquadro dell'operazione, non con un commit.
        let merge = status(Vec::new(), Some(Operation::Merge));
        assert_eq!(
            gate(&ready, &merge, 0, false),
            Err(Blocked::Operation(Operation::Merge))
        );

        let mut amend = ready.clone();
        amend.set_amend(true, None);
        assert_eq!(gate(&amend, &clean, 0, false), Ok(()));
        assert_eq!(
            gate(&amend, &merge, 0, false),
            Err(Blocked::Operation(Operation::Merge))
        );
        let no_head = Gate {
            draft: &amend,
            status: Some(&clean),
            staged: 0,
            has_head: false,
            busy: false,
        };
        assert_eq!(no_head.check(), Err(Blocked::NoCommitToAmend));
        for blocked in [Blocked::Busy, Blocked::NothingStaged, Blocked::NoMessage] {
            assert!(!blocked.hint().is_empty());
        }
    }

    #[test]
    fn notice_after_commit_and_amend() {
        let status = status(Vec::new(), None);
        let c = commit("x", "");
        let other = Oid::from_bytes([8; 20]);
        let result = |previous, backup: Option<&str>, also_on: Vec<&str>| CommitResult {
            commit: c.clone(),
            previous,
            backup: backup.map(str::to_owned),
            also_on: also_on.into_iter().map(str::to_owned).collect(),
            status: status.clone(),
        };
        let id = c.id.short();
        assert_eq!(
            result(Some(other), None, vec![]).notice(),
            format!("Commit {id} creato")
        );
        let amend = result(
            Some(other),
            Some("refs/project/backup/amend-1"),
            vec!["refs/tags/v1"],
        );
        assert_eq!(
            amend.notice(),
            format!(
                "Commit {id} creato al posto di {}; backup del commit precedente in \
                 refs/project/backup/amend-1; il commit precedente resta su refs/tags/v1",
                other.short()
            )
        );
        let same = result(Some(c.id), None, vec![]);
        assert!(
            same.notice().starts_with("Nessun cambiamento"),
            "{}",
            same.notice()
        );
    }

    #[test]
    fn refs_left_behind_by_an_amend() {
        use project_domain::{RefInfo, RefKind};
        let head = Oid::from_bytes([1; 20]);
        let r = |name: &str, kind, target| RefInfo {
            name: name.to_owned(),
            kind,
            target,
            tag_object: None,
            upstream: None,
        };
        let refs = [
            r("refs/heads/altro", RefKind::LocalBranch, head),
            r("refs/heads/main", RefKind::LocalBranch, head),
            r(
                "refs/heads/vecchio",
                RefKind::LocalBranch,
                Oid::from_bytes([2; 20]),
            ),
            r("refs/remotes/origin/main", RefKind::RemoteBranch, head),
            r("refs/tags/v1", RefKind::Tag, head),
        ];
        assert_eq!(
            refs_left_behind(&refs, Some(head), Some("refs/heads/main")),
            ["refs/heads/altro", "refs/tags/v1"]
        );
        assert!(refs_left_behind(&refs, None, None).is_empty());
    }

    #[test]
    fn params_carry_amend_and_confirm_only_when_set() {
        let mut d = draft("Oggetto", "Corpo");
        let send = CommitSend::new(&d);
        assert_eq!(send.command(), "commit.create");
        assert_eq!(send.params(false), json!({ "message": "Oggetto\n\nCorpo" }));
        assert_eq!(send.label(), "Commit");
        d.set_amend(true, None);
        let send = CommitSend::new(&d);
        assert_eq!(
            send.params(true),
            json!({ "message": "Oggetto\n\nCorpo", "amend": true, "confirm": true })
        );
        assert_eq!(send.label(), "Amend dell'ultimo commit");
    }

    #[test]
    fn errors_keep_the_draft() {
        let mut b = CommitBox {
            draft: draft("Oggetto", ""),
            ..CommitBox::default()
        };
        let (_, params) = b.start().unwrap();
        assert_eq!(params, json!({ "message": "Oggetto" }));
        assert!(b.sending().is_some());
        assert!(b.start().is_none(), "mai due commit insieme");

        b.failed(
            Some(ErrorKind::Git),
            "`git commit` non riuscito: ...",
            Some("pre-commit: trovato TODO\nin a.rs:3".to_owned()),
        );
        assert!(b.sending().is_none());
        assert_eq!(b.draft.subject, "Oggetto");
        let error = b.error().unwrap();
        assert!(error.message.contains("hook"), "{}", error.message);
        assert_eq!(
            error.output.as_deref(),
            Some("pre-commit: trovato TODO\nin a.rs:3")
        );

        b.start().unwrap();
        assert!(b.error().is_none(), "un nuovo tentativo toglie l'errore");
        b.failed(
            Some(ErrorKind::NothingToCommit),
            "niente da committare",
            None,
        );
        let error = b.error().unwrap();
        assert!(error.message.contains("niente da committare"));
        assert!(error.message.contains("conservato"));
        assert_eq!(error.output, None);
        b.dismiss_error();
        assert!(b.error().is_none());

        b.start().unwrap();
        b.failed(Some(ErrorKind::Git), "x", Some("  \n".to_owned()));
        assert_eq!(
            b.error().unwrap().output.as_deref(),
            Some("(nessun output)")
        );

        // Commit riuscito con un problema dopo: non va ripetuto, la bozza si svuota.
        b.start().unwrap();
        b.done_with_error("il commit abc è stato creato, ma HEAD è cambiato");
        assert!(b.draft.is_blank());
        assert!(b.sending().is_none());
        assert!(b.error().unwrap().message.contains("è stato creato"));
    }

    #[test]
    fn a_published_amend_asks_then_resends_with_confirm() {
        let mut b = CommitBox::default();
        b.draft.subject = "Riscrivo".to_owned();
        b.draft.set_amend(true, None);
        b.start().unwrap();
        b.failed(
            Some(ErrorKind::ConfirmRequired),
            "chiede una conferma",
            Some("il commit è già su refs/remotes/origin/main".to_owned()),
        );
        let confirm = b.confirm().unwrap();
        assert_eq!(
            confirm.reason,
            "il commit è già su refs/remotes/origin/main"
        );
        assert!(b.error().is_none());
        assert!(b.start().is_none(), "niente commit con la conferma aperta");

        // Annullata: niente da mandare, la bozza resta.
        b.cancel_confirm();
        assert!(b.confirm().is_none());
        assert!(b.draft.amend());

        b.start().unwrap();
        b.failed(Some(ErrorKind::ConfirmRequired), "m", Some("r".to_owned()));
        let (send, params) = b.confirmed().unwrap();
        assert!(send.amend);
        assert_eq!(
            params,
            json!({ "message": "Riscrivo", "amend": true, "confirm": true })
        );
        b.succeeded();
        assert!(b.draft.is_blank());
        assert!(!b.draft.amend());
        assert!(b.sending().is_none());
    }

    /// Il percorso completo con il repo di esempio del `MemoryBackend`, come lo fa la UI:
    /// stage di note.txt, commit, poi il log e lo stato dalla risposta; infine amend.
    #[test]
    fn commit_and_amend_with_the_sample_repo() {
        use crate::stage::StageLists;
        use project_engine::{EngineError, MemoryOpener, Registry, Session};
        use std::sync::Arc;

        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(MemoryOpener::sample()));
        let path = MemoryOpener::SAMPLE_PATH;
        registry
            .run("repo.open", &mut session, json!({ "path": path }))
            .unwrap();
        let out = registry
            .run(
                "index.stage",
                &mut session,
                json!({ "paths": ["note.txt"] }),
            )
            .unwrap();
        let status: Status = serde_json::from_value(out["status"].clone()).unwrap();
        let lists = StageLists::new(&status);
        let head = status.head.target();

        let mut b = CommitBox::default();
        b.draft.subject = "Aggiunge gli appunti".to_owned();
        b.draft.body = "Con README e guida.".to_owned();
        let gate = Gate {
            draft: &b.draft,
            status: Some(&status),
            staged: lists.staged.len(),
            has_head: head.is_some(),
            busy: false,
        };
        assert_eq!(gate.check(), Ok(()));
        let (send, params) = b.start().unwrap();
        let out = registry.run(send.command(), &mut session, params).unwrap();
        let result: CommitResult = serde_json::from_value(out).unwrap();
        b.succeeded();
        assert!(b.draft.is_blank());
        assert_eq!(result.previous, head);
        assert_eq!(result.backup, None);
        assert_eq!(result.commit.summary, "Aggiunge gli appunti");
        assert_eq!(result.commit.author.name, "Ada Rossi");
        assert_eq!(
            result.notice(),
            format!("Commit {} creato", result.commit.id.short())
        );
        // Lo stato dalla risposta: niente più in stage.
        let lists = StageLists::new(&result.status);
        assert!(lists.staged.is_empty());
        assert_eq!(result.status.head.target(), Some(result.commit.id));
        // Il log ricaricato ha il nuovo commit in cima.
        let log = registry
            .run("repo.log", &mut session, json!({ "limit": 2 }))
            .unwrap();
        let log: Vec<Commit> = serde_json::from_value(log["commits"].clone()).unwrap();
        assert_eq!(log[0], result.commit);

        // Un altro commit senza niente in stage: il pulsante è disabilitato, e il motore
        // comunque rifiuterebbe.
        b.draft.subject = "Ancora".to_owned();
        let gate = Gate {
            draft: &b.draft,
            status: Some(&result.status),
            staged: 0,
            has_head: true,
            busy: false,
        };
        assert_eq!(gate.check(), Err(Blocked::NothingStaged));
        let err = registry
            .run(
                "commit.create",
                &mut session,
                CommitSend::new(&b.draft).params(false),
            )
            .unwrap_err();
        assert!(matches!(err, EngineError::NothingToCommit), "{err}");
        b.failed(
            Some(ErrorKind::of(&err)),
            &err.to_string(),
            ErrorKind::detail(&err),
        );
        assert_eq!(b.draft.subject, "Ancora", "il messaggio non si perde");

        // Amend: proposto il messaggio dell'ultimo commit, poi cambiato.
        b.draft.clear();
        b.draft.set_amend(true, Some(&log[0]));
        assert_eq!(b.draft.subject, "Aggiunge gli appunti");
        b.draft.subject = "Aggiunge gli appunti di lavoro".to_owned();
        let (send, params) = b.start().unwrap();
        assert_eq!(params["amend"], true);
        let out = registry.run(send.command(), &mut session, params).unwrap();
        let amended: CommitResult = serde_json::from_value(out).unwrap();
        b.succeeded();
        assert_eq!(amended.previous, Some(result.commit.id));
        let backup = amended.backup.clone().expect("backup del commit riscritto");
        assert!(amended.notice().contains(&backup), "{}", amended.notice());
        assert_eq!(amended.commit.parents, result.commit.parents);
        let log = registry
            .run("repo.log", &mut session, json!({ "limit": 2 }))
            .unwrap();
        let log: Vec<Commit> = serde_json::from_value(log["commits"].clone()).unwrap();
        // Il vecchio commit sparisce dal log, il nuovo è in cima.
        assert_eq!(log[0].id, amended.commit.id);
        assert_eq!(log[0].summary, "Aggiunge gli appunti di lavoro");
        assert!(log.iter().all(|c| c.id != result.commit.id));
    }
}
