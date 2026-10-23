//! Gestione dei remoti dalla sezione "Remoti" della colonna dei ref (S27): `remote.list`,
//! `remote.add`, `remote.remove`. Solo dati, senza egui: si prova con test unitari.
//!
//! L'elenco viene da `remote.list` (anche i remoti senza branch scaricati, e i nomi con `/`
//! interi), letto all'apertura e riletto dopo aggiunta, rimozione, fetch, pull e push. Gli URL
//! arrivano già mascherati dal motore: qui non si vedono mai password o token.
//!
//! L'aggiunta si scrive in un dialogo (nome, URL, "Scarica subito"); gli errori del motore
//! restano nel dialogo. Con il fetch il dialogo mostra l'avanzamento e "Interrompi": un fetch
//! fallito o interrotto non annulla l'aggiunta. La rimozione va in due tempi come
//! l'eliminazione di un branch: prima senza `confirm` (il motore risponde sempre con
//! `ConfirmRequired` e il motivo), poi, solo dopo il clic nel dialogo rosso, `"confirm": true`.

use crate::discard::{History, Phase};
use crate::fetch::{self, Failure, FetchResult, RefUpdate};
use crate::{Action, App, Done, Request, remotes};
use crate::{ErrorKind, Ticket};
use project_domain::{RefInfo, Upstream};
use project_engine::mask_url;
use serde::Deserialize;
use serde_json::{Value, json};

/// L'id del comando che elenca i remoti.
pub const LIST: &str = "remote.list";

/// Un comando che cambia i remoti.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Add,
    Remove,
}

impl Op {
    /// L'id del comando del Registry.
    pub fn command(self) -> &'static str {
        match self {
            Op::Add => "remote.add",
            Op::Remove => "remote.remove",
        }
    }
}

/// Un remoto configurato, come lo dà `remote.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Remote {
    pub name: String,
    /// L'URL del fetch, con le credenziali mascherate; `None` per un remoto senza URL.
    pub url: Option<String>,
    /// Dove va il push se non è `url` (mascherati).
    #[serde(default)]
    pub push_urls: Vec<String>,
    #[serde(default)]
    pub mirror: bool,
    /// Quanti branch remoti `refs/remotes/<nome>/...` ci sono.
    pub branches: usize,
    /// È il remoto del "Fetch" senza scelta.
    #[serde(default)]
    pub default: bool,
    /// È `remote.pushDefault`.
    #[serde(default)]
    pub push_default: bool,
    /// L'URL configurato contiene credenziali (salvate in chiaro da git).
    #[serde(default)]
    pub credentials: bool,
}

impl Remote {
    /// L'URL da mostrare.
    pub fn url_text(&self) -> &str {
        self.url.as_deref().unwrap_or("(senza URL)")
    }

    /// `nessun branch scaricato`, `1 branch`, `3 branch`.
    pub fn branches_text(&self) -> String {
        match self.branches {
            0 => "nessun branch".to_owned(),
            n => format!("{n} branch"),
        }
    }

    /// Le etichette accanto al nome, con la loro spiegazione.
    pub fn badges(&self) -> Vec<(&'static str, String)> {
        let mut badges = Vec::new();
        if self.default {
            badges.push((
                "predefinito",
                "«Fetch» nella barra scarica da qui (upstream del branch attivo, origin o \
                 l'unico remoto)"
                    .to_owned(),
            ));
        }
        if self.push_default {
            badges.push((
                "push",
                format!(
                    "remote.pushDefault: i push dei branch senza pushRemote vanno su «{}»",
                    self.name
                ),
            ));
        }
        if self.mirror {
            badges.push((
                "mirror",
                "remote.<nome>.mirror: un push rispecchia tutti i ref locali".to_owned(),
            ));
        }
        badges
    }

    /// Il testo del suggerimento della riga.
    pub fn tooltip(&self) -> String {
        let mut text = format!("Remoto «{}»\nURL: {}", self.name, self.url_text());
        for url in &self.push_urls {
            text.push_str(&format!("\nPush: {url}"));
        }
        text.push_str(&format!("\n{}", self.branches_text()));
        if self.credentials {
            text.push_str(
                "\nL'URL contiene credenziali, salvate in chiaro nella configurazione del repo \
                 (qui mascherate): meglio un credential helper",
            );
        }
        text
    }
}

/// Risultato di `remote.list`.
#[derive(Debug, Clone, Deserialize)]
struct ListResult {
    remotes: Vec<Remote>,
}

/// Vero se l'URL scritto contiene credenziali, con la stessa regola del motore
/// ([`mask_url`]): solo per l'avviso nel dialogo, prima di mandarlo. Il motore lo dice comunque
/// nei `warnings` dopo l'aggiunta.
pub fn looks_like_credentials(url: &str) -> bool {
    mask_url(url.trim()).1
}

/// I campi del dialogo "Aggiungi remoto".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddFields {
    pub name: String,
    pub url: String,
    /// "Scarica subito (fetch)".
    pub fetch: bool,
}

/// Il dialogo "Aggiungi remoto".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddDialog {
    pub fields: AddFields,
    /// L'errore del motore all'ultimo invio, mostrato nel dialogo.
    pub error: Option<String>,
    /// Vero mentre il comando è in corso.
    pub sending: bool,
    /// Il ticket del comando in corso (per l'avanzamento e "Interrompi").
    pub ticket: Option<Ticket>,
    /// Vero dopo "Interrompi", finché il comando non risponde.
    pub cancelling: bool,
    /// Vero se l'invio in corso ha chiesto anche il fetch.
    pub fetching: bool,
    /// "Nascondi": l'URL con credenziali si mostra mascherato anche mentre si scrive.
    pub hide_url: bool,
}

impl AddDialog {
    /// Vero se il pulsante è attivo: nome e URL scritti, nessun invio in corso. Il resto lo
    /// controlla il motore.
    pub fn can_submit(&self) -> bool {
        !self.sending && !self.fields.name.trim().is_empty() && !self.fields.url.trim().is_empty()
    }

    /// Vero se l'URL si mostra mascherato in un campo di sola lettura: durante l'invio (il
    /// fetch può durare, il token non resta in chiaro sullo schermo) o dopo "Nascondi".
    pub fn url_masked(&self) -> bool {
        (self.sending || self.hide_url) && looks_like_credentials(&self.fields.url)
    }

    /// L'URL come si mostra nel campo: mascherato come lo mostra il motore ([`mask_url`]) se
    /// [`AddDialog::url_masked`], altrimenti com'è scritto.
    pub fn shown_url(&self) -> String {
        if self.url_masked() {
            mask_url(self.fields.url.trim()).0
        } else {
            self.fields.url.clone()
        }
    }

    /// L'avviso da mostrare sotto l'URL, se contiene credenziali.
    pub fn credentials_warning(&self) -> Option<&'static str> {
        looks_like_credentials(&self.fields.url).then_some(
            "L'URL contiene credenziali: git le salva in chiaro nella configurazione del repo \
             (.git/config). Meglio un URL senza e un credential helper (o una chiave SSH).",
        )
    }

    /// Il remoto che si sta aggiungendo, per la barra di stato.
    pub fn label(&self) -> String {
        let name = self.fields.name.trim();
        if self.fetching {
            format!("Aggiunta del remoto «{name}» e fetch")
        } else {
            format!("Aggiunta del remoto «{name}»")
        }
    }
}

/// La rimozione di un remoto, dal controllo alla risposta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveDialog {
    pub name: String,
    pub phase: Phase,
}

/// Il dialogo dei remoti aperto (o la rimozione in controllo, ancora senza dialogo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Add(AddDialog),
    Remove(RemoveDialog),
}

/// L'elenco dei remoti e i dialoghi per cambiarlo: un dialogo alla volta.
#[derive(Debug, Default)]
pub struct RemotesBox {
    /// L'ultimo elenco letto; `None` finché non arriva.
    list: Option<Vec<Remote>>,
    /// L'errore dell'ultima lettura dell'elenco.
    list_error: Option<String>,
    dialog: Option<Dialog>,
    /// Il fetch dopo l'aggiunta non è riuscito per un motivo da spiegare.
    failure: Option<Failure>,
}

/// Cosa è arrivato da `remote.add`/`remote.remove`.
#[derive(Debug)]
pub enum Received {
    /// Fatto: cosa aggiornare nella vista.
    Done(Box<Outcome>),
    /// Rifiutato: l'errore è nel dialogo dell'aggiunta, che resta aperto.
    Inline,
    /// Serve la conferma della rimozione: il dialogo si apre.
    Confirm,
    /// Rifiutato o non riuscito: il messaggio da mostrare; con `reload` si rilegge tutto.
    Failed { message: String, reload: bool },
}

/// Il risultato di un comando riuscito, per la vista.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    /// I ref nuovi, se il comando li ha riletti.
    pub refs: Option<Vec<RefInfo>>,
    /// L'elenco nuovo dei remoti, se il comando l'ha riletto.
    pub remotes: Option<Vec<Remote>>,
    /// Un problema dopo l'operazione riuscita (ref o remoti non riletti, fetch non riuscito).
    pub warning: Option<String>,
    pub history: History,
}

impl RemotesBox {
    /// L'elenco dei remoti, se è arrivato.
    pub fn list(&self) -> Option<&[Remote]> {
        self.list.as_deref()
    }

    /// L'errore dell'ultima lettura dell'elenco.
    pub fn list_error(&self) -> Option<&str> {
        self.list_error.as_deref()
    }

    /// I nomi dei remoti, in ordine (vuoto finché l'elenco non arriva).
    pub fn names(&self) -> Vec<String> {
        self.list.iter().flatten().map(|r| r.name.clone()).collect()
    }

    /// La risposta di `remote.list`.
    pub fn receive_list(&mut self, result: Result<Value, String>) {
        match result.and_then(|value| {
            serde_json::from_value::<ListResult>(value)
                .map_err(|e| format!("{LIST}: risultato inatteso: {e}"))
        }) {
            Ok(list) => {
                self.list = Some(list.remotes);
                self.list_error = None;
            }
            Err(e) => self.list_error = Some(e),
        }
    }

    fn set_list(&mut self, remotes: Vec<Remote>) {
        self.list = Some(remotes);
        self.list_error = None;
    }

    /// Il dialogo da disegnare: la rimozione si vede solo quando chiede la conferma.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Remove(remove) => matches!(remove.phase, Phase::Confirm { .. }),
            Dialog::Add(_) => true,
        })
    }

    /// Vero se un dialogo è aperto: niente altre scritture, i tasti vanno a lui.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// Il fetch dopo l'aggiunta non riuscito, da spiegare in una finestra.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Chiude la finestra dell'errore del fetch.
    pub fn dismiss(&mut self) {
        self.failure = None;
    }

    /// Apre il dialogo "Aggiungi remoto": nome proposto `origin` se non c'è ancora nessun
    /// remoto con quel nome, fetch subito attivo.
    pub fn open_add(&mut self) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        let taken = self.list.iter().flatten().any(|r| r.name == "origin");
        self.dialog = Some(Dialog::Add(AddDialog {
            fields: AddFields {
                name: if taken {
                    String::new()
                } else {
                    "origin".into()
                },
                url: String::new(),
                fetch: true,
            },
            error: None,
            sending: false,
            ticket: None,
            cancelling: false,
            fetching: false,
            hide_url: false,
        }));
        true
    }

    /// I campi del dialogo dell'aggiunta sono cambiati.
    pub fn set_fields(&mut self, fields: AddFields) {
        if let Some(Dialog::Add(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.fields = fields;
        }
    }

    /// "Mostra"/"Nascondi" l'URL con credenziali nel dialogo.
    pub fn hide_url(&mut self, hide: bool) {
        if let Some(Dialog::Add(dialog)) = &mut self.dialog {
            dialog.hide_url = hide;
        }
    }

    /// Invio del dialogo dell'aggiunta: parametri e descrizione. `None` se non si può.
    pub fn submit_add(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Add(dialog)) = &mut self.dialog else {
            return None;
        };
        if !dialog.can_submit() {
            return None;
        }
        dialog.sending = true;
        dialog.error = None;
        dialog.cancelling = false;
        dialog.fetching = dialog.fields.fetch;
        self.failure = None;
        let params = json!({
            "name": dialog.fields.name.trim(),
            "url": dialog.fields.url.trim(),
            "fetch": dialog.fields.fetch,
        });
        Some((params, dialog.label()))
    }

    /// L'aggiunta mandata con [`RemotesBox::submit_add`] è partita con `ticket`.
    pub fn add_started(&mut self, ticket: Ticket) {
        if let Some(Dialog::Add(dialog)) = &mut self.dialog {
            dialog.ticket = Some(ticket);
        }
    }

    /// L'aggiunta in corso con il fetch: il ticket per l'avanzamento.
    pub fn running_fetch(&self) -> Option<&AddDialog> {
        match &self.dialog {
            Some(Dialog::Add(dialog)) if dialog.sending && dialog.fetching => Some(dialog),
            _ => None,
        }
    }

    /// "Interrompi" il fetch dell'aggiunta: il ticket da annullare, una volta sola. Il remoto
    /// resta aggiunto.
    pub fn cancel(&mut self) -> Option<Ticket> {
        let Some(Dialog::Add(dialog)) = &mut self.dialog else {
            return None;
        };
        if !dialog.sending || !dialog.fetching || dialog.cancelling {
            return None;
        }
        dialog.cancelling = true;
        dialog.ticket
    }

    /// Comincia la rimozione del remoto `name`: i parametri del primo invio, senza `confirm`
    /// (il motore risponde con il motivo della conferma).
    pub fn start_remove(&mut self, name: String) -> Option<Value> {
        if self.dialog.is_some() {
            return None;
        }
        let params = json!({ "name": name });
        self.dialog = Some(Dialog::Remove(RemoveDialog {
            name,
            phase: Phase::Checking,
        }));
        Some(params)
    }

    /// L'utente ha confermato la rimozione: i parametri con `"confirm": true`.
    pub fn confirm_remove(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Remove(remove)) = &mut self.dialog else {
            return None;
        };
        if !matches!(remove.phase, Phase::Confirm { .. }) {
            return None;
        }
        remove.phase = Phase::Sending;
        Some((
            json!({ "name": remove.name, "confirm": true }),
            format!("Rimozione del remoto «{}»", remove.name),
        ))
    }

    /// Chiude il dialogo aperto senza mandare niente; non durante un invio.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Add(dialog)) => !dialog.sending,
            Some(Dialog::Remove(remove)) => matches!(remove.phase, Phase::Confirm { .. }),
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
        let received = match op {
            Op::Add => self.receive_add(result, kind),
            Op::Remove => self.receive_remove(result, kind, detail),
        };
        if let Received::Done(outcome) = &received
            && let Some(remotes) = &outcome.remotes
        {
            self.set_list(remotes.clone());
        }
        received
    }

    fn receive_add(&mut self, result: Result<Value, String>, kind: Option<ErrorKind>) -> Received {
        let Some(Dialog::Add(dialog)) = &mut self.dialog else {
            return unexpected(Op::Add);
        };
        if !dialog.sending {
            return unexpected(Op::Add);
        }
        match result {
            // Nome o URL rifiutati, nome già preso: niente è cambiato, si corregge nel dialogo.
            Err(e) if matches!(kind, Some(ErrorKind::InvalidParams | ErrorKind::NotAllowed)) => {
                dialog.sending = false;
                dialog.ticket = None;
                dialog.fetching = false;
                dialog.error = Some(e);
                Received::Inline
            }
            Err(e) => {
                // Un errore di git durante l'aggiunta: forse il remoto c'è, si rilegge tutto.
                let name = dialog.fields.name.trim().to_owned();
                self.dialog = None;
                Received::Failed {
                    message: format!("Aggiunta del remoto «{name}» non riuscita: {e}"),
                    reload: true,
                }
            }
            Ok(value) => {
                self.dialog = None;
                match parse::<AddResult>(Op::Add, value) {
                    Ok(result) => {
                        let (outcome, failure) = result.outcome();
                        self.failure = failure;
                        Received::Done(Box::new(outcome))
                    }
                    Err(message) => Received::Failed {
                        message,
                        reload: true,
                    },
                }
            }
        }
    }

    fn receive_remove(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Remove(remove)) = self.dialog.take() else {
            return unexpected(Op::Remove);
        };
        match result {
            Err(_)
                if kind == Some(ErrorKind::ConfirmRequired) && remove.phase == Phase::Checking =>
            {
                self.dialog = Some(Dialog::Remove(RemoveDialog {
                    name: remove.name,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                }));
                Received::Confirm
            }
            Err(e) => {
                let refused = kind == Some(ErrorKind::NotAllowed);
                let what = if refused {
                    "non possibile"
                } else {
                    "non riuscita"
                };
                Received::Failed {
                    message: format!("Rimozione del remoto «{}» {what}: {e}", remove.name),
                    reload: !refused,
                }
            }
            Ok(value) => match parse::<RemoveResult>(Op::Remove, value) {
                Ok(result) => Received::Done(Box::new(result.outcome())),
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

/// `; attenzione: …` per ogni avviso.
fn warnings_text(warnings: &[String]) -> String {
    warnings
        .iter()
        .map(|w| format!("; attenzione: {w}"))
        .collect()
}

/// Il problema dopo un'operazione riuscita: ref o remoti non riletti.
fn reread_warning(what: &str, refs: Option<String>, remotes: Option<String>) -> Option<String> {
    let parts: Vec<String> = [
        refs.map(|e| format!("i ref non si rileggono: {e}")),
        remotes.map(|e| format!("i remoti non si rileggono: {e}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| format!("{what}, ma {}", parts.join("; ")))
}

/// Il fetch fatto dopo l'aggiunta.
#[derive(Debug, Clone, Deserialize)]
struct FetchPart {
    updated: Option<Vec<RefUpdate>>,
    #[serde(default)]
    messages: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
}

/// Il fetch non riuscito dopo l'aggiunta.
#[derive(Debug, Clone, Deserialize)]
struct FetchError {
    kind: String,
    message: String,
}

/// Risultato di `remote.add`.
#[derive(Debug, Clone, Deserialize)]
struct AddResult {
    remote: String,
    url: String,
    fetch: Option<FetchPart>,
    fetch_error: Option<FetchError>,
    #[serde(default)]
    warnings: Vec<String>,
    remotes: Option<Vec<Remote>>,
    remotes_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

/// Cosa dire nella finestra quando il fetch dopo l'aggiunta non riesce: il remoto c'è.
const ADDED_ANYWAY: &str = "Il remoto è stato aggiunto comunque, senza branch scaricati: \
                            riprovare con «Fetch» nella sezione Remoti, oppure «Annulla» per \
                            toglierlo.";

impl AddResult {
    fn outcome(self) -> (Outcome, Option<Failure>) {
        let name = &self.remote;
        let mut notice = format!("Remoto «{name}» aggiunto ({})", self.url);
        let mut failure = None;
        if let Some(fetch) = self.fetch {
            let summary = FetchResult {
                remote: name.clone(),
                branch: None,
                updated: fetch.updated,
                messages: fetch.messages,
                warnings: fetch.warnings,
                refs: None,
                refs_error: None,
            }
            .outcome()
            .notice;
            notice.push_str(&format!("; {}", crate::lowercase_first(&summary)));
        }
        if let Some(error) = self.fetch_error {
            let who = format!("Il remoto «{name}»");
            let detail = Some(error.message.clone());
            let window = |mut f: Failure| {
                f.explanation = f
                    .explanation
                    .replace("Niente è stato modificato.", ADDED_ANYWAY);
                f
            };
            match error.kind.as_str() {
                "cancelled" => notice.push_str(
                    "; fetch interrotto: i branch remoti già scaricati restano, «Fetch» nella \
                     sezione Remoti lo riprende",
                ),
                "auth" => {
                    failure = Some(window(fetch::auth_failure(&who, detail)));
                    notice.push_str("; fetch non riuscito: autenticazione rifiutata o richiesta");
                }
                "unreachable" => {
                    failure = Some(window(fetch::unreachable_failure(&who, detail)));
                    notice.push_str("; fetch non riuscito: remoto non raggiungibile");
                }
                _ => {
                    failure = Some(Failure {
                        title: "Fetch non riuscito".to_owned(),
                        explanation: format!("{} {ADDED_ANYWAY}", error.message),
                        git: None,
                    });
                    notice.push_str("; fetch non riuscito");
                }
            }
        }
        notice.push_str(". «Annulla» lo toglie");
        notice.push_str(&warnings_text(&self.warnings));
        let outcome = Outcome {
            notice,
            refs: self.refs,
            remotes: self.remotes,
            warning: reread_warning("Remoto aggiunto", self.refs_error, self.remotes_error),
            history: self.history,
        };
        (outcome, failure)
    }
}

/// Il remoto tolto, come lo fotografa il motore (basta il numero di branch remoti).
#[derive(Debug, Clone, Deserialize)]
struct Removed {
    #[serde(default)]
    refs: Vec<Value>,
}

/// Risultato di `remote.remove`.
#[derive(Debug, Clone, Deserialize)]
struct RemoveResult {
    remote: String,
    removed: Removed,
    #[serde(default)]
    upstreams_removed: Vec<String>,
    #[serde(default)]
    push_remotes_removed: Vec<String>,
    #[serde(default)]
    push_default_removed: bool,
    #[serde(default)]
    backups: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
    remotes: Option<Vec<Remote>>,
    remotes_error: Option<String>,
    refs: Option<Vec<RefInfo>>,
    refs_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

/// `«a», «b»`.
fn quoted(names: &[String]) -> String {
    names
        .iter()
        .map(|n| format!("«{n}»"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl RemoveResult {
    fn outcome(self) -> Outcome {
        let mut notice = format!("Remoto «{}» rimosso", self.remote);
        let branches = match self.removed.refs.len() {
            0 => "nessun branch remoto da togliere".to_owned(),
            1 => "tolto 1 branch remoto".to_owned(),
            n => format!("tolti {n} branch remoti"),
        };
        notice.push_str(&format!(" ({branches})"));
        if !self.upstreams_removed.is_empty() {
            notice.push_str(&format!(
                "; senza upstream: {}",
                quoted(&self.upstreams_removed)
            ));
        }
        if !self.push_remotes_removed.is_empty() {
            notice.push_str(&format!(
                "; tolto il pushRemote di {}",
                quoted(&self.push_remotes_removed)
            ));
        }
        if self.push_default_removed {
            notice.push_str("; tolto remote.pushDefault");
        }
        match self.backups.len() {
            0 => {}
            1 => notice.push_str(&format!("; backup in {}", self.backups[0])),
            n => notice.push_str(&format!("; {n} backup in refs/project/backup/remote-…")),
        }
        notice.push_str(". «Annulla» lo rimette");
        notice.push_str(&warnings_text(&self.warnings));
        Outcome {
            notice,
            refs: self.refs,
            remotes: self.remotes,
            warning: reread_warning("Remoto rimosso", self.refs_error, self.remotes_error),
            history: self.history,
        }
    }
}

/// Il nome breve dell'upstream: `origin/main`.
pub fn upstream_name(upstream: &Upstream) -> &str {
    upstream
        .name
        .strip_prefix("refs/remotes/")
        .unwrap_or(&upstream.name)
}

/// Avanti e indietro rispetto all'upstream, accanto a un branch: il testo breve (`↑2 ↓1`,
/// vuoto se in pari, `upstream sparito`) e la spiegazione per il suggerimento.
pub fn ahead_behind(upstream: &Upstream) -> (String, String) {
    let name = upstream_name(upstream);
    match (upstream.ahead, upstream.behind) {
        (Some(ahead), Some(behind)) => {
            let mut short = Vec::new();
            let mut long = Vec::new();
            if ahead > 0 {
                short.push(format!("↑{ahead}"));
                long.push(format!("{ahead} commit da pubblicare"));
            }
            if behind > 0 {
                short.push(format!("↓{behind}"));
                long.push(format!("{behind} commit da scaricare"));
            }
            let long = if long.is_empty() {
                format!("Allineato con {name}")
            } else {
                format!("{} rispetto a {name}", long.join(", "))
            };
            (short.join(" "), long)
        }
        _ => (
            "upstream sparito".to_owned(),
            format!(
                "L'upstream {name} non c'è più (tolto sul remoto o il remoto non lo ha più): un \
                 Fetch non lo riporta; Push lo ricrea"
            ),
        ),
    }
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_remotes(&mut self, action: Action) {
        match action {
            Action::AddRemote => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.remotes.open_add();
                }
            }
            Action::AddRemoteFields(fields) => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.set_fields(fields);
                }
            }
            Action::HideRemoteUrl(hide) => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.hide_url(hide);
                }
            }
            Action::SubmitAddRemote => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let Some((params, label)) = repo.remotes.submit_add() else {
                    return;
                };
                repo.write_error = None;
                repo.write_label = Some(label);
                let op = remotes::Op::Add;
                let ticket = self.executor.submit(op.command(), params);
                self.pending.insert(ticket, Request::Remote(generation, op));
                if let Some(repo) = &mut self.repo {
                    repo.remotes.add_started(ticket);
                }
            }
            Action::CancelAddRemote => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.remotes.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::RemoveRemote(name) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let label = format!("Controllo per la rimozione di «{name}»");
                if let Some(params) = repo.remotes.start_remove(name) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = remotes::Op::Remove;
                    self.submit(op.command(), params, Request::Remote(generation, op));
                }
            }
            Action::ConfirmRemoveRemote => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.remotes.confirm_remove() {
                    repo.write_label = Some(label);
                    let op = remotes::Op::Remove;
                    self.submit(op.command(), params, Request::Remote(generation, op));
                }
            }
            Action::CloseRemoteDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.close();
                }
            }
            Action::DismissRemoteError => {
                if let Some(repo) = &mut self.repo {
                    repo.remotes.dismiss();
                }
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Risposta di `remote.add`/`remote.remove`: elenco dei remoti, ref e diario dalla
    /// risposta, poi lo stato e il log riletti (i branch remoti scaricati o tolti cambiano il
    /// grafo e l'upstream del branch attivo). Un nome o URL rifiutato resta nel dialogo.
    pub(crate) fn receive_remote(&mut self, generation: u64, op: remotes::Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .remotes
            .receive(op, done.result, done.error_kind, done.error_detail)
        {
            remotes::Received::Inline | remotes::Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            remotes::Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                repo.history = outcome.history.clone();
                let refs_read = outcome.refs.is_some();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                let remotes_read = outcome.remotes.is_some();
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                if !refs_read {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                }
                if !remotes_read {
                    self.request_remotes(generation);
                }
                self.reload_after_fetch(generation);
            }
            remotes::Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    self.request_remotes(generation);
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{FetchFailure, MemoryBackend, MemoryOpener, Registry, Session};
    use std::sync::Arc;

    fn opened(repo: MemoryBackend) -> (Registry, Session) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        (registry, session)
    }

    /// Esegue `command` come l'executor e passa la risposta a `receive`.
    fn run(
        registry: &Registry,
        session: &mut Session,
        b: &mut RemotesBox,
        op: Op,
        params: Value,
    ) -> Received {
        let result = registry.run(op.command(), session, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        b.receive(op, result.map_err(|e| e.to_string()), kind, detail)
    }

    fn list(registry: &Registry, session: &mut Session, b: &mut RemotesBox) {
        let result = registry.run(LIST, session, json!({}));
        b.receive_list(result.map_err(|e| e.to_string()));
    }

    #[test]
    fn list_of_the_sample_repo() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        assert!(b.list().is_none());
        assert!(b.names().is_empty());
        list(&registry, &mut session, &mut b);
        let remotes = b.list().unwrap();
        assert_eq!(remotes.len(), 1);
        let origin = &remotes[0];
        assert_eq!(origin.name, "origin");
        assert_eq!(origin.url_text(), "https://example.com/progetto.git");
        assert_eq!(origin.branches_text(), "2 branch");
        assert!(origin.default);
        assert_eq!(origin.badges()[0].0, "predefinito");
        assert_eq!(b.names(), ["origin"]);
        assert_eq!(b.list_error(), None);

        b.receive_list(Err("rotto".into()));
        assert_eq!(b.list_error(), Some("rotto"));
        assert!(b.list().is_some(), "l'elenco di prima resta");
    }

    #[test]
    fn list_shows_remotes_without_branches_and_slashes() {
        let mut repo = MemoryBackend::sample();
        repo.add_remote("team/fork", "https://anna:segreto@example.com/fork.git");
        let (registry, mut session) = opened(repo);
        let mut b = RemotesBox::default();
        list(&registry, &mut session, &mut b);
        let fork = b
            .list()
            .unwrap()
            .iter()
            .find(|r| r.name == "team/fork")
            .unwrap();
        assert_eq!(fork.branches_text(), "nessun branch");
        assert!(fork.credentials);
        assert!(!fork.url_text().contains("segreto"), "{}", fork.url_text());
        assert!(fork.tooltip().contains("credenziali"));
    }

    #[test]
    fn credentials_are_spotted_in_the_dialog() {
        assert!(looks_like_credentials("https://anna:pw@example.com/r.git"));
        assert!(looks_like_credentials("https://ghp_token@github.com/x/y"));
        assert!(looks_like_credentials("ssh://git:pw@host/r"));
        assert!(looks_like_credentials("remote-x::https://t@host/r"));
        assert!(!looks_like_credentials("ssh://git@host/r"));
        assert!(!looks_like_credentials("git@github.com:x/y.git"));
        assert!(!looks_like_credentials("https://example.com/a@b"));
        assert!(!looks_like_credentials("/percorso/locale"));
    }

    #[test]
    fn add_with_fetch_on_the_memory_backend() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        list(&registry, &mut session, &mut b);
        assert!(b.open_add());
        assert!(!b.open_add(), "un dialogo alla volta");
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.fields.name, "", "origin è già preso");
        assert!(dialog.fields.fetch);
        assert!(!dialog.can_submit());
        assert_eq!(b.submit_add(), None);

        // Un nome già preso: l'errore resta nel dialogo.
        b.set_fields(AddFields {
            name: "origin".into(),
            url: "https://example.com/altro.git".into(),
            fetch: true,
        });
        let (params, _) = b.submit_add().unwrap();
        let r = run(&registry, &mut session, &mut b, Op::Add, params);
        assert!(matches!(r, Received::Inline), "{r:?}");
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(dialog.error.as_deref().unwrap().contains("esiste già"));
        assert!(!dialog.sending);

        // URL con credenziali: avviso nel dialogo e dopo.
        b.set_fields(AddFields {
            name: "fork".into(),
            url: " https://anna:pw@example.com/fork.git ".into(),
            fetch: true,
        });
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(dialog.credentials_warning().is_some());
        assert!(!dialog.url_masked(), "mentre si scrive si vede");
        b.hide_url(true);
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.shown_url(), "https://anna:***@example.com/fork.git");
        b.hide_url(false);
        let (params, label) = b.submit_add().unwrap();
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        // Durante l'invio (e il fetch) il token non è sullo schermo.
        assert!(dialog.url_masked());
        assert_eq!(dialog.shown_url(), "https://anna:***@example.com/fork.git");
        assert_eq!(label, "Aggiunta del remoto «fork» e fetch");
        assert_eq!(params["url"], "https://anna:pw@example.com/fork.git");
        b.add_started(Ticket::for_test(3));
        assert!(b.running_fetch().is_some());
        assert_eq!(b.cancel(), Some(Ticket::for_test(3)));
        assert_eq!(b.cancel(), None, "una volta sola");
        b.close();
        assert!(b.modal_open(), "non si chiude durante l'invio");
        let r = run(&registry, &mut session, &mut b, Op::Add, params);
        let Received::Done(outcome) = r else {
            panic!("{r:?}")
        };
        assert!(
            outcome.notice.starts_with(
                "Remoto «fork» aggiunto (https://anna:***@example.com/fork.git); fetch da «fork»"
            ),
            "{}",
            outcome.notice
        );
        assert!(outcome.notice.contains("«Annulla» lo toglie"));
        assert!(
            outcome
                .notice
                .contains("attenzione: l'URL contiene credenziali")
        );
        assert!(!outcome.notice.contains("pw@"));
        assert!(outcome.history.can_undo);
        assert!(b.dialog().is_none());
        assert_eq!(b.names(), ["fork", "origin"], "l'elenco dalla risposta");
        assert!(b.failure().is_none());
    }

    #[test]
    fn add_with_fetch_downloads_the_branches() {
        // Tolto origin e riaggiunto con lo stesso URL sotto un altro nome: il fetch ritrova i
        // branch del server in memoria.
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        let params = b.start_remove("origin".into()).unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);
        let (params, _) = b.confirm_remove().unwrap();
        assert!(matches!(
            run(&registry, &mut session, &mut b, Op::Remove, params),
            Received::Done(_)
        ));
        b.open_add();
        let Some(Dialog::Add(dialog)) = b.dialog() else {
            panic!()
        };
        assert_eq!(dialog.fields.name, "origin", "proposto: non c'è più");
        b.set_fields(AddFields {
            name: "upstream".into(),
            url: "https://example.com/progetto.git".into(),
            fetch: true,
        });
        let (params, _) = b.submit_add().unwrap();
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Add, params) else {
            panic!()
        };
        assert!(
            outcome.notice.contains("2 ref aggiornati"),
            "{}",
            outcome.notice
        );
        let refs = outcome.refs.unwrap();
        assert!(refs.iter().any(|r| r.name == "refs/remotes/upstream/main"));
        assert_eq!(b.list().unwrap()[0].branches, 2);
    }

    #[test]
    fn add_with_a_failed_fetch_keeps_the_remote_and_explains() {
        let mut repo = MemoryBackend::sample();
        // Il server in memoria di un URL già visto: rifiuta le credenziali.
        repo.add_remote("vecchio", "https://example.com/privato.git");
        repo.fail_fetch(
            "vecchio",
            Some(FetchFailure::Auth("fatal: Authentication failed".into())),
        );
        let (registry, mut session) = opened(repo);
        let mut b = RemotesBox::default();
        let params = b.start_remove("vecchio".into()).unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);
        let (params, _) = b.confirm_remove().unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);

        b.open_add();
        b.set_fields(AddFields {
            name: "privato".into(),
            url: "https://example.com/privato.git".into(),
            fetch: true,
        });
        let (params, _) = b.submit_add().unwrap();
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Add, params) else {
            panic!()
        };
        assert!(
            outcome
                .notice
                .contains("fetch non riuscito: autenticazione"),
            "{}",
            outcome.notice
        );
        let failure = b.failure().unwrap();
        assert_eq!(failure.title, "Autenticazione non riuscita");
        assert!(failure.explanation.contains("aggiunto comunque"));
        assert!(!failure.explanation.contains("Niente è stato modificato"));
        assert!(b.names().contains(&"privato".to_owned()));
        b.dismiss();
        assert!(b.failure().is_none());
    }

    #[test]
    fn remove_asks_for_confirmation_and_undo_restores() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        list(&registry, &mut session, &mut b);
        let params = b.start_remove("origin".into()).unwrap();
        assert_eq!(params, json!({ "name": "origin" }), "prima senza confirm");
        assert!(!b.modal_open(), "in controllo: niente dialogo");
        assert_eq!(b.start_remove("origin".into()), None);
        let r = run(&registry, &mut session, &mut b, Op::Remove, params);
        assert!(matches!(r, Received::Confirm), "{r:?}");
        let Some(Dialog::Remove(remove)) = b.dialog() else {
            panic!()
        };
        let Phase::Confirm { reason } = &remove.phase else {
            panic!()
        };
        assert!(reason.contains("main"), "{reason}");
        // Niente è cambiato finché non si conferma.
        list(&registry, &mut session, &mut b);
        assert_eq!(b.names(), ["origin"]);

        // "Non rimuovere" chiude senza mandare niente.
        b.close();
        assert!(b.dialog().is_none());
        let params = b.start_remove("origin".into()).unwrap();
        run(&registry, &mut session, &mut b, Op::Remove, params);
        let (params, label) = b.confirm_remove().unwrap();
        assert_eq!(params, json!({ "name": "origin", "confirm": true }));
        assert_eq!(label, "Rimozione del remoto «origin»");
        assert_eq!(b.confirm_remove(), None, "una volta sola");
        let r = run(&registry, &mut session, &mut b, Op::Remove, params);
        let Received::Done(outcome) = r else {
            panic!("{r:?}")
        };
        assert!(
            outcome
                .notice
                .starts_with("Remoto «origin» rimosso (tolti 2 branch remoti); senza upstream: «"),
            "{}",
            outcome.notice
        );
        assert!(outcome.notice.contains("«Annulla» lo rimette"));
        assert!(b.names().is_empty());
        let refs = outcome.refs.unwrap();
        assert!(!refs.iter().any(|r| r.name.starts_with("refs/remotes/")));
        assert!(refs.iter().all(|r| r.upstream.is_none()));

        registry.run("edit.undo", &mut session, json!({})).unwrap();
        list(&registry, &mut session, &mut b);
        assert_eq!(b.names(), ["origin"]);
        assert_eq!(b.list().unwrap()[0].branches, 2);
    }

    #[test]
    fn remove_of_an_unknown_remote_is_refused() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = RemotesBox::default();
        let params = b.start_remove("nessuno".into()).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Remove, params)
        else {
            panic!()
        };
        assert!(message.starts_with("Rimozione del remoto «nessuno» non possibile"));
        assert!(!reload);
        assert!(b.dialog().is_none());
    }

    #[test]
    fn ahead_behind_text_and_tooltip() {
        let up = |ahead, behind| Upstream {
            name: "refs/remotes/origin/main".to_owned(),
            ahead,
            behind,
        };
        assert_eq!(
            ahead_behind(&up(Some(0), Some(0))),
            (String::new(), "Allineato con origin/main".to_owned())
        );
        assert_eq!(
            ahead_behind(&up(Some(2), Some(1))),
            (
                "↑2 ↓1".to_owned(),
                "2 commit da pubblicare, 1 commit da scaricare rispetto a origin/main".to_owned()
            )
        );
        assert_eq!(ahead_behind(&up(Some(0), Some(3))).0, "↓3");
        let (short, long) = ahead_behind(&up(None, None));
        assert_eq!(short, "upstream sparito");
        assert!(long.contains("origin/main non c'è più"), "{long}");
        // Lo stato dà il nome breve.
        let status = Upstream {
            name: "origin/main".to_owned(),
            ahead: Some(1),
            behind: Some(0),
        };
        assert_eq!(
            ahead_behind(&status),
            (
                "↑1".to_owned(),
                "1 commit da pubblicare rispetto a origin/main".to_owned()
            )
        );
    }

    #[test]
    fn ahead_behind_of_the_sample_after_fetch() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        registry
            .run(fetch::COMMAND, &mut session, json!({}))
            .unwrap();
        let refs: Vec<RefInfo> =
            serde_json::from_value(registry.run("repo.refs", &mut session, json!({})).unwrap())
                .unwrap();
        let text = |name: &str| {
            let info = refs.iter().find(|r| r.name == name).unwrap();
            ahead_behind(info.upstream.as_ref().unwrap()).0
        };
        assert_eq!(text("refs/heads/main"), "↑1");
        assert_eq!(text("refs/heads/feature/grafo"), "↓1");
    }
}
