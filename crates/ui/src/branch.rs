//! Branch locali dal menu della colonna dei ref (S24): `branch.create`, `branch.checkout`,
//! `branch.delete`, `branch.rename`. Solo dati, senza egui: si prova con test unitari.
//!
//! Il nome di un branch nuovo (o rinominato) si scrive in un piccolo dialogo; l'unico controllo
//! qui è che non sia vuoto: il motore decide se è valido, e il suo errore resta nel dialogo.
//! L'eliminazione va in due tempi come lo scarto: prima senza `confirm`, che elimina subito un
//! branch unito oppure risponde con `ConfirmRequired`; solo dopo il clic nel dialogo di
//! conferma, `"confirm": true`. Un checkout rifiutato perché sovrascriverebbe modifiche locali
//! apre un dialogo con i file.

use crate::ErrorKind;
use crate::discard::{History, Phase};
use crate::{Action, App, Done, Request, branch};
use project_domain::{Oid, RefInfo, RefKind, Status};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// Il nome breve di un branch locale: senza `refs/heads/`.
pub fn short(full: &str) -> &str {
    full.strip_prefix("refs/heads/").unwrap_or(full)
}

/// Un comando sui branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Create,
    Checkout,
    Delete,
    Rename,
}

impl Op {
    /// L'id del comando del Registry.
    pub fn command(self) -> &'static str {
        match self {
            Op::Create => "branch.create",
            Op::Checkout => "branch.checkout",
            Op::Delete => "branch.delete",
            Op::Rename => "branch.rename",
        }
    }
}

/// Da dove parte un branch nuovo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    /// Il commit, sempre per id completo: è quello che l'utente ha visto, anche se nel
    /// frattempo il ref si è spostato.
    pub target: Oid,
    /// Come mostrarlo nel dialogo, es. `origin/main` o `commit 1a2b3c4`.
    pub label: String,
}

impl Start {
    /// Il punto di partenza del ref `info` e il nome proposto: il nome del branch remoto senza
    /// il remoto (`origin/feature/x` → `feature/x`); vuoto per branch locali e tag (lo stesso
    /// nome sarebbe già preso, o ambiguo con il tag).
    pub fn from_ref(info: &RefInfo) -> (Self, String) {
        let label = info.short_name().to_owned();
        let name = match info.kind {
            RefKind::RemoteBranch => label
                .split_once('/')
                .map(|(_, rest)| rest)
                .filter(|rest| !rest.is_empty() && *rest != "HEAD")
                .unwrap_or_default()
                .to_owned(),
            RefKind::LocalBranch | RefKind::Tag | RefKind::Stash => String::new(),
        };
        let start = Start {
            target: info.target,
            label,
        };
        (start, name)
    }

    /// Il punto di partenza di un commit della lista.
    pub fn from_commit(id: Oid) -> Self {
        Start {
            target: id,
            label: tr!("branch-start-commit", id = id.short()),
        }
    }
}

/// Il dialogo del nome: branch nuovo o rinomina.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameKind {
    Create(Start),
    /// Il nome breve attuale.
    Rename(String),
}

/// Il dialogo per scrivere il nome di un branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameDialog {
    pub kind: NameKind,
    /// Il testo scritto.
    pub name: String,
    /// L'errore del motore all'ultimo invio, mostrato nel dialogo.
    pub error: Option<String>,
    /// Vero mentre il comando è in corso.
    pub sending: bool,
}

impl NameDialog {
    /// Il nome da mandare: senza spazi ai lati.
    fn trimmed(&self) -> &str {
        self.name.trim()
    }

    /// Vero se il pulsante è attivo: un nome scritto (e, per la rinomina, diverso da quello
    /// attuale), nessun invio in corso. Il resto lo controlla il motore.
    pub fn can_submit(&self) -> bool {
        let name = self.trimmed();
        !self.sending
            && !name.is_empty()
            && match &self.kind {
                NameKind::Create(_) => true,
                NameKind::Rename(old) => name != old,
            }
    }

    fn op(&self) -> Op {
        match self.kind {
            NameKind::Create(_) => Op::Create,
            NameKind::Rename(_) => Op::Rename,
        }
    }

    fn params(&self) -> Value {
        let name = self.trimmed();
        match &self.kind {
            NameKind::Create(start) => json!({ "name": name, "start": start.target.to_string() }),
            NameKind::Rename(old) => json!({ "name": old, "new_name": name }),
        }
    }

    /// La descrizione dell'operazione per la barra di stato.
    pub fn label(&self) -> String {
        match &self.kind {
            NameKind::Create(_) => tr!("branch-create-label", name = self.trimmed()),
            NameKind::Rename(old) => tr!("branch-rename-label", name = old),
        }
    }
}

/// L'eliminazione di un branch, dal controllo alla risposta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteDialog {
    /// Il nome breve.
    pub name: String,
    /// Il commit del branch come lo vedeva l'utente: va al motore come `expected` in entrambi
    /// gli invii, così la conferma vale solo per quel commit.
    pub target: Oid,
    pub phase: Phase,
}

/// Il checkout rifiutato: sovrascriverebbe modifiche locali.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    /// Il nome breve del branch.
    pub name: String,
    /// I file indicati da git.
    pub files: Vec<String>,
}

/// Il dialogo dei branch aperto (o l'eliminazione in controllo, ancora senza dialogo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Name(NameDialog),
    Delete(DeleteDialog),
    Refused(Refused),
}

/// I comandi sui branch: un dialogo alla volta, un comando alla volta.
#[derive(Debug, Default)]
pub struct BranchBox {
    dialog: Option<Dialog>,
    /// Il checkout in corso, nome breve (non ha dialogo).
    checkout: Option<String>,
}

/// Cosa è arrivato da un comando `branch.*`.
#[derive(Debug)]
pub enum Received {
    /// Fatto: cosa aggiornare nella vista.
    Done(Box<Outcome>),
    /// Il nome non va: l'errore è nel dialogo, che resta aperto.
    Inline,
    /// Serve la conferma dell'eliminazione: il dialogo si apre.
    Confirm,
    /// Checkout rifiutato per le modifiche locali: il dialogo con i file si apre.
    Refused,
    /// Rifiutato o non riuscito: il messaggio da mostrare; con `reload` si rilegge tutto
    /// (l'operazione potrebbe essere fatta, o a metà).
    Failed { message: String, reload: bool },
}

/// Il risultato di un comando riuscito, per la vista.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub op: Op,
    pub notice: String,
    /// I ref nuovi, se il comando li ha riletti.
    pub refs: Option<Vec<RefInfo>>,
    /// Il nuovo stato (checkout).
    pub status: Option<Status>,
    /// Un problema dopo l'operazione riuscita (ref o stato non riletti).
    pub warning: Option<String>,
    pub history: History,
}

impl BranchBox {
    /// Il dialogo da disegnare: l'eliminazione si vede solo quando chiede la conferma.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Delete(delete) => matches!(delete.phase, Phase::Confirm { .. }),
            Dialog::Name(_) | Dialog::Refused(_) => true,
        })
    }

    /// Vero se un dialogo è aperto: niente altre scritture, i tasti vanno a lui.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// Vero se c'è un dialogo o un comando dei branch in corso: non se ne apre un altro.
    fn busy(&self) -> bool {
        self.dialog.is_some() || self.checkout.is_some()
    }

    /// Apre il dialogo per un branch nuovo da `start`, con il nome proposto `name`.
    pub fn open_create(&mut self, start: Start, name: String) -> bool {
        self.open_name(NameKind::Create(start), name)
    }

    /// Apre il dialogo per rinominare il branch `name` (breve).
    pub fn open_rename(&mut self, name: String) -> bool {
        self.open_name(NameKind::Rename(name.clone()), name)
    }

    fn open_name(&mut self, kind: NameKind, name: String) -> bool {
        if self.busy() {
            return false;
        }
        self.dialog = Some(Dialog::Name(NameDialog {
            kind,
            name,
            error: None,
            sending: false,
        }));
        true
    }

    /// Il testo del dialogo del nome è cambiato.
    pub fn set_name(&mut self, name: String) {
        if let Some(Dialog::Name(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.name = name;
        }
    }

    /// Invio del dialogo del nome: comando, parametri e descrizione. `None` se non si può.
    pub fn submit_name(&mut self) -> Option<(Op, Value, String)> {
        let Some(Dialog::Name(dialog)) = &mut self.dialog else {
            return None;
        };
        if !dialog.can_submit() {
            return None;
        }
        dialog.sending = true;
        dialog.error = None;
        Some((dialog.op(), dialog.params(), dialog.label()))
    }

    /// Comincia il checkout del branch `name` (breve).
    pub fn start_checkout(&mut self, name: String) -> Option<Value> {
        if self.busy() {
            return None;
        }
        let params = json!({ "name": name });
        self.checkout = Some(name);
        Some(params)
    }

    /// Comincia l'eliminazione del branch `name` (breve) che l'utente vede su `target`: i
    /// parametri del primo invio, senza `confirm`.
    pub fn start_delete(&mut self, name: String, target: Oid) -> Option<Value> {
        if self.busy() {
            return None;
        }
        let params = json!({ "name": name, "expected": target.to_string() });
        self.dialog = Some(Dialog::Delete(DeleteDialog {
            name,
            target,
            phase: Phase::Checking,
        }));
        Some(params)
    }

    /// L'utente ha confermato l'eliminazione: i parametri con `"confirm": true`.
    pub fn confirm_delete(&mut self) -> Option<Value> {
        let Some(Dialog::Delete(delete)) = &mut self.dialog else {
            return None;
        };
        if !matches!(delete.phase, Phase::Confirm { .. }) {
            return None;
        }
        delete.phase = Phase::Sending;
        Some(json!({
            "name": delete.name,
            "confirm": true,
            "expected": delete.target.to_string(),
        }))
    }

    /// Chiude il dialogo aperto senza mandare niente; non durante un invio.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Name(dialog)) => !dialog.sending,
            Some(Dialog::Delete(delete)) => matches!(delete.phase, Phase::Confirm { .. }),
            Some(Dialog::Refused(_)) => true,
            None => false,
        };
        if closable {
            self.dialog = None;
        }
    }

    /// La risposta del comando `op`.
    pub fn receive(
        &mut self,
        op: Op,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        match op {
            Op::Create | Op::Rename => self.receive_name(op, result),
            Op::Checkout => self.receive_checkout(result, kind, detail),
            Op::Delete => self.receive_delete(result, kind, detail),
        }
    }

    fn receive_name(&mut self, op: Op, result: Result<Value, String>) -> Received {
        let Some(Dialog::Name(dialog)) = &mut self.dialog else {
            return unexpected(op);
        };
        if !dialog.sending || dialog.op() != op {
            return unexpected(op);
        }
        match result {
            Err(e) => {
                // Qualunque rifiuto (nome non valido, già preso, rev sparita) resta nel dialogo:
                // niente è cambiato e il nome si può correggere.
                dialog.sending = false;
                dialog.error = Some(e);
                Received::Inline
            }
            Ok(value) => {
                self.dialog = None;
                let outcome = match op {
                    Op::Create => parse::<CreateResult>(op, value).map(CreateResult::outcome),
                    _ => parse::<RenameResult>(op, value).map(RenameResult::outcome),
                };
                done(outcome)
            }
        }
    }

    fn receive_checkout(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(name) = self.checkout.take() else {
            return unexpected(Op::Checkout);
        };
        match result {
            Err(_) if kind == Some(ErrorKind::CheckoutConflict) => {
                let files = detail
                    .unwrap_or_default()
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                self.dialog = Some(Dialog::Refused(Refused { name, files }));
                Received::Refused
            }
            Err(e) => {
                // Rifiutato prima di cominciare: niente è cambiato. Un errore di git invece
                // potrebbe aver lasciato il checkout a metà: si rilegge tutto.
                let refused =
                    matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams));
                let message = if refused {
                    tr!("branch-checkout-refused", name = &name, error = &e)
                } else {
                    tr!("branch-checkout-failed", name = &name, error = &e)
                };
                Received::Failed {
                    message,
                    reload: !refused,
                }
            }
            Ok(value) => done(parse::<CheckoutResult>(Op::Checkout, value).map(|r| r.outcome())),
        }
    }

    fn receive_delete(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Delete(delete)) = self.dialog.take() else {
            return unexpected(Op::Delete);
        };
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && delete.phase == Phase::Checking =>
            {
                self.dialog = Some(Dialog::Delete(DeleteDialog {
                    name: delete.name,
                    target: delete.target,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                }));
                Received::Confirm
            }
            Err(e) => {
                let message = if kind == Some(ErrorKind::NotAllowed) {
                    tr!("branch-delete-refused", name = &delete.name, error = &e)
                } else {
                    tr!("branch-delete-failed", name = &delete.name, error = &e)
                };
                Received::Failed {
                    message,
                    reload: false,
                }
            }
            Ok(value) => done(parse::<DeleteResult>(Op::Delete, value).map(DeleteResult::outcome)),
        }
    }
}

fn unexpected(op: Op) -> Received {
    Received::Failed {
        message: tr!("branch-unexpected-response", command = op.command()),
        reload: true,
    }
}

fn parse<T: for<'de> Deserialize<'de>>(op: Op, value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| {
        tr!(
            "branch-unexpected-result",
            command = op.command(),
            error = e.to_string()
        )
    })
}

/// Un risultato che non si legge: l'operazione è probabilmente fatta, si rilegge tutto.
fn done(outcome: Result<Outcome, String>) -> Received {
    match outcome {
        Ok(outcome) => Received::Done(Box::new(outcome)),
        Err(message) => Received::Failed {
            message,
            reload: true,
        },
    }
}

/// `; attenzione: …` per ogni avviso.
fn warnings_text(warnings: &[String]) -> String {
    warnings
        .iter()
        .map(|w| tr!("branch-warning", warning = w))
        .collect()
}

/// Risultato di `branch.create`.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateResult {
    pub branch: String,
    pub target: Oid,
    pub refs: Option<Vec<RefInfo>>,
    pub refs_error: Option<String>,
    #[serde(flatten)]
    pub history: History,
}

impl CreateResult {
    fn outcome(self) -> Outcome {
        Outcome {
            op: Op::Create,
            notice: tr!(
                "branch-created",
                branch = short(&self.branch),
                id = self.target.short()
            ),
            refs: self.refs,
            status: None,
            warning: self.refs_error.map(refs_warning),
            history: self.history,
        }
    }
}

/// Risultato di `branch.checkout`.
#[derive(Debug, Clone, Deserialize)]
pub struct CheckoutResult {
    pub branch: String,
    pub changed: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub status: Option<Status>,
    pub status_error: Option<String>,
    #[serde(flatten)]
    pub history: History,
}

impl CheckoutResult {
    fn outcome(self) -> Outcome {
        let name = short(&self.branch);
        let notice = if self.changed {
            tr!("branch-checked-out", branch = name)
        } else {
            tr!("branch-already-active", branch = name)
        };
        Outcome {
            op: Op::Checkout,
            notice: notice + &warnings_text(&self.warnings),
            refs: None,
            status: self.status,
            warning: self
                .status_error
                .map(|e| tr!("branch-checkout-status-error", error = e)),
            history: self.history,
        }
    }
}

/// Risultato di `branch.delete`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeleteResult {
    pub branch: String,
    pub target: Oid,
    pub unmerged: usize,
    pub backup: String,
    /// La configurazione `branch.<nome>.*` tolta con il branch (upstream, descrizione):
    /// l'undo la rimette.
    #[serde(default)]
    pub removed_config: Vec<ConfigEntry>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub refs: Option<Vec<RefInfo>>,
    pub refs_error: Option<String>,
    #[serde(flatten)]
    pub history: History,
}

/// Una voce di configurazione tolta da `branch.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ConfigEntry {
    pub key: String,
    pub value: Option<String>,
}

impl DeleteResult {
    /// L'avviso: con la configurazione tolta dice l'upstream se c'era, altrimenti le chiavi.
    fn notice(&self) -> String {
        let branch = short(&self.branch);
        let id = self.target.short();
        let (unmerged, backup) = (self.unmerged, &self.backup);
        let upstream = self
            .removed_config
            .iter()
            .any(|c| c.key.ends_with(".merge") || c.key.ends_with(".remote"));
        if self.removed_config.is_empty() {
            tr!(
                "branch-deleted",
                branch = branch,
                id = id,
                unmerged = unmerged,
                backup = backup
            )
        } else if upstream {
            tr!(
                "branch-deleted-upstream",
                branch = branch,
                id = id,
                unmerged = unmerged,
                backup = backup
            )
        } else {
            let keys: Vec<&str> = self.removed_config.iter().map(|c| c.key.as_str()).collect();
            tr!(
                "branch-deleted-config",
                branch = branch,
                id = id,
                unmerged = unmerged,
                backup = backup,
                keys = keys.join(", ")
            )
        }
    }

    fn outcome(self) -> Outcome {
        Outcome {
            op: Op::Delete,
            notice: self.notice() + &warnings_text(&self.warnings),
            refs: self.refs,
            status: None,
            warning: self.refs_error.map(refs_warning),
            history: self.history,
        }
    }
}

/// Risultato di `branch.rename`.
#[derive(Debug, Clone, Deserialize)]
pub struct RenameResult {
    pub branch: String,
    pub previous: String,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub refs: Option<Vec<RefInfo>>,
    pub refs_error: Option<String>,
    #[serde(flatten)]
    pub history: History,
}

impl RenameResult {
    fn outcome(self) -> Outcome {
        Outcome {
            op: Op::Rename,
            notice: tr!(
                "branch-renamed",
                previous = short(&self.previous),
                branch = short(&self.branch)
            ) + &warnings_text(&self.warnings),
            refs: self.refs,
            status: None,
            warning: self.refs_error.map(refs_warning),
            history: self.history,
        }
    }
}

fn refs_warning(error: String) -> String {
    tr!("branch-refs-warning", error = error)
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_branch(&mut self, action: Action) {
        match action {
            Action::NewBranch(start, name) => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.branch.open_create(start, name);
                }
            }
            Action::RenameBranch(name) => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.branch.open_rename(name);
                }
            }
            Action::BranchName(name) => {
                if let Some(repo) = &mut self.repo {
                    repo.branch.set_name(name);
                }
            }
            Action::SubmitBranchName => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((op, params, label)) = repo.branch.submit_name() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::Checkout(name) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = tr!("branch-checkout-label", name = &name);
                if let Some(params) = repo.branch.start_checkout(name) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = branch::Op::Checkout;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::DeleteBranch(name, target) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = tr!("branch-delete-label", name = &name);
                if let Some(params) = repo.branch.start_delete(name, target) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = branch::Op::Delete;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::ConfirmDeleteBranch => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let label = match repo.branch.dialog() {
                    Some(branch::Dialog::Delete(d)) => tr!("branch-delete-label", name = &d.name),

                    _ => return,
                };
                if let Some(params) = repo.branch.confirm_delete() {
                    repo.write_label = Some(label);
                    let op = branch::Op::Delete;
                    self.submit(op.command(), params, Request::Branch(generation, op));
                }
            }
            Action::CloseBranchDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.branch.close();
                }
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Risposta di `branch.*`: ref e diario dalla risposta, poi il log riletto (un branch
    /// eliminato può portarsi via dei commit); dopo un checkout si rilegge tutto (HEAD, grafo,
    /// area di stage). Un nome rifiutato resta nel suo dialogo.
    pub(crate) fn receive_branch(&mut self, generation: u64, op: branch::Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .branch
            .receive(op, done.result, done.error_kind, done.error_detail)
        {
            branch::Received::Inline | branch::Received::Confirm | branch::Received::Refused => {
                self.error.clear();
                self.notice.clear();
            }
            branch::Received::Done(outcome) => {
                repo.history = outcome.history.clone();
                let refs_read = outcome.refs.is_some();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                if let Some(status) = outcome.status {
                    repo.set_status(status);
                }
                let checkout = outcome.op == branch::Op::Checkout;
                // Dopo un checkout il nuovo HEAD si seleziona appena arriva il log.
                if checkout && !repo.worktree_selected {
                    repo.select_after_load = repo.head.target();
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                if checkout {
                    self.follow_diff();
                    self.load();
                } else {
                    // Di solito i ref arrivano con la risposta: basta il log.
                    if !refs_read {
                        self.submit("repo.refs", json!({}), Request::Refs(generation));
                    }
                    if let Some(repo) = self.current(generation) {
                        let first = repo.list.reset();
                        self.request_page(first);
                    }
                }
            }
            branch::Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    // Un'eliminazione rifiutata (es. il branch non è più sul commit visto): i
                    // ref si rileggono, così la colonna mostra dov'è adesso.
                    if op == branch::Op::Delete {
                        self.submit("repo.refs", json!({}), Request::Refs(generation));
                    }
                    // Il diario si rilegge comunque: costa poco e resta allineato.
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{GitBackend, LogQuery, MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

    fn remote(name: &str) -> RefInfo {
        RefInfo {
            name: format!("refs/remotes/{name}"),
            kind: RefKind::RemoteBranch,
            target: Oid::from_bytes([7; 20]),
            tag_object: None,
            upstream: None,
        }
    }

    #[test]
    fn proposed_names_drop_the_remote() {
        let (start, name) = Start::from_ref(&remote("origin/feature/x"));
        assert_eq!(name, "feature/x");
        assert_eq!(start.label, "origin/feature/x");
        assert_eq!(start.target, Oid::from_bytes([7; 20]));
        assert_eq!(Start::from_ref(&remote("origin/HEAD")).1, "");
        let tag = RefInfo {
            name: "refs/tags/v1".to_owned(),
            kind: RefKind::Tag,
            ..remote("x")
        };
        assert_eq!(Start::from_ref(&tag).1, "");
        let commit = Start::from_commit(Oid::from_bytes([7; 20]));
        assert!(commit.label.starts_with("commit "), "{}", commit.label);
    }

    #[test]
    fn the_name_dialog_sends_only_a_written_name() {
        let mut b = BranchBox::default();
        let start = Start::from_commit(Oid::from_bytes([3; 20]));
        assert!(b.open_create(start.clone(), String::new()));
        // Un dialogo alla volta.
        assert!(!b.open_rename("main".to_owned()));
        assert!(b.modal_open());
        assert_eq!(b.submit_name(), None);
        b.set_name("  feature/y ".to_owned());
        let (op, params, label) = b.submit_name().unwrap();
        assert_eq!(op, Op::Create);
        assert_eq!(
            params,
            json!({ "name": "feature/y", "start": Oid::from_bytes([3; 20]).to_string() })
        );
        assert_eq!(label, "Creazione del branch «feature/y»");
        // Durante l'invio niente si cambia, niente si chiude, niente parte due volte.
        assert_eq!(b.submit_name(), None);
        b.set_name("altro".to_owned());
        b.close();
        let Some(Dialog::Name(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.name, "  feature/y ");

        // L'errore del motore resta nel dialogo, che si può correggere e rimandare.
        let r = b.receive(Op::Create, Err("nome non valido".to_owned()), None, None);
        assert!(matches!(r, Received::Inline));
        let Some(Dialog::Name(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.error.as_deref(), Some("nome non valido"));
        assert!(!dialog.sending);
        b.close();
        assert!(b.dialog().is_none());

        // Rinomina: lo stesso nome non si manda.
        assert!(b.open_rename("main".to_owned()));
        assert_eq!(b.submit_name(), None);
        b.set_name("principale".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        assert_eq!(op, Op::Rename);
        assert_eq!(params, json!({ "name": "main", "new_name": "principale" }));
    }

    #[test]
    fn delete_asks_confirmation_only_when_the_engine_does() {
        let mut b = BranchBox::default();
        let tip = Oid::from_bytes([7; 20]);
        let check = b.start_delete("feature/x".to_owned(), tip).unwrap();
        assert_eq!(
            check,
            json!({ "name": "feature/x", "expected": tip.to_string() })
        );
        // Il controllo non si vede e non si conferma prima della risposta.
        assert!(b.dialog().is_none());
        assert_eq!(b.confirm_delete(), None);
        assert_eq!(b.start_delete("altro".to_owned(), tip), None);
        assert_eq!(b.start_checkout("altro".to_owned()), None);

        let r = b.receive(
            Op::Delete,
            Err(String::new()),
            Some(ErrorKind::ConfirmRequired),
            Some("2 commit non uniti".to_owned()),
        );
        assert!(matches!(r, Received::Confirm));
        let Some(Dialog::Delete(d)) = b.dialog() else {
            panic!()
        };
        assert_eq!(
            d.phase,
            Phase::Confirm {
                reason: "2 commit non uniti".to_owned()
            }
        );
        // Chiudere non manda niente.
        b.close();
        assert!(b.dialog().is_none());
        assert_eq!(b.confirm_delete(), None);

        b.start_delete("feature/x".to_owned(), tip).unwrap();
        b.receive(
            Op::Delete,
            Err(String::new()),
            Some(ErrorKind::ConfirmRequired),
            None,
        );
        let send = b.confirm_delete().unwrap();
        assert_eq!(
            send,
            json!({ "name": "feature/x", "confirm": true, "expected": tip.to_string() })
        );
        assert!(b.dialog().is_none(), "durante l'invio il dialogo è chiuso");
        // Una nuova richiesta di conferma dopo `confirm: true` non riapre il dialogo.
        let r = b.receive(
            Op::Delete,
            Err("x".to_owned()),
            Some(ErrorKind::ConfirmRequired),
            None,
        );
        assert!(matches!(r, Received::Failed { .. }), "{r:?}");
        assert!(b.dialog().is_none());

        // Un rifiuto: il messaggio, niente da rileggere.
        b.start_delete("main".to_owned(), tip).unwrap();
        let r = b.receive(
            Op::Delete,
            Err("è il branch attivo".to_owned()),
            Some(ErrorKind::NotAllowed),
            None,
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert_eq!(
            message,
            "Eliminazione di «main» non possibile: è il branch attivo"
        );
        assert!(!reload);
    }

    #[test]
    fn checkout_conflicts_open_the_list_of_files() {
        let mut b = BranchBox::default();
        assert_eq!(
            b.start_checkout("feature/x".to_owned()).unwrap(),
            json!({ "name": "feature/x" })
        );
        assert!(!b.modal_open());
        let r = b.receive(
            Op::Checkout,
            Err("rifiutato".to_owned()),
            Some(ErrorKind::CheckoutConflict),
            Some("README.md\nsrc/lib.rs".to_owned()),
        );
        assert!(matches!(r, Received::Refused));
        assert_eq!(
            b.dialog(),
            Some(&Dialog::Refused(Refused {
                name: "feature/x".to_owned(),
                files: vec!["README.md".to_owned(), "src/lib.rs".to_owned()],
            }))
        );
        b.close();
        assert!(b.dialog().is_none());

        b.start_checkout("x".to_owned()).unwrap();
        let r = b.receive(
            Op::Checkout,
            Err("merge in corso".to_owned()),
            Some(ErrorKind::NotAllowed),
            None,
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert_eq!(message, "Checkout di «x» non possibile: merge in corso");
        assert!(!reload);
        b.start_checkout("x".to_owned()).unwrap();
        let r = b.receive(
            Op::Checkout,
            Err("git".to_owned()),
            Some(ErrorKind::Git),
            None,
        );
        assert!(matches!(r, Received::Failed { reload: true, .. }), "{r:?}");
    }

    fn open(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        (registry, session)
    }

    /// Il repo di esempio con `esperimento`, un branch con un commit dopo `main`.
    fn with_unmerged_branch() -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        let head = repo.log(&LogQuery::page(0, 1)).unwrap().remove(0);
        let extra = project_domain::Commit {
            id: Oid::from_bytes([0xee; 20]),
            parents: vec![head.id],
            summary: "Prova non unita".to_owned(),
            ..head
        };
        repo.set_changes(extra.id, Vec::new());
        repo.set_branch("esperimento", extra.id);
        repo.insert_commit(extra);
        repo
    }

    /// Manda `params` a `op` e passa la risposta al box, come fa la UI.
    fn send(b: &mut BranchBox, r: &Registry, s: &mut Session, op: Op, params: Value) -> Received {
        match r.run(op.command(), s, params) {
            Ok(value) => b.receive(op, Ok(value), None, None),
            Err(e) => b.receive(
                op,
                Err(e.to_string()),
                Some(ErrorKind::of(&e)),
                ErrorKind::detail(&e),
            ),
        }
    }

    fn done(r: Received) -> Outcome {
        match r {
            Received::Done(outcome) => *outcome,
            other => panic!("{other:?}"),
        }
    }

    /// Il commit del branch locale `name`, come lo mostra la colonna dei ref.
    fn tip(r: &Registry, s: &mut Session, name: &str) -> Oid {
        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", s, json!({})).unwrap()).unwrap();
        let full = format!("refs/heads/{name}");
        refs.iter().find(|i| i.name == full).unwrap().target
    }

    fn has_branch(outcome: &Outcome, name: &str) -> bool {
        let full = format!("refs/heads/{name}");
        outcome
            .refs
            .as_ref()
            .unwrap()
            .iter()
            .any(|r| r.name == full)
    }

    /// Il percorso completo con il repo di esempio: branch nuovo da un branch remoto, nome
    /// sbagliato corretto nel dialogo, rinomina, eliminazione con conferma, poi Annulla.
    #[test]
    fn create_rename_delete_with_the_sample_repo() {
        let (r, mut s) = open(with_unmerged_branch());
        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", &mut s, json!({})).unwrap()).unwrap();
        let remote = refs
            .iter()
            .find(|i| i.name == "refs/remotes/origin/main")
            .unwrap();
        let mut b = BranchBox::default();

        // Il nome proposto (`main`) c'è già: l'errore resta nel dialogo.
        let (start, name) = Start::from_ref(remote);
        assert_eq!(name, "main");
        b.open_create(start, name);
        let (op, params, _) = b.submit_name().unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, op, params),
            Received::Inline
        ));
        let Some(Dialog::Name(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(
            dialog.error.as_ref().unwrap().contains("esiste già"),
            "{dialog:?}"
        );
        // Un nome che git non accetta: lo dice il motore.
        b.set_name("con spazio".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, op, params),
            Received::Inline
        ));
        b.set_name("da-origin".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        let created = done(send(&mut b, &r, &mut s, op, params));
        assert!(b.dialog().is_none());
        assert!(has_branch(&created, "da-origin"));
        let new = created
            .refs
            .as_ref()
            .unwrap()
            .iter()
            .find(|i| i.name == "refs/heads/da-origin")
            .unwrap();
        assert_eq!(new.target, remote.target);
        assert!(
            created.notice.contains("«da-origin» creato"),
            "{}",
            created.notice
        );
        assert!(created.history.can_undo);

        // Rinomina.
        b.open_rename("da-origin".to_owned());
        b.set_name("prova".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        let renamed = done(send(&mut b, &r, &mut s, op, params));
        assert!(has_branch(&renamed, "prova") && !has_branch(&renamed, "da-origin"));
        assert!(
            renamed.notice.contains("rinominato in «prova»"),
            "{}",
            renamed.notice
        );

        // `prova` è su origin/main, che è in HEAD (main): si elimina senza conferma.
        let check = b
            .start_delete("prova".to_owned(), tip(&r, &mut s, "prova"))
            .unwrap();
        let deleted = done(send(&mut b, &r, &mut s, Op::Delete, check));
        assert!(!has_branch(&deleted, "prova"));
        assert!(
            deleted
                .notice
                .contains("backup in refs/project/backup/delete-"),
            "{}",
            deleted.notice
        );

        // `feature/grafo` è unito e ha un upstream: eliminato subito, con la configurazione.
        let check = b
            .start_delete("feature/grafo".to_owned(), tip(&r, &mut s, "feature/grafo"))
            .unwrap();
        let deleted = done(send(&mut b, &r, &mut s, Op::Delete, check));
        assert!(
            deleted
                .notice
                .contains("tolta anche l'impostazione dell'upstream")
                && deleted
                    .notice
                    .contains("lo ricrea con la sua configurazione"),
            "{}",
            deleted.notice
        );
        r.run("edit.undo", &mut s, json!({})).unwrap();

        // Un branch nuovo da un branch locale: nessun nome proposto.
        let grafo = refs
            .iter()
            .find(|i| i.name == "refs/heads/feature/grafo")
            .unwrap();
        let (start, name) = Start::from_ref(grafo);
        assert_eq!(name, "");
        b.open_create(start, "copia".to_owned());
        let (op, params, _) = b.submit_name().unwrap();
        assert!(has_branch(
            &done(send(&mut b, &r, &mut s, op, params)),
            "copia"
        ));

        // Un branch che non è più sul commit visto non si elimina: lo dice il motore.
        let main = tip(&r, &mut s, "main");
        let stale = b.start_delete("esperimento".to_owned(), main).unwrap();
        let refused = send(&mut b, &r, &mut s, Op::Delete, stale);
        assert!(
            matches!(&refused, Received::Failed { reload: false, .. }),
            "{refused:?}"
        );

        // `esperimento` ha un commit che main non ha: per eliminarlo serve la conferma.
        let check = b
            .start_delete("esperimento".to_owned(), tip(&r, &mut s, "esperimento"))
            .unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, Op::Delete, check),
            Received::Confirm
        ));
        let Some(Dialog::Delete(DeleteDialog {
            phase: Phase::Confirm { reason },
            ..
        })) = b.dialog()
        else {
            panic!()
        };
        assert!(reason.contains("non è unito"), "{reason}");
        let send_params = b.confirm_delete().unwrap();
        let deleted = done(send(&mut b, &r, &mut s, Op::Delete, send_params));
        assert!(!has_branch(&deleted, "esperimento"));
        assert!(deleted.notice.contains("non unit"), "{}", deleted.notice);
        assert_eq!(
            deleted.history.next_undo.is_some(),
            deleted.history.can_undo
        );

        // Annulla lo ricrea.
        r.run("edit.undo", &mut s, json!({})).unwrap();
        let refs: Vec<RefInfo> =
            serde_json::from_value(r.run("repo.refs", &mut s, json!({})).unwrap()).unwrap();
        assert!(refs.iter().any(|i| i.name == "refs/heads/esperimento"));

        // La conferma vale per il commit mostrato: se nel frattempo il branch è stato rifatto
        // altrove, `confirm: true` è rifiutato e il branch resta.
        let seen = tip(&r, &mut s, "esperimento");
        let check = b.start_delete("esperimento".to_owned(), seen).unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, Op::Delete, check),
            Received::Confirm
        ));
        r.run(
            "branch.delete",
            &mut s,
            json!({ "name": "esperimento", "confirm": true }),
        )
        .unwrap();
        r.run(
            "branch.create",
            &mut s,
            json!({ "name": "esperimento", "start": grafo.target.to_string() }),
        )
        .unwrap();
        let confirmed = b.confirm_delete().unwrap();
        let refused = send(&mut b, &r, &mut s, Op::Delete, confirmed);
        let Received::Failed { message, .. } = refused else {
            panic!("{refused:?}")
        };
        assert!(message.contains("non possibile"), "{message}");
        assert_eq!(tip(&r, &mut s, "esperimento"), grafo.target);
    }

    /// Checkout con il repo di esempio: rifiutato per le modifiche locali (con i file), poi
    /// riuscito con il worktree pulito, e Annulla torna al branch di prima.
    #[test]
    fn checkout_with_the_sample_repo() {
        let (r, mut s) = open(MemoryBackend::sample());
        let mut b = BranchBox::default();
        let params = b.start_checkout("feature/grafo".to_owned()).unwrap();
        assert!(matches!(
            send(&mut b, &r, &mut s, Op::Checkout, params),
            Received::Refused
        ));
        let Some(Dialog::Refused(refused)) = b.dialog() else {
            panic!()
        };
        assert!(!refused.files.is_empty());
        assert!(
            refused.files.iter().any(|f| f == "README.md"),
            "{refused:?}"
        );
        b.close();

        let mut clean = MemoryBackend::sample();
        clean.set_entries(Vec::new());
        let (r, mut s) = open(clean);
        let params = b.start_checkout("feature/grafo".to_owned()).unwrap();
        let out = done(send(&mut b, &r, &mut s, Op::Checkout, params));
        let status = out.status.unwrap();
        assert!(
            matches!(&status.head, project_domain::Head::Branch { name, .. } if name == "feature/grafo"),
            "{:?}",
            status.head
        );
        assert!(
            out.notice.starts_with("Passato al branch «feature/grafo»"),
            "{}",
            out.notice
        );
        assert!(out.history.can_undo);

        // Il branch già attivo: niente cambia.
        let params = b.start_checkout("feature/grafo".to_owned()).unwrap();
        let out = done(send(&mut b, &r, &mut s, Op::Checkout, params));
        assert!(
            out.notice.contains("già il branch attivo"),
            "{}",
            out.notice
        );

        // Eliminare il branch attivo è rifiutato.
        let params = b
            .start_delete("feature/grafo".to_owned(), tip(&r, &mut s, "feature/grafo"))
            .unwrap();
        let r2 = send(&mut b, &r, &mut s, Op::Delete, params);
        assert!(
            matches!(r2, Received::Failed { reload: false, .. }),
            "{r2:?}"
        );

        let err = r.run("edit.undo", &mut s, json!({})).map(|_| ());
        assert!(err.is_ok(), "{err:?}");
        let status: Status =
            serde_json::from_value(r.run("repo.status", &mut s, json!({})).unwrap()).unwrap();
        assert!(
            matches!(&status.head, project_domain::Head::Branch { name, .. } if name == "main"),
            "{:?}",
            status.head
        );
    }
}
