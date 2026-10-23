//! Merge e operazione in corso (S29): `merge.start` dai menu di branch e commit, il riquadro
//! dell'operazione in corso (`repo.operation`) con `merge.continue` e `merge.abort`. Solo dati,
//! senza egui: si prova con test unitari. Contratto dei comandi in `impl-docs/git/merge.md`.
//!
//! - **Riquadro**: quando lo stato del repo (`repo.status`) dice che c'è un'operazione in corso
//!   (merge, rebase, cherry-pick, revert, bisect, `git am`) si legge `repo.operation`: cosa sta
//!   entrando, i file in conflitto, il messaggio preparato da git e i comandi che concludono o
//!   annullano l'operazione da qui (oggi solo per il merge). Si rilegge a ogni stato nuovo
//!   (dopo ogni comando e con "Aggiorna"), così i conflitti risolti spariscono dall'elenco.
//! - **Avvio**: `merge.start` con il nome del branch (o l'id del commit) scelto. Un merge
//!   concluso (fast-forward o commit di merge) entra nel diario: «Annulla» lo annulla. Un merge
//!   fermo in conflitto **resta in corso** (non è un errore): compare il riquadro. Un rifiuto
//!   (modifiche locali, HEAD staccata, operazione già in corso) apre una finestra che lo spiega.
//! - **Continua**: un dialogo con il messaggio del commit di merge, precompilato con quello di
//!   git; un rifiuto (restano conflitti, un hook) resta nel dialogo.
//! - **Annulla il merge**: in due tempi, come l'eliminazione di uno stash: prima senza
//!   `confirm` (il motore risponde sempre con `ConfirmRequired` e cosa si perderebbe), poi, solo
//!   dopo il clic nel dialogo rosso, `"confirm": true`.

use crate::ErrorKind;
use crate::discard::{History, Phase};
use crate::fetch::Failure;
use crate::{Action, App, Done, Request, merge};
use project_domain::{Commit, Head, Oid, Operation, RefInfo, RepoPath, Status};
use serde::Deserialize;
use serde_json::{Value, json};

/// L'id del comando che legge l'operazione in corso.
pub const OPERATION: &str = "repo.operation";

/// Quanti file in conflitto si elencano nel riquadro; gli altri si vedono nell'area di stage.
pub const BANNER_FILES: usize = 8;

/// Un comando del merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Start,
    Continue,
    Abort,
}

impl Op {
    /// L'id del comando del Registry.
    pub fn command(self) -> &'static str {
        match self {
            Op::Start => "merge.start",
            Op::Continue => "merge.continue",
            Op::Abort => "merge.abort",
        }
    }
}

/// Il nome dell'operazione, per il riquadro: «Merge», «Rebase», ...
pub fn operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "Merge",
        Operation::Rebase => "Rebase",
        Operation::CherryPick => "Cherry-pick",
        Operation::Revert => "Revert",
        Operation::Bisect => "Bisect",
        Operation::Am => "`git am`",
    }
}

/// Come si conclude da terminale un'operazione che la UI non sa ancora concludere.
pub fn terminal_hint(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => "da terminale: `git merge --continue` o `git merge --abort`",
        Operation::Rebase => "da terminale: `git rebase --continue` o `git rebase --abort`",
        Operation::CherryPick => {
            "da terminale: `git cherry-pick --continue` o `git cherry-pick --abort`"
        }
        Operation::Revert => "da terminale: `git revert --continue` o `git revert --abort`",
        Operation::Bisect => "da terminale: `git bisect reset` quando la ricerca è finita",
        Operation::Am => "da terminale: `git am --continue` o `git am --abort`",
    }
}

/// Un commit in ingresso (`MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Incoming {
    pub id: Oid,
    /// Il nome breve del primo branch o tag che ci punta, altrimenti l'id abbreviato.
    pub label: String,
    /// L'oggetto del commit.
    pub summary: Option<String>,
    /// I ref (nomi completi) che ci puntano.
    #[serde(default)]
    pub refs: Vec<String>,
}

/// Risultato di `repo.operation`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OperationInfo {
    pub operation: Option<Operation>,
    #[serde(default)]
    pub conflicts: Vec<RepoPath>,
    #[serde(default)]
    pub incoming: Vec<Incoming>,
    /// Il messaggio che `merge.continue` userebbe.
    pub message: Option<String>,
    /// Per un rebase: il commit su cui si riapplica e il branch riscritto.
    pub onto: Option<Oid>,
    pub rebase_branch: Option<String>,
    /// C'è un'operazione e non restano file in conflitto.
    pub ready: bool,
    /// L'id del comando che conclude l'operazione da qui, se c'è.
    #[serde(rename = "continue")]
    pub continue_command: Option<String>,
    /// L'id del comando che annulla l'operazione da qui, se c'è.
    #[serde(rename = "abort")]
    pub abort_command: Option<String>,
}

impl OperationInfo {
    /// Cosa sta entrando, per il riquadro: `«feature» (Titolo)`, più voci separate da virgole.
    pub fn incoming_text(&self) -> Option<String> {
        if self.incoming.is_empty() {
            return None;
        }
        Some(
            self.incoming
                .iter()
                .map(|i| match &i.summary {
                    Some(summary) => format!("«{}» ({summary})", i.label),
                    None => format!("«{}»", i.label),
                })
                .collect::<Vec<_>>()
                .join(", "),
        )
    }

    /// La riga del riquadro: tipo di operazione e cosa sta entrando (o, per un rebase, su cosa
    /// si riapplica).
    pub fn title(&self) -> String {
        let Some(operation) = self.operation else {
            return String::new();
        };
        let name = operation_name(operation);
        let mut text = format!("{name} in corso");
        match operation {
            Operation::Rebase => {
                if let Some(branch) = &self.rebase_branch {
                    let short = branch.strip_prefix("refs/heads/").unwrap_or(branch);
                    text.push_str(&format!(" di «{short}»"));
                }
                if let Some(onto) = self.onto {
                    text.push_str(&format!(" su {}", onto.short()));
                }
            }
            Operation::Merge => {
                if let Some(incoming) = self.incoming_text() {
                    text.push_str(&format!(": entra {incoming}"));
                }
            }
            _ => {
                if let Some(incoming) = self.incoming_text() {
                    text.push_str(&format!(": {incoming}"));
                }
            }
        }
        text
    }
}

/// Perché "Unisci" non si può offrire, o in quale branch si unirebbe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// Si unisce nel branch attivo con questo nome.
    Into(String),
    /// Non si può: il motivo da mostrare.
    Blocked(&'static str),
}

impl Gate {
    /// Da HEAD e dallo stato: un merge va fatto su un branch con commit, senza operazioni in
    /// corso né conflitti (il motore controlla di nuovo, con le modifiche locali).
    pub fn new(head: &Head, status: Option<&Status>) -> Self {
        let branch = match head {
            Head::Branch {
                name,
                target: Some(_),
            } => name.clone(),
            Head::Branch { target: None, .. } => {
                return Gate::Blocked("Il branch attivo non ha ancora commit");
            }
            Head::Detached(_) => {
                return Gate::Blocked(
                    "HEAD è staccata: il merge va fatto su un branch. Passare prima a un branch",
                );
            }
        };
        if let Some(status) = status {
            if status.operation.is_some() {
                return Gate::Blocked(
                    "C'è già un'operazione in corso: concluderla o annullarla dal riquadro in \
                     alto",
                );
            }
            if status.has_conflicts() {
                return Gate::Blocked("Ci sono file in conflitto: risolverli prima");
            }
        }
        Gate::Into(branch)
    }
}

/// Il dialogo "Continua il merge": il messaggio del commit di merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueDialog {
    pub message: String,
    /// L'errore del motore all'ultimo invio (restano conflitti, un hook), nel dialogo.
    pub error: Option<String>,
    /// Il testo di git dell'errore, se c'è.
    pub git: Option<String>,
    /// Vero mentre il comando è in corso.
    pub sending: bool,
}

impl ContinueDialog {
    /// Vero se il messaggio è vuoto o di soli spazi: il pulsante resta spento.
    pub fn is_blank(&self) -> bool {
        self.message.trim().is_empty()
    }
}

/// Il dialogo aperto: uno alla volta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Continue(ContinueDialog),
    /// L'annullamento: controllo, conferma con il motivo del motore, invio.
    Abort(Phase),
}

/// Il merge avviato e non ancora risposto.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Starting {
    /// Cosa si unisce, come mostrato (nome del branch o id abbreviato).
    what: String,
    /// Il branch in cui si unisce.
    into: String,
}

/// Lo stato della UI per merge e operazione in corso.
#[derive(Debug, Default)]
pub struct MergeBox {
    /// L'ultima lettura di `repo.operation`; `None` senza operazione in corso.
    info: Option<OperationInfo>,
    /// L'errore dell'ultima lettura.
    info_error: Option<String>,
    /// Il numero dello stato per cui si è chiesto `repo.operation` l'ultima volta.
    asked_for: Option<u64>,
    /// Vero mentre `repo.operation` è in corso.
    reading: bool,
    starting: Option<Starting>,
    dialog: Option<Dialog>,
    /// La finestra di un avvio rifiutato o non riuscito.
    failure: Option<Failure>,
}

/// Cosa è arrivato da un comando del merge.
#[derive(Debug)]
pub enum Received {
    /// Fatto: cosa aggiornare nella vista.
    Done(Box<Outcome>),
    /// Rifiutato: l'errore è nel dialogo "Continua", che resta aperto.
    Inline,
    /// Serve la conferma dell'annullamento: il dialogo si apre.
    Confirm,
    /// Rifiutato o non riuscito: il messaggio per la barra di stato; con `reload` si rilegge
    /// tutto (qualcosa può essere cambiato).
    Failed { message: String, reload: bool },
}

/// Il risultato di un comando riuscito, per la vista.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    pub status: Option<Status>,
    pub refs: Option<Vec<RefInfo>>,
    pub history: History,
    /// Un problema dopo l'operazione riuscita (avvisi di git, stato o ref non riletti).
    pub warning: Option<String>,
    /// Il branch si è spostato: si rilegge il log e si seleziona il nuovo HEAD.
    pub moved: bool,
    /// Il merge è fermo in conflitto: si apre l'area di stage con i file da risolvere.
    pub show_changes: bool,
}

impl MergeBox {
    /// Un nuovo stato del repo (numero `seq`, crescente) con `operation`: senza operazione il
    /// riquadro sparisce; con un'operazione, se per questo stato non si è ancora chiesto,
    /// `true`: va chiesto `repo.operation` (uno alla volta; finito, si richiede se nel frattempo
    /// lo stato è cambiato).
    pub fn want_info(&mut self, operation: Option<Operation>, seq: u64) -> bool {
        if operation.is_none() {
            self.info = None;
            self.info_error = None;
            self.asked_for = Some(seq);
            // Senza operazione non ci sono né Continua né Annulla da mostrare.
            if matches!(self.dialog, Some(Dialog::Continue(ref d)) if !d.sending)
                || matches!(self.dialog, Some(Dialog::Abort(Phase::Confirm { .. })))
            {
                self.dialog = None;
            }
            return false;
        }
        if self.reading || self.asked_for == Some(seq) {
            return false;
        }
        self.reading = true;
        self.asked_for = Some(seq);
        true
    }

    /// La risposta di `repo.operation`.
    pub fn receive_info(&mut self, result: Result<Value, String>) {
        self.reading = false;
        match result.and_then(|value| {
            serde_json::from_value::<OperationInfo>(value)
                .map_err(|e| format!("{OPERATION}: risultato inatteso: {e}"))
        }) {
            Ok(info) => {
                self.info = info.operation.is_some().then_some(info);
                self.info_error = None;
            }
            Err(e) => self.info_error = Some(e),
        }
    }

    /// L'operazione in corso letta da `repo.operation`, se c'è.
    pub fn info(&self) -> Option<&OperationInfo> {
        self.info.as_ref()
    }

    /// L'errore dell'ultima lettura di `repo.operation`.
    pub fn info_error(&self) -> Option<&str> {
        self.info_error.as_deref()
    }

    /// La finestra di un avvio rifiutato, se è aperta.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Chiude la finestra dell'avvio rifiutato.
    pub fn dismiss(&mut self) {
        self.failure = None;
    }

    /// Il dialogo da disegnare: l'annullamento si vede solo quando chiede la conferma.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Abort(phase) => matches!(phase, Phase::Confirm { .. }),
            Dialog::Continue(_) => true,
        })
    }

    /// Vero se un dialogo è aperto: niente altre scritture, i tasti vanno a lui.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// Vero se un comando del merge aspetta risposta.
    pub fn busy(&self) -> bool {
        self.starting.is_some()
            || match &self.dialog {
                Some(Dialog::Continue(d)) => d.sending,
                Some(Dialog::Abort(phase)) => !matches!(phase, Phase::Confirm { .. }),
                None => false,
            }
    }

    /// Comincia il merge di `rev` (mostrato come `what`) nel branch `into`: parametri e
    /// descrizione. `None` se un altro comando del merge è in corso o un dialogo è aperto.
    pub fn start(
        &mut self,
        rev: String,
        what: String,
        into: String,
        no_ff: bool,
    ) -> Option<(Value, String)> {
        if self.busy() || self.dialog.is_some() {
            return None;
        }
        let mut params = json!({ "rev": rev });
        if no_ff {
            params["no_ff"] = json!(true);
        }
        let label = format!("Merge di «{what}» in «{into}»");
        self.failure = None;
        self.starting = Some(Starting { what, into });
        Some((params, label))
    }

    /// Apre il dialogo "Continua il merge" con il messaggio preparato da git. `false` se non
    /// c'è un merge da concludere da qui o se un altro dialogo è aperto.
    pub fn open_continue(&mut self) -> bool {
        if self.dialog.is_some() || self.busy() {
            return false;
        }
        let Some(info) = &self.info else {
            return false;
        };
        if info.continue_command.as_deref() != Some(Op::Continue.command()) {
            return false;
        }
        let message = info.message.clone().unwrap_or_default();
        self.dialog = Some(Dialog::Continue(ContinueDialog {
            message: message.trim_end().to_owned(),
            error: None,
            git: None,
            sending: false,
        }));
        true
    }

    /// Il messaggio del dialogo "Continua" è cambiato.
    pub fn set_message(&mut self, message: String) {
        if let Some(Dialog::Continue(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.message = message;
        }
    }

    /// Invio del dialogo "Continua": parametri e descrizione. `None` se non si può (messaggio
    /// vuoto, invio già in corso).
    pub fn submit_continue(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Continue(dialog)) = &mut self.dialog else {
            return None;
        };
        if dialog.sending || dialog.is_blank() {
            return None;
        }
        dialog.sending = true;
        dialog.error = None;
        dialog.git = None;
        Some((
            json!({ "message": dialog.message.trim_end() }),
            "Conclusione del merge".to_owned(),
        ))
    }

    /// Comincia l'annullamento: i parametri del primo invio, senza `confirm` (il motore
    /// risponde con cosa si perderebbe). `None` se non c'è un merge da annullare da qui.
    pub fn start_abort(&mut self) -> Option<(Value, String)> {
        if self.dialog.is_some() || self.busy() {
            return None;
        }
        let info = self.info.as_ref()?;
        if info.abort_command.as_deref() != Some(Op::Abort.command()) {
            return None;
        }
        self.dialog = Some(Dialog::Abort(Phase::Checking));
        Some((
            json!({}),
            "Controllo per l'annullamento del merge".to_owned(),
        ))
    }

    /// L'utente ha confermato l'annullamento: i parametri con `"confirm": true`.
    pub fn confirm_abort(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Abort(phase)) = &mut self.dialog else {
            return None;
        };
        if !matches!(phase, Phase::Confirm { .. }) {
            return None;
        }
        *phase = Phase::Sending;
        Some((
            json!({ "confirm": true }),
            "Annullamento del merge".to_owned(),
        ))
    }

    /// Chiude il dialogo aperto senza mandare niente; non durante un invio.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Continue(dialog)) => !dialog.sending,
            Some(Dialog::Abort(phase)) => matches!(phase, Phase::Confirm { .. }),
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
            Op::Start => self.receive_start(result, kind, detail),
            Op::Continue => self.receive_continue(result, kind, detail),
            Op::Abort => self.receive_abort(result, kind, detail),
        }
    }

    fn receive_start(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(starting) = self.starting.take() else {
            return unexpected(Op::Start);
        };
        let what = format!("Merge di «{}» in «{}»", starting.what, starting.into);
        match result {
            Err(e) => {
                // Rifiuti del motore (modifiche locali, HEAD staccata, operazione in corso,
                // rev che non c'è): niente è cambiato. Altri errori (git): forse sì, si rilegge.
                let refused = matches!(
                    kind,
                    Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams | ErrorKind::UnknownRev)
                );
                let (title, explanation) = if refused {
                    (
                        "Merge non possibile".to_owned(),
                        format!("{what} non possibile: {e}"),
                    )
                } else {
                    (
                        "Merge non riuscito".to_owned(),
                        format!(
                            "{what} non riuscito: {e}. Il repo si rilegge per mostrare com'è \
                             adesso"
                        ),
                    )
                };
                let git = detail.filter(|d| kind == Some(ErrorKind::Git) && !d.trim().is_empty());
                self.failure = Some(Failure {
                    title,
                    explanation: explanation.clone(),
                    git,
                });
                Received::Failed {
                    message: explanation,
                    reload: !refused,
                }
            }
            Ok(value) => match parse::<StartResult>(Op::Start, value) {
                Ok(result) => {
                    let notice = result.notice(&starting);
                    let warning = reread_warning(
                        "Merge fatto",
                        &result.warnings,
                        result.status_error.clone(),
                        result.refs_error.clone(),
                    );
                    Received::Done(Box::new(Outcome {
                        notice,
                        moved: result.after != result.before,
                        show_changes: result.in_progress && !result.conflicts.is_empty(),
                        status: result.status,
                        refs: result.refs,
                        history: result.history,
                        warning,
                    }))
                }
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }

    fn receive_continue(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Continue(dialog)) = &mut self.dialog else {
            return unexpected(Op::Continue);
        };
        if !dialog.sending {
            return unexpected(Op::Continue);
        }
        match result {
            // Il commit è fatto ma dopo qualcosa non torna: non va ripetuto, si rilegge tutto.
            Err(e) if kind == Some(ErrorKind::PostCommit) => {
                self.dialog = None;
                Received::Failed {
                    message: format!("Merge concluso, ma: {e}"),
                    reload: true,
                }
            }
            // Restano conflitti, nessun merge in corso, messaggio non valido, un hook che
            // rifiuta, niente da committare: niente è committato, si corregge nel dialogo.
            Err(e) => {
                dialog.sending = false;
                dialog.git =
                    detail.filter(|d| kind == Some(ErrorKind::Git) && !d.trim().is_empty());
                dialog.error = Some(e);
                Received::Inline
            }
            Ok(value) => {
                self.dialog = None;
                match parse::<ContinueResult>(Op::Continue, value) {
                    Ok(result) => {
                        let warning = reread_warning(
                            "Merge concluso",
                            &[],
                            result.status_error.clone(),
                            result.refs_error.clone(),
                        );
                        let into = result
                            .branch
                            .as_deref()
                            .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b))
                            .map(|b| format!(" su «{b}»"))
                            .unwrap_or_default();
                        let notice = format!(
                            "Merge concluso: commit di merge {} creato{into}. «Annulla» lo annulla",
                            result.commit.id.short()
                        );
                        Received::Done(Box::new(Outcome {
                            notice,
                            status: result.status,
                            refs: result.refs,
                            history: result.history,
                            warning,
                            moved: true,
                            show_changes: false,
                        }))
                    }
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                }
            }
        }
    }

    fn receive_abort(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Abort(phase)) = self.dialog.take() else {
            return unexpected(Op::Abort);
        };
        match result {
            Err(_) if kind == Some(ErrorKind::ConfirmRequired) && phase == Phase::Checking => {
                self.dialog = Some(Dialog::Abort(Phase::Confirm {
                    reason: detail.unwrap_or_default(),
                }));
                Received::Confirm
            }
            Err(e) => {
                let refused =
                    matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams));
                let what = if refused {
                    "non possibile"
                } else {
                    "non riuscito"
                };
                Received::Failed {
                    message: format!("Annullamento del merge {what}: {e}"),
                    reload: !refused,
                }
            }
            Ok(value) => match parse::<AbortResult>(Op::Abort, value) {
                Ok(result) => {
                    let warning = reread_warning(
                        "Merge annullato",
                        &result.warnings,
                        result.status_error.clone(),
                        result.refs_error.clone(),
                    );
                    let files = match result.files.len() {
                        0 => String::new(),
                        1 => " (1 file)".to_owned(),
                        n => format!(" ({n} file)"),
                    };
                    let notice = format!(
                        "Merge annullato: indice e worktree salvati prima in {}{files}; si \
                         rimettono con `git stash apply {}`. Non si annulla con «Annulla»",
                        result.backup, result.backup
                    );
                    Received::Done(Box::new(Outcome {
                        notice,
                        status: result.status,
                        refs: result.refs,
                        history: result.history,
                        warning,
                        moved: false,
                        show_changes: false,
                    }))
                }
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }
}

fn unexpected(op: Op) -> Received {
    Received::Failed {
        message: format!("risposta di `{}` inattesa", op.command()),
        reload: true,
    }
}

fn parse<T: for<'de> Deserialize<'de>>(op: Op, value: Value) -> Result<T, String> {
    serde_json::from_value(value).map_err(|e| format!("{}: risultato inatteso: {e}", op.command()))
}

/// Gli avvisi di git e il problema dopo un'operazione riuscita: stato o ref non riletti.
fn reread_warning(
    what: &str,
    warnings: &[String],
    status: Option<String>,
    refs: Option<String>,
) -> Option<String> {
    let parts: Vec<String> = warnings
        .iter()
        .map(|w| format!("attenzione: {w}"))
        .chain(status.map(|e| format!("lo stato non si rilegge: {e}")))
        .chain(refs.map(|e| format!("i ref non si rileggono: {e}")))
        .collect();
    (!parts.is_empty()).then(|| format!("{what}, ma {}", parts.join("; ")))
}

/// Da dove viene il commit unito.
#[derive(Debug, Clone, Deserialize)]
struct Source {
    name: String,
}

/// Risultato di `merge.start`.
#[derive(Debug, Clone, Deserialize)]
struct StartResult {
    source: Source,
    branch: String,
    strategy: String,
    before: Oid,
    after: Oid,
    #[serde(default)]
    behind: usize,
    in_progress: bool,
    #[serde(default)]
    conflicts: Vec<RepoPath>,
    #[serde(default)]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

impl StartResult {
    /// Il messaggio per la barra di stato.
    fn notice(&self, starting: &Starting) -> String {
        let name = if self.source.name.is_empty() {
            starting.what.as_str()
        } else {
            self.source.name.as_str()
        };
        let branch = &self.branch;
        let commits = match self.behind {
            1 => "1 commit".to_owned(),
            n => format!("{n} commit"),
        };
        if self.in_progress {
            let files = match self.conflicts.len() {
                0 => {
                    return format!(
                        "Merge di «{name}» in «{branch}» fermo prima del commit (un hook?): \
                         concluderlo con «Continua il merge…» nel riquadro in alto, o annullarlo"
                    );
                }
                1 => "1 file in conflitto".to_owned(),
                n => format!("{n} file in conflitto"),
            };
            return format!(
                "Merge di «{name}» in «{branch}» fermo: {files}. Risolverli e segnarli come \
                 risolti nell'area di stage, poi «Continua il merge…» nel riquadro in alto (o \
                 «Annulla il merge…»)"
            );
        }
        match self.strategy.as_str() {
            "up_to_date" => format!("«{branch}» contiene già «{name}»: niente da unire"),
            "fast_forward" => format!(
                "Fast-forward: «{branch}» portato avanti di {commits} fino a «{name}» ({}). \
                 «Annulla» lo riporta indietro",
                self.after.short()
            ),
            _ => format!(
                "«{name}» unito in «{branch}» ({commits}): commit di merge {}. «Annulla» lo \
                 annulla",
                self.after.short()
            ),
        }
    }
}

/// Risultato di `merge.continue`.
#[derive(Debug, Clone, Deserialize)]
struct ContinueResult {
    commit: Commit,
    branch: Option<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

/// Risultato di `merge.abort`.
#[derive(Debug, Clone, Deserialize)]
struct AbortResult {
    backup: String,
    #[serde(default)]
    files: Vec<RepoPath>,
    #[serde(default)]
    warnings: Vec<String>,
    status: Option<Status>,
    status_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

/// Il nome del commit `id` da mostrare: il primo branch locale, remoto o tag che ci punta,
/// altrimenti l'id abbreviato. Serve al menu del commit.
pub fn commit_name(id: Oid, refs: &[RefInfo]) -> String {
    use project_domain::RefKind;
    for kind in [RefKind::LocalBranch, RefKind::RemoteBranch, RefKind::Tag] {
        if let Some(r) = refs.iter().find(|r| r.kind == kind && r.target == id) {
            return r.short_name().to_owned();
        }
    }
    id.short()
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_merge(&mut self, action: Action) {
        match action {
            Action::Merge { rev, what, no_ff } => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let merge::Gate::Into(into) = merge::Gate::new(&repo.head, repo.status.as_ref())
                else {
                    return;
                };
                if let Some((params, label)) = repo.merge.start(rev, what, into, no_ff) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = merge::Op::Start;
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::ContinueMerge => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.merge.open_continue();
                }
            }
            Action::MergeMessage(message) => {
                if let Some(repo) = &mut self.repo {
                    repo.merge.set_message(message);
                }
            }
            Action::SubmitContinueMerge => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.merge.submit_continue() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = merge::Op::Continue;
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::AbortMerge => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                if let Some((params, label)) = repo.merge.start_abort() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = merge::Op::Abort;
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::ConfirmAbortMerge => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.merge.confirm_abort() {
                    repo.write_label = Some(label);
                    let op = merge::Op::Abort;
                    self.submit(op.command(), params, Request::Merge(generation, op));
                }
            }
            Action::CloseMergeDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.merge.close();
                }
            }
            Action::DismissMergeError => {
                if let Some(repo) = &mut self.repo {
                    repo.merge.dismiss();
                }
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Risposta di `merge.*`: stato, ref e diario dalla risposta (il riquadro si rilegge con lo
    /// stato nuovo), poi il log riletto se il branch si è spostato, con il nuovo HEAD
    /// selezionato. Un merge fermo in conflitto apre l'area di stage; un avvio rifiutato apre la
    /// sua finestra, un rifiuto di "Continua" resta nel dialogo.
    pub(crate) fn receive_merge(&mut self, generation: u64, op: merge::Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .merge
            .receive(op, done.result, done.error_kind, done.error_detail)
        {
            merge::Received::Inline | merge::Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            merge::Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                repo.history = outcome.history.clone();
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
                    repo.stash.select(None);
                    repo.select_worktree(true);
                } else if outcome.moved && !repo.worktree_selected {
                    repo.select_after_load = repo.head.target();
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                self.follow_diff();
                if !refs_read {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                }
                if !status_read {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                }
                if outcome.moved
                    && let Some(repo) = self.current(generation)
                {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            merge::Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, ChangedFile, Signature};
    use project_engine::{MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

    fn oid(n: u64) -> Oid {
        MemoryBackend::fake_oid(n)
    }

    fn sig(n: u64) -> Signature {
        Signature {
            name: "Ada".into(),
            email: "ada@example.com".into(),
            time: 1_700_000_000 + i64::try_from(n).unwrap() * 60,
            offset_min: 0,
        }
    }

    /// Il commit `n` con genitore `parent` che porta `a.txt` da `old` (blob) a `new`.
    fn commit(
        repo: &mut MemoryBackend,
        n: u64,
        parent: Option<u64>,
        summary: &str,
        file: (&str, Option<u64>, u64),
    ) {
        let (path, old, new) = file;
        repo.set_blob(oid(100 + new), format!("{summary}\n"));
        repo.insert_commit(Commit {
            id: oid(n),
            parents: parent.map(oid).into_iter().collect(),
            author: sig(n),
            committer: sig(n),
            summary: summary.into(),
            body: String::new(),
        });
        repo.set_changes(
            oid(n),
            vec![ChangedFile {
                path: RepoPath::from(path),
                change: if old.is_some() {
                    Change::Modified
                } else {
                    Change::Added
                },
                original: None,
                score: None,
                old_mode: if old.is_some() { 0o100644 } else { 0 },
                new_mode: 0o100644,
                old_oid: old.map_or(Oid::from_bytes([0; Oid::LEN]), |o| oid(100 + o)),
                new_oid: oid(100 + new),
            }],
        );
    }

    /// `c1` base; `main` su `c2` (a.txt «nostra»); `altro` su `c3` da `c1` (a.txt «loro»:
    /// conflitto); `avanti` su `c4` da `c2` (fast-forward).
    fn opened() -> (Registry, Session) {
        let mut repo = MemoryBackend::new();
        repo.set_identity("Ada", "ada@example.com");
        commit(&mut repo, 1, None, "base", ("a.txt", None, 1));
        commit(&mut repo, 2, Some(1), "nostra", ("a.txt", Some(1), 2));
        commit(&mut repo, 3, Some(1), "loro", ("a.txt", Some(1), 3));
        commit(&mut repo, 4, Some(2), "avanti", ("b.txt", None, 4));
        repo.set_branch("main", oid(2));
        repo.set_branch("altro", oid(3));
        repo.set_branch("avanti", oid(4));
        repo.set_head_branch("main");
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        (registry, session)
    }

    /// Esegue `op` come l'executor e passa la risposta a `receive`.
    fn run(r: &Registry, s: &mut Session, b: &mut MergeBox, op: Op, params: Value) -> Received {
        let result = r.run(op.command(), s, params);
        let (kind, detail) = match &result {
            Err(e) => (Some(ErrorKind::of(e)), ErrorKind::detail(e)),
            Ok(_) => (None, None),
        };
        b.receive(op, result.map_err(|e| e.to_string()), kind, detail)
    }

    /// Rilegge stato e operazione come la UI: `want_info` e, se serve, `repo.operation`.
    fn refresh(r: &Registry, s: &mut Session, b: &mut MergeBox, seq: u64) {
        let status: Status =
            serde_json::from_value(r.run("repo.status", s, json!({})).unwrap()).unwrap();
        if b.want_info(status.operation, seq) {
            b.receive_info(r.run(OPERATION, s, json!({})).map_err(|e| e.to_string()));
        }
    }

    fn done(received: Received) -> Outcome {
        match received {
            Received::Done(outcome) => *outcome,
            other => panic!("atteso Done, arrivato {other:?}"),
        }
    }

    fn start(b: &mut MergeBox, rev: &str) -> Value {
        b.start(rev.into(), rev.into(), "main".into(), false)
            .unwrap()
            .0
    }

    #[test]
    fn conflitto_banner_risoluzione_e_conclusione() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        refresh(&r, &mut s, &mut b, 1);
        assert!(b.info().is_none());

        let params = start(&mut b, "altro");
        assert!(b.busy());
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(!outcome.moved);
        assert!(outcome.show_changes);
        assert!(
            outcome.notice.contains("1 file in conflitto"),
            "{}",
            outcome.notice
        );
        assert_eq!(
            outcome.status.as_ref().and_then(|s| s.operation),
            Some(Operation::Merge)
        );

        // Lo stato nuovo porta il riquadro.
        refresh(&r, &mut s, &mut b, 2);
        let info = b.info().expect("merge in corso");
        assert_eq!(info.operation, Some(Operation::Merge));
        assert_eq!(info.conflicts, vec![RepoPath::from("a.txt")]);
        assert!(!info.ready);
        assert_eq!(info.title(), "Merge in corso: entra «altro» (loro)");
        assert_eq!(info.continue_command.as_deref(), Some("merge.continue"));
        // Lo stesso stato non si richiede.
        assert!(!b.want_info(Some(Operation::Merge), 2));

        // Con i conflitti il motore rifiuta: l'errore resta nel dialogo.
        assert!(b.open_continue());
        let Some(Dialog::Continue(dialog)) = b.dialog() else {
            panic!("dialogo Continua");
        };
        assert!(
            dialog.message.starts_with("Merge branch 'altro'"),
            "{}",
            dialog.message
        );
        let (params, _) = b.submit_continue().unwrap();
        assert!(matches!(
            run(&r, &mut s, &mut b, Op::Continue, params),
            Received::Inline
        ));
        let Some(Dialog::Continue(dialog)) = b.dialog() else {
            panic!("il dialogo resta aperto");
        };
        assert!(dialog.error.as_deref().unwrap().contains("a.txt"));
        b.close();
        assert!(b.dialog().is_none());

        // Segnato come risolto: pronto.
        r.run("index.stage", &mut s, json!({ "paths": ["a.txt"] }))
            .unwrap();
        refresh(&r, &mut s, &mut b, 3);
        assert!(b.info().unwrap().ready);
        assert!(b.info().unwrap().conflicts.is_empty());

        assert!(b.open_continue());
        b.set_message("Unisci altro\n\nCon la versione nostra".into());
        let (params, label) = b.submit_continue().unwrap();
        assert_eq!(params["message"], "Unisci altro\n\nCon la versione nostra");
        assert_eq!(label, "Conclusione del merge");
        let outcome = done(run(&r, &mut s, &mut b, Op::Continue, params));
        assert!(outcome.moved);
        assert!(outcome.history.can_undo);
        assert!(
            outcome.notice.starts_with("Merge concluso"),
            "{}",
            outcome.notice
        );
        assert!(b.dialog().is_none());

        // Merge finito: il riquadro sparisce.
        refresh(&r, &mut s, &mut b, 4);
        assert!(b.info().is_none());
    }

    #[test]
    fn annullamento_in_due_tempi() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "altro");
        done(run(&r, &mut s, &mut b, Op::Start, params));
        refresh(&r, &mut s, &mut b, 1);

        let (params, _) = b.start_abort().unwrap();
        assert_eq!(params, json!({}));
        // Mentre controlla non si vede niente e non si può ricominciare.
        assert!(b.dialog().is_none());
        assert!(b.start_abort().is_none());
        assert!(matches!(
            run(&r, &mut s, &mut b, Op::Abort, params),
            Received::Confirm
        ));
        let Some(Dialog::Abort(Phase::Confirm { reason })) = b.dialog() else {
            panic!("conferma dell'annullamento");
        };
        assert!(reason.contains("a.txt"), "{reason}");
        assert!(b.modal_open());

        let (params, label) = b.confirm_abort().unwrap();
        assert_eq!(params, json!({ "confirm": true }));
        assert_eq!(label, "Annullamento del merge");
        let outcome = done(run(&r, &mut s, &mut b, Op::Abort, params));
        assert!(
            outcome.notice.contains("refs/project/backup/merge-abort-"),
            "{}",
            outcome.notice
        );
        assert!(!outcome.moved);
        assert_eq!(outcome.status.as_ref().unwrap().operation, None);
        refresh(&r, &mut s, &mut b, 2);
        assert!(b.info().is_none());
    }

    #[test]
    fn annullamento_rinunciato_non_manda_niente() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "altro");
        done(run(&r, &mut s, &mut b, Op::Start, params));
        refresh(&r, &mut s, &mut b, 1);
        let (params, _) = b.start_abort().unwrap();
        run(&r, &mut s, &mut b, Op::Abort, params);
        b.close();
        assert!(b.dialog().is_none());
        assert!(b.confirm_abort().is_none());
        refresh(&r, &mut s, &mut b, 2);
        assert!(b.info().is_some(), "il merge resta in corso");
    }

    #[test]
    fn fast_forward_e_gia_aggiornato() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "avanti");
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(outcome.moved);
        assert!(!outcome.show_changes);
        assert!(
            outcome.notice.starts_with("Fast-forward"),
            "{}",
            outcome.notice
        );
        assert!(outcome.history.can_undo);

        let params = start(&mut b, "avanti");
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(!outcome.moved);
        assert!(
            outcome.notice.contains("niente da unire"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn rifiuto_apre_la_finestra() {
        let (r, mut s) = opened();
        let mut b = MergeBox::default();
        let params = start(&mut b, "non-esiste");
        let Received::Failed { reload, message } = run(&r, &mut s, &mut b, Op::Start, params)
        else {
            panic!("rifiutato");
        };
        assert!(!reload);
        assert!(message.contains("non-esiste"), "{message}");
        let failure = b.failure().expect("finestra");
        assert_eq!(failure.title, "Merge non possibile");
        assert!(!b.busy());
        b.dismiss();
        assert!(b.failure().is_none());

        // `no_ff`: un commit di merge anche dove basterebbe il fast-forward.
        let params = b
            .start("avanti".into(), "avanti".into(), "main".into(), true)
            .unwrap()
            .0;
        assert_eq!(params, json!({ "rev": "avanti", "no_ff": true }));
        let outcome = done(run(&r, &mut s, &mut b, Op::Start, params));
        assert!(
            outcome.notice.contains("commit di merge"),
            "{}",
            outcome.notice
        );
    }

    #[test]
    fn gate_del_menu() {
        let head = Head::Branch {
            name: "main".into(),
            target: Some(oid(1)),
        };
        assert_eq!(Gate::new(&head, None), Gate::Into("main".into()));
        assert!(matches!(
            Gate::new(&Head::Detached(oid(1)), None),
            Gate::Blocked(_)
        ));
        let empty = Head::Branch {
            name: "main".into(),
            target: None,
        };
        assert!(matches!(Gate::new(&empty, None), Gate::Blocked(_)));
        let status = Status {
            head: head.clone(),
            upstream: None,
            entries: Vec::new(),
            operation: Some(Operation::Rebase),
        };
        assert!(matches!(Gate::new(&head, Some(&status)), Gate::Blocked(_)));
    }

    #[test]
    fn titolo_di_rebase_e_cherry_pick() {
        let info: OperationInfo = serde_json::from_value(json!({
            "operation": "Rebase",
            "head": { "Detached": oid(1).to_string() },
            "conflicts": ["x.txt"],
            "incoming": [],
            "message": null,
            "onto": oid(2).to_string(),
            "rebase_branch": "refs/heads/feature",
            "ready": false,
            "continue": null,
            "abort": null,
        }))
        .unwrap();
        assert_eq!(
            info.title(),
            format!("Rebase in corso di «feature» su {}", oid(2).short())
        );
        let mut b = MergeBox {
            info: Some(info),
            ..MergeBox::default()
        };
        // Senza comandi da qui niente dialoghi.
        assert!(!b.open_continue());
        assert!(b.start_abort().is_none());

        let pick: OperationInfo = serde_json::from_value(json!({
            "operation": "CherryPick",
            "conflicts": [],
            "incoming": [{ "id": oid(3).to_string(), "label": "abc1234", "summary": "Correggi", "refs": [] }],
            "message": "Correggi\n",
            "onto": null,
            "rebase_branch": null,
            "ready": true,
            "continue": null,
            "abort": null,
        }))
        .unwrap();
        assert_eq!(pick.title(), "Cherry-pick in corso: «abc1234» (Correggi)");
    }

    #[test]
    fn nome_del_commit_dai_ref() {
        let refs = vec![RefInfo {
            name: "refs/remotes/origin/x".into(),
            kind: project_domain::RefKind::RemoteBranch,
            target: oid(5),
            tag_object: None,
            upstream: None,
        }];
        assert_eq!(commit_name(oid(5), &refs), "origin/x");
        assert_eq!(commit_name(oid(6), &refs), oid(6).short());
    }
}
