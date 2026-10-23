//! Pull e push (S26): `remote.pull` e `remote.push` in background, con avanzamento e
//! "Interrompi" come il fetch. Solo dati, senza egui: si prova con test unitari.
//!
//! Un'operazione di rete alla volta: fetch, pull e push sono scritture
//! ([`CommandSpec::writes`](project_engine::CommandSpec)), e la UI non ne manda una mentre
//! un'altra è in corso. I rifiuti da spiegare aprono una finestra:
//!
//! - pull in merge fermo in conflitto: non è un errore, il merge **resta in corso** come con
//!   `merge.start` (`in_progress` nel risultato). Lo stato riletto fa comparire il riquadro
//!   dell'operazione in corso (Continua/Annulla il merge) e si apre l'area di stage con i file;
//! - pull in rebase fermo in conflitto ([`ErrorKind::PullConflict`]): il motore ha già annullato
//!   il rebase, il repo è come prima; la finestra elenca i file e propone l'altra strategia;
//! - push rifiutato ([`ErrorKind::PushRejected`]): non fast-forward propone "Pull" e, come
//!   seconda scelta, "Forza (con lease)…"; lease scaduto propone "Fetch"; un hook mostra il suo
//!   output.
//!
//! Il force non parte mai da un pulsante della barra: solo da quella finestra, prima con
//! `"force": true` e senza `confirm`. Se il remoto perderebbe commit il motore risponde
//! `ConfirmRequired` con i commit, e si apre la conferma; `"confirm": true` solo dopo il clic.

use crate::ErrorKind;
use crate::discard::History;
use crate::fetch::{self, Failure};
use crate::{Action, App, Done, sync};
use project_domain::{Commit, Head, Oid, RefInfo, RepoPath, Status};
use project_engine::{PullOperation, PushRejection};
use project_i18n::tr;
use serde::Deserialize;
use serde_json::{Value, json};

/// Gli id dei comandi del Registry.
pub const PULL: &str = "remote.pull";
pub const PUSH: &str = "remote.push";

/// Pull o push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Pull,
    Push,
}

impl Op {
    pub fn command(self) -> &'static str {
        match self {
            Op::Pull => PULL,
            Op::Push => PUSH,
        }
    }
}

/// La strategia chiesta per un pull (`None` nella richiesta: quella della configurazione).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Merge,
    Rebase,
}

impl Strategy {
    fn param(self) -> &'static str {
        match self {
            Strategy::Merge => "merge",
            Strategy::Rebase => "rebase",
        }
    }

    /// L'altra strategia rispetto all'operazione che si è fermata in conflitto.
    pub fn other_than(operation: PullOperation) -> Self {
        match operation {
            PullOperation::Merge => Strategy::Rebase,
            PullOperation::Rebase => Strategy::Merge,
        }
    }

    pub fn name(self) -> &'static str {
        self.param()
    }
}

/// Cosa chiedere: pull o push, di quale branch, con quali opzioni.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub op: Op,
    /// Il branch locale (nome breve); `None` = quello di HEAD.
    pub branch: Option<String>,
    /// Solo pull: la strategia al posto della configurazione.
    pub strategy: Option<Strategy>,
    /// Solo push: `"force": true` (sempre con lease nel motore).
    pub force: bool,
    /// Solo push forzato: `"confirm": true`, dopo il clic nella conferma.
    pub confirm: bool,
    /// Solo push forzato confermato: l'oid del branch remoto locale visto nella conferma
    /// (`"expected"`), se il motore l'ha dato.
    pub expected: Option<Oid>,
}

impl Request {
    pub fn pull(branch: Option<String>, strategy: Option<Strategy>) -> Self {
        Self {
            op: Op::Pull,
            branch,
            strategy,
            force: false,
            confirm: false,
            expected: None,
        }
    }

    pub fn push(branch: Option<String>) -> Self {
        Self {
            op: Op::Push,
            branch,
            strategy: None,
            force: false,
            confirm: false,
            expected: None,
        }
    }

    /// I parametri del comando.
    pub fn params(&self) -> Value {
        let mut params = json!({});
        if let Some(branch) = &self.branch {
            params["branch"] = json!(branch);
        }
        if let Some(strategy) = self.strategy {
            params["strategy"] = json!(strategy.param());
        }
        if self.force {
            params["force"] = json!(true);
        }
        if self.confirm {
            params["confirm"] = json!(true);
        }
        if let Some(expected) = self.expected {
            params["expected"] = json!(expected.to_string());
        }
        params
    }

    /// La descrizione per la barra: `Pull di «main»`, `Push forzato di «main»`, `Pull`.
    pub fn label(&self) -> String {
        match (self.op, self.force, &self.branch) {
            (Op::Pull, _, Some(branch)) => tr!("sync-label-pull-branch", branch = branch),
            (Op::Pull, _, None) => tr!("sync-label-pull"),
            (Op::Push, false, Some(branch)) => tr!("sync-label-push-branch", branch = branch),
            (Op::Push, false, None) => tr!("sync-label-push"),
            (Op::Push, true, Some(branch)) => tr!("sync-label-force-branch", branch = branch),
            (Op::Push, true, None) => tr!("sync-label-force"),
        }
    }
}

/// Il pull o push in corso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub ticket: crate::Ticket,
    pub request: Request,
    /// Vero dopo "Interrompi", finché il comando non risponde.
    pub cancelling: bool,
}

/// Un pull fermo in conflitto e già annullato dal motore: oggi solo il rebase (il merge fermo
/// resta in corso, [`Outcome::show_changes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// Il branch del pull (`None`: quello di HEAD).
    pub branch: Option<String>,
    pub operation: PullOperation,
    pub files: Vec<String>,
    /// Il messaggio del motore (con l'upstream e cosa fare da terminale).
    pub message: String,
}

impl Conflict {
    /// La strategia da proporre per riprovare.
    pub fn other(&self) -> Strategy {
        Strategy::other_than(self.operation)
    }
}

/// Un push rifiutato: niente è cambiato, né sul remoto né in locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Il branch del push (`None`: quello di HEAD).
    pub branch: Option<String>,
    pub reason: PushRejection,
    /// Cosa è successo e cosa fare.
    pub explanation: String,
    /// Il testo di git (l'output dell'hook, la riga `[rejected]`).
    pub git: Option<String>,
}

impl Rejected {
    /// Vero se il remoto ha commit che il branch non ha: si propongono Pull e il force.
    pub fn non_fast_forward(&self) -> bool {
        self.reason == PushRejection::NonFastForward
    }

    /// Vero se il remoto si è spostato dall'ultimo fetch: si propone il fetch.
    pub fn stale(&self) -> bool {
        self.reason == PushRejection::StaleLease
    }
}

/// La conferma di un push forzato che toglierebbe commit dal remoto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmForce {
    pub branch: Option<String>,
    /// Il motivo del motore: i commit persi sul remoto e il backup.
    pub reason: String,
    /// L'oid del branch remoto locale che la conferma mostra: torna al motore con
    /// `"confirm": true` (`"expected"`), così non si conferma un remoto diverso da quello visto.
    /// `None`: il motore non l'ha dato (es. il branch remoto locale è sparito) e non si può
    /// confermare.
    pub expected: Option<Oid>,
    /// Vero se la conferma è stata mandata ma nel frattempo il branch remoto locale è cambiato:
    /// il dialogo mostra i commit nuovi e va confermato di nuovo.
    pub changed: bool,
}

impl ConfirmForce {
    /// Vero se si può confermare: c'è l'oid visto da rimandare.
    pub fn can_confirm(&self) -> bool {
        self.expected.is_some()
    }
}

/// La finestra aperta dopo un pull o un push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Window {
    /// Errore da spiegare (credenziali, rete, rifiuto con un messaggio lungo, errore di git).
    Failure(Failure),
    Conflict(Conflict),
    Rejected(Rejected),
    /// La conferma del force: è un dialogo modale.
    ConfirmForce(ConfirmForce),
}

/// Pull e push: quello in corso e la finestra aperta.
#[derive(Debug, Default)]
pub struct SyncBox {
    running: Option<Running>,
    window: Option<Window>,
}

/// Cosa è arrivato da `remote.pull` o `remote.push`.
#[derive(Debug)]
pub enum Received {
    Done(Box<Outcome>),
    /// Il motore chiede la conferma del force: la finestra è aperta.
    Confirm,
    /// Interrotto: con `reload` si rileggono ref, stato e log (il fetch del pull o il push
    /// potrebbero aver aggiornato qualcosa).
    Cancelled {
        notice: String,
        reload: bool,
    },
    /// Non riuscito o rifiutato: il messaggio per la barra di stato (i dettagli, se servono,
    /// nella finestra); con `reload` si rileggono ref, stato e log.
    Failed {
        message: String,
        reload: bool,
    },
}

/// Il risultato di un pull o push riuscito, per la vista.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    pub refs: Option<Vec<RefInfo>>,
    pub status: Option<Status>,
    /// Il diario dopo il pull (Annulla lo propone); `None` per il push, che non ci entra.
    pub history: Option<History>,
    /// Vero se il branch attivo si è spostato: dopo il log si seleziona il nuovo HEAD.
    pub moved_head: bool,
    /// Il merge del pull è fermo con file in conflitto e resta in corso: si apre l'area di
    /// stage con i file da risolvere (il riquadro dell'operazione compare dallo stato riletto).
    pub show_changes: bool,
    /// Un problema dopo l'operazione riuscita (ref o stato non riletti).
    pub warning: Option<String>,
}

impl SyncBox {
    pub fn running(&self) -> Option<&Running> {
        self.running.as_ref()
    }

    pub fn window(&self) -> Option<&Window> {
        self.window.as_ref()
    }

    /// Vero se è aperta la conferma del force: finché c'è niente scritture.
    pub fn modal_open(&self) -> bool {
        matches!(self.window, Some(Window::ConfirmForce(_)))
    }

    /// Chiude la finestra.
    pub fn dismiss(&mut self) {
        self.window = None;
    }

    /// La richiesta, se non c'è già un pull o push in corso, con il branch fissato: senza
    /// `branch` (pulsanti della barra) si usa quello di HEAD mostrato adesso (`head`). Così le
    /// finestre che ne nascono (rifiuto, conferma del force) mandano sempre il branch per nome,
    /// anche se intanto HEAD cambia da terminale. Con HEAD staccata resta `None` e il motore
    /// rifiuta.
    pub fn request(&self, mut request: Request, head: &Head) -> Option<Request> {
        if self.running.is_some() {
            return None;
        }
        if request.branch.is_none()
            && let Head::Branch { name, .. } = head
        {
            request.branch = Some(name.clone());
        }
        Some(request)
    }

    /// "Forza (con lease)…" dalla finestra del push rifiutato perché non fast-forward: il push
    /// con `"force": true` e senza `confirm`. Da nessun altro posto.
    pub fn force(&self) -> Option<Request> {
        let Some(Window::Rejected(rejected)) = &self.window else {
            return None;
        };
        if !rejected.non_fast_forward() || self.running.is_some() || rejected.branch.is_none() {
            return None;
        }
        Some(Request {
            force: true,
            ..Request::push(rejected.branch.clone())
        })
    }

    /// Il clic su "Forza il push" nella conferma: `"force": true, "confirm": true`.
    pub fn confirm_force(&self) -> Option<Request> {
        let Some(Window::ConfirmForce(confirm)) = &self.window else {
            return None;
        };
        if self.running.is_some() || !confirm.can_confirm() || confirm.branch.is_none() {
            return None;
        }
        Some(Request {
            force: true,
            confirm: true,
            expected: confirm.expected,
            ..Request::push(confirm.branch.clone())
        })
    }

    /// La richiesta è partita con `ticket`: la finestra si chiude.
    pub fn started(&mut self, ticket: crate::Ticket, request: Request) {
        self.window = None;
        self.running = Some(Running {
            ticket,
            request,
            cancelling: false,
        });
    }

    /// "Interrompi": il ticket da annullare, una volta sola.
    pub fn cancel(&mut self) -> Option<crate::Ticket> {
        let running = self.running.as_mut().filter(|r| !r.cancelling)?;
        running.cancelling = true;
        Some(running.ticket)
    }

    /// La risposta del comando per `ticket`.
    pub fn receive(
        &mut self,
        ticket: crate::Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
        expected: Option<String>,
    ) -> Received {
        let request = match self.running.take() {
            Some(running) if running.ticket == ticket => running.request,
            other => {
                self.running = other;
                return Received::Failed {
                    message: tr!("sync-unexpected-response"),
                    reload: true,
                };
            }
        };
        let label = request.label();
        let command = request.op.command();
        let error = match result {
            Ok(value) => {
                let outcome = match request.op {
                    Op::Pull => {
                        serde_json::from_value::<PullResult>(value).map(PullResult::outcome)
                    }
                    Op::Push => {
                        serde_json::from_value::<PushResult>(value).map(PushResult::outcome)
                    }
                };
                return match outcome {
                    Ok(outcome) => Received::Done(Box::new(outcome)),
                    Err(e) => Received::Failed {
                        message: tr!(
                            "sync-unexpected-result",
                            command = command,
                            error = e.to_string()
                        ),
                        reload: true,
                    },
                };
            }
            Err(error) => error,
        };
        let branch = request.branch.clone();
        match kind {
            Some(ErrorKind::Cancelled) => Received::Cancelled {
                notice: match request.op {
                    Op::Pull => tr!("sync-pull-cancelled", label = &label),
                    Op::Push => tr!("sync-push-cancelled", label = &label),
                },
                reload: true,
            },
            Some(ErrorKind::Auth) => {
                self.window = Some(Window::Failure(fetch::auth_failure(
                    &fetch::who(None),
                    detail,
                )));
                Received::Failed {
                    message: tr!("sync-failed-auth", label = &label),
                    reload: false,
                }
            }
            Some(ErrorKind::RemoteUnreachable) => {
                self.window = Some(Window::Failure(fetch::unreachable_failure(
                    &fetch::who(None),
                    detail,
                )));
                Received::Failed {
                    message: tr!("sync-failed-unreachable", label = &label),
                    reload: false,
                }
            }
            Some(ErrorKind::PullConflict(operation)) => {
                let files = detail
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .collect();
                self.window = Some(Window::Conflict(Conflict {
                    branch,
                    operation,
                    files,
                    message: error,
                }));
                // Il fetch è fatto: il branch remoto si rilegge.
                Received::Failed {
                    message: tr!(
                        "sync-pull-conflict",
                        label = &label,
                        operation = operation.to_string()
                    ),
                    reload: true,
                }
            }
            Some(ErrorKind::PushRejected(reason)) => {
                self.window = Some(Window::Rejected(Rejected {
                    branch,
                    reason,
                    explanation: rejection_text(reason),
                    git: detail,
                }));
                Received::Failed {
                    message: tr!(
                        "sync-rejected",
                        label = &label,
                        reason = rejection_short(reason)
                    ),
                    reload: false,
                }
            }
            // Il force chiede la conferma; se era già la conferma, il branch remoto locale è
            // cambiato da quando è stato visto: il dialogo si aggiorna e si conferma di nuovo,
            // niente parte da solo.
            Some(ErrorKind::ConfirmRequired) if request.force => {
                self.window = Some(Window::ConfirmForce(ConfirmForce {
                    branch,
                    reason: detail.unwrap_or(error),
                    expected: expected.and_then(|e| e.parse().ok()),
                    changed: request.confirm,
                }));
                Received::Confirm
            }
            // Rifiutato prima di cominciare (HEAD staccata, nessun upstream, modifiche locali,
            // branch divergente con solo fast-forward...): il motivo è lungo, va in finestra.
            // Il pull può aver già fatto il fetch: si rilegge.
            Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => {
                self.window = Some(Window::Failure(Failure {
                    title: tr!("sync-not-possible-title", label = &label),
                    explanation: error.clone(),
                    git: None,
                }));
                Received::Failed {
                    message: tr!("sync-not-possible", label = &label, error = error),
                    reload: request.op == Op::Pull,
                }
            }
            _ => {
                self.window = Some(Window::Failure(Failure {
                    title: tr!("sync-failed-title", label = &label),
                    explanation: error.clone(),
                    git: detail,
                }));
                Received::Failed {
                    message: tr!("sync-failed", label = &label, error = error),
                    reload: true,
                }
            }
        }
    }
}

/// Cosa spiegare nella finestra di un push rifiutato.
fn rejection_text(reason: PushRejection) -> String {
    match reason {
        PushRejection::NonFastForward => tr!("sync-rejection-non-fast-forward"),
        PushRejection::StaleLease => tr!("sync-rejection-stale-lease"),
        PushRejection::RemoteRejected => tr!("sync-rejection-remote"),
        PushRejection::Hook => tr!("sync-rejection-hook"),
        PushRejection::Other => tr!("sync-rejection-other"),
    }
}

/// Il motivo del rifiuto in breve, per la barra di stato.
fn rejection_short(reason: PushRejection) -> String {
    match reason {
        PushRejection::NonFastForward => tr!("sync-rejection-short-non-fast-forward"),
        PushRejection::StaleLease => tr!("sync-rejection-short-stale-lease"),
        PushRejection::RemoteRejected => tr!("sync-rejection-short-remote"),
        PushRejection::Hook => tr!("sync-rejection-short-hook"),
        PushRejection::Other => tr!("sync-rejection-short-other"),
    }
}

/// `refs/remotes/origin/main` → `origin/main`.
fn remote_short(name: &str) -> &str {
    name.strip_prefix("refs/remotes/").unwrap_or(name)
}

/// La parte di `fetch` nel risultato del pull.
#[derive(Debug, Clone, Deserialize)]
struct PullFetch {
    #[serde(default)]
    messages: Vec<String>,
}

/// Risultato di `remote.pull`.
#[derive(Debug, Clone, Deserialize)]
struct PullResult {
    branch: String,
    upstream: String,
    strategy: String,
    strategy_source: String,
    before: Oid,
    after: Oid,
    ahead: u64,
    behind: u64,
    /// Il merge si è fermato (conflitti, hook, firma) e resta in corso.
    #[serde(default)]
    in_progress: bool,
    #[serde(default)]
    conflicts: Vec<RepoPath>,
    backup: Option<String>,
    fetch: Option<PullFetch>,
    #[serde(default)]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

impl PullResult {
    fn outcome(self) -> Outcome {
        for line in self.fetch.iter().flat_map(|f| &f.messages) {
            log::info!("{PULL}: {line}");
        }
        let upstream = remote_short(&self.upstream);
        let what = tr!("sync-label-pull-branch", branch = &self.branch);
        let range = format!("{} → {}", self.before.short(), self.after.short());
        let mut notice = match self.strategy.as_str() {
            _ if self.in_progress => match self.conflicts.len() {
                0 => tr!("sync-pull-stopped", what = what, upstream = upstream),
                n => tr!(
                    "sync-pull-stopped-conflicts",
                    what = what,
                    upstream = upstream,
                    n = n
                ),
            },
            "up_to_date" if self.ahead > 0 => tr!(
                "sync-pull-up-to-date-ahead",
                what = what,
                upstream = upstream,
                n = self.ahead
            ),
            "up_to_date" => tr!("sync-pull-up-to-date", what = what, upstream = upstream),
            "fast_forward" => tr!(
                "sync-pull-fast-forward",
                what = what,
                upstream = upstream,
                range = range,
                n = self.behind
            ),
            "merge" => tr!(
                "sync-pull-merge",
                what = what,
                upstream = upstream,
                range = range
            ),
            "rebase" => tr!(
                "sync-pull-rebase",
                what = what,
                upstream = upstream,
                range = range,
                n = self.ahead
            ),
            other => tr!("sync-pull-other", what = what, kind = other, range = range),
        };
        if self.strategy == "merge" && self.strategy_source == "default" {
            notice.push_str(&tr!("sync-pull-merge-default"));
        }
        match &self.backup {
            Some(backup) if self.in_progress => {
                notice.push_str(&tr!("sync-pull-backup-in-progress", backup = backup));
            }
            Some(backup) => notice.push_str(&tr!("sync-pull-backup", backup = backup)),
            None => {}
        }
        for warning in &self.warnings {
            notice.push_str(&tr!("sync-warning", warning = warning));
        }
        let moved_head = self.after != self.before
            && self.status.as_ref().is_some_and(
                |s| matches!(&s.head, Head::Branch { name, .. } if *name == self.branch),
            );
        let warning = match (self.status_error, self.refs_error) {
            (Some(e), _) => Some(tr!("sync-pull-status-not-reread", error = e)),
            (None, Some(e)) => Some(tr!("sync-pull-refs-not-reread", error = e)),
            (None, None) => None,
        };
        Outcome {
            notice,
            refs: self.refs,
            status: self.status,
            history: Some(self.history),
            moved_head,
            show_changes: self.in_progress && !self.conflicts.is_empty(),
            warning,
        }
    }
}

/// Risultato di `remote.push`.
#[derive(Debug, Clone, Deserialize)]
struct PushResult {
    remote: String,
    /// Da dove viene il remoto: `param`, `push_remote`, `push_default`, `upstream`, `origin`,
    /// `only`.
    #[serde(default)]
    remote_source: Option<String>,
    branch: String,
    remote_branch: String,
    kind: String,
    #[serde(default)]
    summary: String,
    set_upstream: bool,
    #[serde(default)]
    lost: Vec<Commit>,
    backup: Option<String>,
    #[serde(default)]
    messages: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
}

impl PushResult {
    fn outcome(self) -> Outcome {
        for line in &self.messages {
            log::info!("{PUSH}: {line}");
        }
        let target = format!("{}/{}", self.remote, self.remote_branch);
        let summary = if self.summary.is_empty() {
            String::new()
        } else {
            format!(" ({})", self.summary)
        };
        let branch = &self.branch;
        let mut notice = match self.kind.as_str() {
            "new" => tr!(
                "sync-push-new",
                branch = branch,
                target = &target,
                summary = &summary
            ),
            "fast_forward" => tr!(
                "sync-push-fast-forward",
                branch = branch,
                target = &target,
                summary = &summary
            ),
            "forced" => tr!(
                "sync-push-forced",
                branch = branch,
                target = &target,
                summary = &summary,
                n = self.lost.len()
            ),
            "up_to_date" => tr!("sync-push-up-to-date", branch = branch, target = &target),
            other => tr!(
                "sync-push-other",
                branch = branch,
                target = &target,
                kind = other
            ),
        };
        // Un remoto scelto dalla configurazione di push non è quello che ci si aspetta: lo si
        // dice.
        match self.remote_source.as_deref() {
            Some("push_remote") => notice.push_str(&tr!(
                "sync-push-to-push-remote",
                remote = &self.remote,
                branch = &self.branch
            )),
            Some("push_default") => {
                notice.push_str(&tr!("sync-push-to-push-default", remote = &self.remote));
            }
            _ => {}
        }
        if let Some(backup) = &self.backup {
            notice.push_str(&tr!("sync-push-backup", backup = backup));
        }
        if self.set_upstream {
            notice.push_str(&tr!("sync-push-set-upstream", target = &target));
        }
        for warning in &self.warnings {
            notice.push_str(&tr!("sync-warning", warning = warning));
        }
        let warning = match (self.status_error, self.refs_error) {
            (Some(e), _) => Some(tr!("sync-push-status-not-reread", error = e)),
            (None, Some(e)) => Some(tr!("sync-push-refs-not-reread", error = e)),
            (None, None) => None,
        };
        Outcome {
            notice,
            refs: self.refs,
            status: self.status,
            history: None,
            moved_head: false,
            show_changes: false,
            warning,
        }
    }
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_sync(&mut self, action: Action) {
        match action {
            Action::Pull(branch, strategy) => {
                self.send_sync(sync::Request::pull(branch, strategy), false);
            }
            Action::Push(branch) => self.send_sync(sync::Request::push(branch), false),
            Action::ForcePush => {
                if let Some(request) = self.repo.as_ref().and_then(|r| r.sync.force()) {
                    self.send_sync(request, false);
                }
            }
            Action::ConfirmForcePush => {
                if let Some(request) = self.repo.as_ref().and_then(|r| r.sync.confirm_force()) {
                    self.send_sync(request, true);
                }
            }
            Action::CancelSync => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.sync.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::DismissSync => {
                if let Some(repo) = &mut self.repo {
                    repo.sync.dismiss();
                }
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Manda un pull o un push, se non c'è un'altra scrittura in corso (fetch compreso: una
    /// operazione di rete alla volta) né un dialogo aperto; `confirmed`: è il clic nella
    /// conferma del force, che è lei il dialogo aperto.
    pub(crate) fn send_sync(&mut self, request: sync::Request, confirmed: bool) {
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        if self.executor.writing().is_some() || (repo.modal_open() && !confirmed) {
            return;
        }
        let Some(request) = repo.sync.request(request, &repo.head) else {
            return;
        };
        repo.write_error = None;
        repo.write_label = Some(request.label());
        let ticket = self.executor.submit(request.op.command(), request.params());
        self.pending
            .insert(ticket, crate::Request::Sync(generation));
        if let Some(repo) = &mut self.repo {
            repo.sync.started(ticket, request);
        }
    }

    /// Risposta di `remote.pull`/`remote.push`: ref, stato e diario dalla risposta, poi il log
    /// riletto (tutti i ref: i commit arrivati, il branch spostato, il branch remoto
    /// aggiornato); dopo un pull del branch attivo si seleziona il nuovo HEAD. Un rifiuto apre
    /// la sua finestra.
    pub(crate) fn receive_sync(&mut self, generation: u64, done: Done) {
        // Un push può creare un branch remoto, un pull ne scarica.
        if self.current(generation).is_some() {
            self.request_remotes(generation);
        }
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received = repo.sync.receive(
            done.ticket,
            done.result,
            done.error_kind,
            done.error_detail,
            done.confirm_expected,
        );
        match received {
            sync::Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                if let Some(history) = outcome.history {
                    repo.history = history;
                }
                let refs_read = outcome.refs.is_some();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                let status_read = outcome.status.is_some();
                if let Some(status) = outcome.status {
                    repo.set_status(status);
                }
                if outcome.show_changes {
                    // Merge del pull fermo in conflitto: l'area di stage con i file da risolvere.
                    repo.stash.select(None);
                    repo.select_worktree(true);
                } else if outcome.moved_head && !repo.worktree_selected {
                    repo.select_after_load = repo.head.target();
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                self.follow_diff();
                if !refs_read {
                    self.submit("repo.refs", json!({}), crate::Request::Refs(generation));
                }
                if !status_read {
                    self.submit("repo.status", json!({}), crate::Request::Status(generation));
                }
                if let Some(repo) = self.current(generation) {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            sync::Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            sync::Received::Cancelled { notice, reload } => {
                self.notice = notice;
                self.error.clear();
                if reload {
                    self.load();
                }
            }
            sync::Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Ticket;
    use project_domain::{Change, ChangedFile, RepoPath, Signature};
    use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

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

    /// Manda `request` come farebbe l'app e consegna la risposta alla `SyncBox`.
    fn run(r: &Registry, s: &mut Session, sync: &mut SyncBox, request: Request) -> Received {
        let status = r.run("repo.status", s, json!({})).unwrap();
        let head: Head = serde_json::from_value(status["head"].clone()).unwrap();
        let request = sync.request(request, &head).expect("niente in corso");
        let ticket = Ticket::for_test(1);
        let params = request.params();
        let command = request.op.command();
        sync.started(ticket, request);
        let result = r.run(command, s, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        let expected = result.as_ref().err().and_then(ErrorKind::expected);
        sync.receive(
            ticket,
            result.map_err(|e| e.to_string()),
            kind,
            detail,
            expected,
        )
    }

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    fn main_head() -> Head {
        Head::Branch {
            name: "main".into(),
            target: Some(oid(12)),
        }
    }

    #[test]
    fn the_branch_is_pinned_when_the_request_starts() {
        let sync = SyncBox::default();
        let pinned = sync.request(Request::push(None), &main_head()).unwrap();
        assert_eq!(pinned.params(), json!({ "branch": "main" }));
        // Un branch indicato resta quello.
        let other = sync
            .request(Request::pull(Some("x".into()), None), &main_head())
            .unwrap();
        assert_eq!(other.branch.as_deref(), Some("x"));
        // HEAD staccata: niente da fissare, il motore rifiuta.
        let detached = sync
            .request(Request::push(None), &Head::Detached(oid(12)))
            .unwrap();
        assert_eq!(detached.branch, None);
    }

    #[test]
    fn force_and_pull_from_the_windows_keep_the_branch_after_a_checkout() {
        let (r, mut s) = open(diverged(None));
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        let mut sync = SyncBox::default();
        run(&r, &mut s, &mut sync, Request::push(None));
        let Some(Window::Rejected(rejected)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(rejected.branch.as_deref(), Some("main"));
        // Checkout da terminale mentre la finestra è aperta.
        r.run(
            "branch.checkout",
            &mut s,
            json!({ "name": "feature/grafo" }),
        )
        .unwrap();
        let pull = Request::pull(rejected.branch.clone(), None);
        assert_eq!(pull.params(), json!({ "branch": "main" }));
        let force = sync.force().unwrap();
        assert_eq!(force.params(), json!({ "branch": "main", "force": true }));
        let received = run(&r, &mut s, &mut sync, force);
        assert!(matches!(received, Received::Confirm), "{received:?}");
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(confirm.branch.as_deref(), Some("main"));
        let confirmed = sync.confirm_force().unwrap();
        assert_eq!(confirmed.branch.as_deref(), Some("main"));
        let received = run(&r, &mut s, &mut sync, confirmed);
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome.notice.starts_with("Push forzato di «main»"),
            "{}",
            outcome.notice
        );
        let refs = outcome.refs.unwrap();
        let grafo = refs
            .iter()
            .find(|r| r.name == "refs/remotes/origin/feature/grafo")
            .unwrap();
        assert_ne!(grafo.target, oid(12), "feature/grafo non è stato forzato");
    }

    fn who() -> Signature {
        Signature {
            name: "Carla".into(),
            email: "carla@example.com".into(),
            time: 1_700_000_000 + 20 * 3_600,
            offset_min: 0,
        }
    }

    /// L'esempio senza modifiche locali, con `origin/main` su un commit (20) che `main` non
    /// ha; se c'è, 20 crea `file` con un contenuto diverso da quello locale.
    fn diverged(file: Option<&str>) -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        repo.set_entries(Vec::new());
        repo.insert_remote_commit(
            "origin",
            Commit {
                id: oid(20),
                parents: vec![oid(11)],
                author: who(),
                committer: who(),
                summary: "Dal remoto".into(),
                body: String::new(),
            },
        );
        if let Some(file) = file {
            repo.set_blob(oid(5_000), "dal remoto\n");
            repo.set_changes(
                oid(20),
                vec![ChangedFile {
                    path: RepoPath::from(file),
                    change: Change::Added,
                    original: None,
                    score: None,
                    old_mode: 0,
                    new_mode: 0o100644,
                    old_oid: Oid::from_bytes([0; Oid::LEN]),
                    new_oid: oid(5_000),
                }],
            );
        }
        repo.set_branch_on_remote("origin", "main", oid(20));
        repo
    }

    #[test]
    fn params_and_labels() {
        assert_eq!(Request::pull(None, None).params(), json!({}));
        assert_eq!(
            Request::pull(Some("x".into()), Some(Strategy::Rebase)).params(),
            json!({ "branch": "x", "strategy": "rebase" })
        );
        assert_eq!(Request::push(None).params(), json!({}));
        assert_eq!(Request::push(None).label(), "Push");
        assert_eq!(
            Request::pull(Some("feature/grafo".into()), None).label(),
            "Pull di «feature/grafo»"
        );
    }

    #[test]
    fn one_at_a_time_and_cancel_once() {
        let mut sync = SyncBox::default();
        let ticket = Ticket::for_test(3);
        sync.started(ticket, Request::push(None));
        assert_eq!(sync.request(Request::pull(None, None), &main_head()), None);
        assert_eq!(sync.cancel(), Some(ticket));
        assert_eq!(sync.cancel(), None);
        let r = sync.receive(
            ticket,
            Err("annullato".into()),
            Some(ErrorKind::Cancelled),
            None,
            None,
        );
        let Received::Cancelled { notice, reload } = r else {
            panic!("{r:?}")
        };
        assert!(reload);
        assert!(notice.starts_with("Push interrotto"), "{notice}");
        assert!(notice.contains("Fetch"), "{notice}");
        assert!(sync.running().is_none());
    }

    #[test]
    fn pull_of_feature_grafo_is_a_fast_forward() {
        let (r, mut s) = open(MemoryBackend::sample());
        let mut sync = SyncBox::default();
        let received = run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(Some("feature/grafo".into()), None),
        );
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Pull di «feature/grafo»: fast-forward a origin/feature/grafo"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("1 commit nuovo"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("refs/project/backup/pull-1"),
            "{}",
            outcome.notice
        );
        // Non è il branch attivo: HEAD non si sposta.
        assert!(!outcome.moved_head);
        let refs = outcome.refs.unwrap();
        let grafo = refs
            .iter()
            .find(|r| r.name == "refs/heads/feature/grafo")
            .unwrap();
        assert_eq!(grafo.target, oid(13));
        assert_eq!(grafo.upstream.as_ref().unwrap().behind, Some(0));
        // Annulla lo propone.
        let history = outcome.history.unwrap();
        assert!(history.can_undo);
        assert!(
            history
                .next_undo
                .unwrap()
                .starts_with("Pull di `feature/grafo`")
        );
        assert!(sync.window().is_none());

        // Di nuovo: già aggiornato.
        let received = run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(Some("feature/grafo".into()), None),
        );
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert_eq!(
            outcome.notice,
            "Pull di «feature/grafo»: già aggiornato con origin/feature/grafo"
        );
    }

    #[test]
    fn pull_merge_by_default_is_explained() {
        let (r, mut s) = open(diverged(Some("nuovo.txt")));
        let mut sync = SyncBox::default();
        let received = run(&r, &mut s, &mut sync, Request::pull(None, None));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Pull di «main»: merge di origin/main"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome
                .notice
                .contains("merge perché pull.rebase non è configurato"),
            "{}",
            outcome.notice
        );
        assert!(outcome.moved_head);
        assert!(outcome.history.unwrap().can_undo);
    }

    #[test]
    fn pull_merge_conflict_stays_in_progress_without_a_window() {
        let (r, mut s) = open(diverged(Some("src/bin/app.rs")));
        let mut sync = SyncBox::default();
        let received = run(&r, &mut s, &mut sync, Request::pull(None, None));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        // Non è un errore: niente finestra, l'area di stage con i file e lo stato riletto (con
        // l'operazione in corso, da cui il riquadro Continua/Annulla).
        assert_eq!(sync.window(), None);
        assert!(outcome.show_changes);
        assert!(!outcome.moved_head);
        assert!(
            outcome
                .notice
                .starts_with("Pull di «main»: merge di origin/main fermo: 1 file in conflitto"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("Continua il merge"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("refs/project/backup/pull-1"),
            "{}",
            outcome.notice
        );
        assert_eq!(
            outcome.status.unwrap().operation,
            Some(project_domain::Operation::Merge)
        );
        assert!(!outcome.history.unwrap().can_undo);
    }

    #[test]
    fn pull_rebase_conflict_opens_the_window_with_the_files() {
        let (r, mut s) = open(diverged(Some("src/bin/app.rs")));
        let mut sync = SyncBox::default();
        let received = run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(None, Some(Strategy::Rebase)),
        );
        let Received::Failed { message, reload } = received else {
            panic!("{received:?}")
        };
        assert!(reload, "il fetch è fatto");
        assert!(message.contains("annullato"), "{message}");
        let Some(Window::Conflict(conflict)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(conflict.operation, PullOperation::Rebase);
        assert_eq!(conflict.files, ["src/bin/app.rs"]);
        assert_eq!(conflict.other(), Strategy::Merge);
        let refs = r.run("repo.refs", &mut s, json!({})).unwrap();
        let main = refs
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == "refs/heads/main")
            .unwrap();
        assert_eq!(main["target"], oid(12).to_string(), "repo com'era");
        // L'altra strategia si chiede dalla finestra: il merge resta in corso.
        let retry = Request::pull(conflict.branch.clone(), Some(conflict.other()));
        let received = run(&r, &mut s, &mut sync, retry);
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(outcome.show_changes);
        assert_eq!(sync.window(), None);
    }

    #[test]
    fn push_of_main_is_a_fast_forward() {
        let (r, mut s) = open(MemoryBackend::sample());
        let mut sync = SyncBox::default();
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Push di «main» su origin/main ("),
            "{}",
            outcome.notice
        );
        assert!(outcome.history.is_none());
        assert!(!outcome.notice.contains("pushDefault"));
        let refs = outcome.refs.unwrap();
        let origin = refs
            .iter()
            .find(|r| r.name == "refs/remotes/origin/main")
            .unwrap();
        assert_eq!(origin.target, oid(12));

        // Il primo push di un branch nuovo imposta l'upstream.
        r.run("branch.create", &mut s, json!({ "name": "nuovo" }))
            .unwrap();
        let received = run(&r, &mut s, &mut sync, Request::push(Some("nuovo".into())));
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .ends_with("; upstream impostato su origin/nuovo"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn push_rejected_offers_pull_then_force_asks_to_confirm() {
        let (r, mut s) = open(diverged(None));
        let mut sync = SyncBox::default();
        assert_eq!(sync.force(), None, "il force non parte senza la finestra");
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        let Received::Failed { message, reload } = received else {
            panic!("{received:?}")
        };
        assert!(!reload);
        assert!(
            message.contains("il remoto ha commit che non hai: fai Pull"),
            "{message}"
        );
        let Some(Window::Rejected(rejected)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(rejected.non_fast_forward());
        assert!(
            rejected
                .explanation
                .starts_with("Il remoto ha commit che non hai: fai Pull")
        );
        assert!(rejected.git.as_deref().unwrap().contains("[rejected]"));

        // "Forza (con lease)…": force senza confirm. Il branch remoto locale è vecchio (il
        // remoto ha 20, qui non è ancora arrivato): il lease lo ferma.
        let force = sync.force().unwrap();
        assert_eq!(force.params(), json!({ "branch": "main", "force": true }));
        let received = run(&r, &mut s, &mut sync, force);
        assert!(matches!(received, Received::Failed { .. }), "{received:?}");
        let Some(Window::Rejected(stale)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(stale.stale());
        assert!(stale.explanation.contains("Fai Fetch"));
        assert_eq!(sync.force(), None, "dopo il lease scaduto niente force");

        // Dopo il fetch il force toglierebbe 20: il motore chiede la conferma.
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        let received = run(&r, &mut s, &mut sync, Request::push(None));
        assert!(matches!(received, Received::Failed { .. }));
        let force = sync.force().unwrap();
        let received = run(&r, &mut s, &mut sync, force);
        assert!(matches!(received, Received::Confirm), "{received:?}");
        assert!(sync.modal_open());
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(
            confirm.reason.contains("«Dal remoto»"),
            "{}",
            confirm.reason
        );
        assert!(confirm.reason.contains("refs/project/backup/push-"));
        // "Non forzare" chiude senza mandare niente.
        let pending = sync.window().cloned();
        sync.dismiss();
        assert_eq!(sync.confirm_force(), None);
        sync.window = pending;

        let confirmed = sync.confirm_force().unwrap();
        assert_eq!(
            confirmed.params(),
            json!({
                "branch": "main", "force": true, "confirm": true,
                "expected": oid(20).to_string()
            })
        );
        let received = run(&r, &mut s, &mut sync, confirmed);
        let Received::Done(outcome) = received else {
            panic!("{received:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Push forzato di «main» su origin/main"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("1 commit tolto dal remoto"),
            "{}",
            outcome.notice
        );
        assert!(outcome.notice.contains("refs/project/backup/push-1"));
        assert!(!sync.modal_open());
    }

    #[test]
    fn push_remote_from_the_push_config_is_mentioned() {
        let value = json!({
            "remote": "fork", "remote_source": "push_default", "branch": "main",
            "remote_branch": "main", "kind": "new", "summary": "[new branch]",
            "before": null, "after": oid(12).to_string(), "set_upstream": false,
            "forced": false, "lost": [], "backup": null, "updated": [], "messages": [],
            "warnings": [], "status": null, "refs": null
        });
        let outcome = serde_json::from_value::<PushResult>(value)
            .unwrap()
            .outcome();
        assert!(
            outcome.notice.contains("su «fork» (remote.pushDefault)"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn push_rejected_by_a_hook_shows_its_output() {
        let mut repo = MemoryBackend::sample();
        repo.fail_pre_push(Some("pre-push: test falliti"));
        let (r, mut s) = open(repo);
        let mut sync = SyncBox::default();
        run(&r, &mut s, &mut sync, Request::push(None));
        let Some(Window::Rejected(rejected)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(rejected.reason, PushRejection::Hook);
        assert_eq!(rejected.git.as_deref(), Some("pre-push: test falliti"));
        assert_eq!(sync.force(), None);
    }

    #[test]
    fn a_confirm_without_force_is_not_a_force_dialog() {
        let mut sync = SyncBox::default();
        let ticket = Ticket::for_test(0);
        sync.started(ticket, Request::push(None));
        let r = sync.receive(
            ticket,
            Err("serve conferma".into()),
            Some(ErrorKind::ConfirmRequired),
            Some("motivo".into()),
            None,
        );
        assert!(matches!(r, Received::Failed { .. }), "{r:?}");
        assert!(!sync.modal_open());
    }

    #[test]
    fn a_force_confirm_without_expected_cannot_be_confirmed() {
        let mut sync = SyncBox::default();
        let ticket = Ticket::for_test(0);
        sync.started(
            ticket,
            Request {
                force: true,
                ..Request::push(None)
            },
        );
        let r = sync.receive(
            ticket,
            Err("serve conferma".into()),
            Some(ErrorKind::ConfirmRequired),
            Some("il branch remoto locale non c'è più".into()),
            None,
        );
        assert!(matches!(r, Received::Confirm), "{r:?}");
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(!confirm.can_confirm());
        assert_eq!(sync.confirm_force(), None);
    }

    #[test]
    fn a_remote_that_moved_after_the_dialog_asks_again() {
        let (r, mut s) = open(diverged(None));
        r.run("remote.fetch", &mut s, json!({})).unwrap();
        // La conferma vista quando `origin/main` era ancora su 11; ora è su 20.
        let mut sync = SyncBox {
            running: None,
            window: Some(Window::ConfirmForce(ConfirmForce {
                branch: Some("main".into()),
                reason: "vecchio motivo".into(),
                expected: Some(oid(11)),
                changed: false,
            })),
        };
        let confirmed = sync.confirm_force().unwrap();
        assert_eq!(
            confirmed.params(),
            json!({
                "branch": "main", "force": true, "confirm": true,
                "expected": oid(11).to_string()
            })
        );
        let received = run(&r, &mut s, &mut sync, confirmed);
        // Niente pubblicato: il dialogo si aggiorna con l'oid nuovo e aspetta un altro clic.
        assert!(matches!(received, Received::Confirm), "{received:?}");
        assert!(sync.running().is_none());
        let Some(Window::ConfirmForce(confirm)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert!(confirm.changed);
        assert_eq!(confirm.expected, Some(oid(20)));
        assert_ne!(confirm.reason, "vecchio motivo");
        assert_eq!(
            r.run("repo.refs", &mut s, json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .find(|x| x["name"] == "refs/remotes/origin/main")
                .unwrap()["target"],
            oid(20).to_string()
        );
        let again = sync.confirm_force().unwrap();
        assert_eq!(again.expected, Some(oid(20)));
        let received = run(&r, &mut s, &mut sync, again);
        assert!(matches!(received, Received::Done(_)), "{received:?}");
    }

    #[test]
    fn pull_without_upstream_is_explained_in_a_window() {
        let (r, mut s) = open(MemoryBackend::sample());
        r.run("branch.create", &mut s, json!({ "name": "solo" }))
            .unwrap();
        let mut sync = SyncBox::default();
        run(
            &r,
            &mut s,
            &mut sync,
            Request::pull(Some("solo".into()), None),
        );
        let Some(Window::Failure(failure)) = sync.window() else {
            panic!("{:?}", sync.window())
        };
        assert_eq!(failure.title, "Pull di «solo» non possibile");
        assert!(failure.explanation.contains("--set-upstream-to"));
    }
}
