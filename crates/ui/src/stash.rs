//! Gli stash dalla sezione "Stash" della colonna dei ref (S28): `stash.list`, `stash.push`,
//! `stash.apply`, `stash.pop`, `stash.drop`. Solo dati, senza egui: si prova con test unitari.
//!
//! L'elenco viene da `stash.list`, letto all'apertura (e con "Aggiorna", dopo undo e redo) e
//! preso dalle risposte dei comandi. Ogni stash si disegna nel grafo accanto al commit `base`
//! (il `repo.log` della lista chiede `"stashes": true`, così quel commit c'è sempre).
//!
//! - **Push**: un dialogo con messaggio facoltativo, "includi i non tracciati" e "tieni lo
//!   stage"; un rifiuto del motore (niente da mettere da parte, conflitti, operazione in corso)
//!   resta nel dialogo. Entra nel diario: «Annulla» lo riapplica.
//! - **Apply/pop**: sempre con `index` ed `expected` (l'id visto): se l'elenco è cambiato il
//!   motore rifiuta e l'elenco si rilegge. Non entrano nel diario: l'avviso non propone
//!   «Annulla» (che riguarderebbe l'operazione di prima). Con `conflicts` non vuoto è un
//!   successo parziale: si apre una finestra con i file, che si risolvono dall'area di stage.
//! - **Drop**: in due tempi come la rimozione di un remoto: prima senza `confirm` (il motore
//!   risponde sempre con `ConfirmRequired` e cosa si perderebbe), poi, solo dopo il clic nel
//!   dialogo rosso, `"confirm": true`.

use crate::discard::{History, Phase};
use crate::{Action, App, Done, Request, stash};
use crate::{ErrorKind, format};
use project_domain::{Oid, Signature, Status};
use serde::Deserialize;
use serde_json::{Value, json};

/// L'id del comando che elenca gli stash.
pub const LIST: &str = "stash.list";

/// Un comando che cambia gli stash (o i file, per apply e pop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Push,
    Apply,
    Pop,
    Drop,
}

impl Op {
    /// L'id del comando del Registry.
    pub fn command(self) -> &'static str {
        match self {
            Op::Push => "stash.push",
            Op::Apply => "stash.apply",
            Op::Pop => "stash.pop",
            Op::Drop => "stash.drop",
        }
    }
}

/// Uno stash, come lo dà `stash.list`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct Stash {
    /// `n` di `stash@{n}`, 0 il più recente.
    pub index: usize,
    /// `stash@{n}`.
    pub name: String,
    /// Il commit dello stash: `commit.show`/`diff.commit` su questo id danno i file tracciati.
    pub id: Oid,
    /// Il commit su cui è stato creato.
    pub base: Oid,
    /// Il messaggio come in `git stash list`.
    pub message: String,
    pub branch: Option<String>,
    pub time: i64,
    /// Contiene anche file non tracciati (che `commit.show` non mostra).
    pub untracked: bool,
}

impl Stash {
    /// Il messaggio senza il prefisso `On <branch>: ` / `WIP on <branch>: `, che la riga
    /// mostra a parte.
    pub fn title(&self) -> &str {
        let message = self.message.as_str();
        for prefix in ["On ", "WIP on "] {
            if let Some(rest) = message.strip_prefix(prefix)
                && let Some((_, title)) = rest.split_once(": ")
            {
                return title;
            }
        }
        message
    }

    /// Data dello stash, es. `2026-10-23 09:12` (UTC).
    pub fn date(&self) -> String {
        format::date_time(&Signature {
            name: String::new(),
            email: String::new(),
            time: self.time,
            offset_min: 0,
        })
    }

    /// Branch e base: `su main · 1a2b3c4`, `HEAD staccata · 1a2b3c4`.
    pub fn origin(&self) -> String {
        match &self.branch {
            Some(branch) => format!("su {branch} · {}", self.base.short()),
            None => format!("HEAD staccata · {}", self.base.short()),
        }
    }

    /// Il testo del suggerimento della riga.
    pub fn tooltip(&self) -> String {
        let mut text = format!(
            "{}\n{}\n{}\nCreato su {} il {} (UTC)",
            self.name,
            self.message,
            self.id,
            self.base,
            self.date()
        );
        if self.untracked {
            text.push_str(
                "\nContiene anche file non tracciati: non si vedono nei file dello stash, ma \
                 applicandolo tornano",
            );
        }
        text.push_str("\nClic: mostra i file dello stash\nClic destro: applica, pop, elimina");
        text
    }
}

/// Risultato di `stash.list`.
#[derive(Debug, Clone, Deserialize)]
struct ListResult {
    stashes: Vec<Stash>,
}

/// I campi del dialogo "Stash".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PushFields {
    pub message: String,
    /// "Includi i file non tracciati" (`include_untracked`).
    pub include_untracked: bool,
    /// "Tieni lo stage" (`keep_index`).
    pub keep_index: bool,
}

/// Il dialogo "Stash".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushDialog {
    pub fields: PushFields,
    /// L'errore del motore all'ultimo invio, mostrato nel dialogo.
    pub error: Option<String>,
    /// Vero mentre il comando è in corso.
    pub sending: bool,
}

impl PushDialog {
    /// Il messaggio come lo vuole il motore: una riga, senza spazi in testa e in coda (vuoto
    /// = git scrive `WIP on ...`). Gli a capo incollati diventano spazi: il motore li
    /// rifiuterebbe.
    pub fn message(&self) -> String {
        self.fields
            .message
            .split(['\n', '\r'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// I parametri di `stash.push`.
    pub fn params(&self) -> Value {
        let mut params = json!({
            "include_untracked": self.fields.include_untracked,
            "keep_index": self.fields.keep_index,
        });
        let message = self.message();
        if !message.is_empty() {
            params["message"] = json!(message);
        }
        params
    }
}

/// L'eliminazione di uno stash, dal controllo alla risposta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropDialog {
    pub stash: Stash,
    pub phase: Phase,
}

/// Il dialogo degli stash aperto (o l'eliminazione in controllo, ancora senza dialogo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Push(PushDialog),
    Drop(DropDialog),
}

/// Apply o pop fermo in conflitto: lo stash è applicato in parte, i file sono da risolvere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflicts {
    pub op: Op,
    /// `stash@{n}` com'era quando è stato applicato.
    pub stash: String,
    pub files: Vec<String>,
}

/// L'elenco degli stash, lo stash selezionato e i dialoghi: un dialogo alla volta.
#[derive(Debug, Default)]
pub struct StashBox {
    /// L'ultimo elenco letto; `None` finché non arriva.
    list: Option<Vec<Stash>>,
    /// L'errore dell'ultima lettura dell'elenco.
    list_error: Option<String>,
    /// Lo stash mostrato a destra al posto dei dettagli del commit (per id: la posizione
    /// cambia a ogni push e drop).
    selected: Option<Oid>,
    dialog: Option<Dialog>,
    /// L'apply o il pop in corso (lo stash visto), per i messaggi.
    applying: Option<(Op, Stash)>,
    /// "Rimetti anche lo stage" per apply e pop (`restore_index`).
    pub restore_index: bool,
    /// La finestra dei conflitti dopo apply o pop.
    conflicts: Option<Conflicts>,
    /// Gli stash già applicati da un pop che poi non è riuscito a toglierli dall'elenco: le
    /// modifiche sono nei file, il pop non va ripetuto (resta Elimina).
    popped: Vec<Oid>,
}

/// Cosa è arrivato da un comando degli stash.
#[derive(Debug)]
pub enum Received {
    /// Fatto: cosa aggiornare nella vista.
    Done(Box<Outcome>),
    /// Rifiutato: l'errore è nel dialogo dello stash, che resta aperto.
    Inline,
    /// Serve la conferma dell'eliminazione: il dialogo si apre.
    Confirm,
    /// Rifiutato o non riuscito: il messaggio da mostrare; con `reload` si rilegge tutto,
    /// altrimenti solo l'elenco degli stash e il diario.
    Failed { message: String, reload: bool },
}

/// Il risultato di un comando riuscito, per la vista.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub op: Op,
    pub notice: String,
    /// Il nuovo stato, se il comando l'ha riletto (non c'è per il drop, che non tocca i file).
    pub status: Option<Status>,
    /// Il diario, per i comandi che ci entrano (push e drop); apply e pop non lo cambiano.
    pub history: Option<History>,
    /// Un problema dopo l'operazione riuscita (elenco o stato non riletti).
    pub warning: Option<String>,
    /// Vero se va riletto il log: l'insieme delle basi degli stash è cambiato (push, drop, pop
    /// riuscito), e una base che nessun branch contiene esce dal grafo.
    pub reload_log: bool,
}

impl StashBox {
    /// L'elenco degli stash, se è arrivato.
    pub fn list(&self) -> Option<&[Stash]> {
        self.list.as_deref()
    }

    /// L'errore dell'ultima lettura dell'elenco.
    pub fn list_error(&self) -> Option<&str> {
        self.list_error.as_deref()
    }

    /// La risposta di `stash.list`.
    pub fn receive_list(&mut self, result: Result<Value, String>) {
        match result.and_then(|value| {
            serde_json::from_value::<ListResult>(value)
                .map_err(|e| format!("{LIST}: risultato inatteso: {e}"))
        }) {
            Ok(list) => self.set_list(list.stashes),
            Err(e) => self.list_error = Some(e),
        }
    }

    /// Un elenco nuovo: lo stash selezionato che non c'è più si deseleziona.
    fn set_list(&mut self, stashes: Vec<Stash>) {
        if let Some(id) = self.selected
            && !stashes.iter().any(|s| s.id == id)
        {
            self.selected = None;
        }
        self.popped.retain(|id| stashes.iter().any(|s| s.id == *id));
        self.list = Some(stashes);
        self.list_error = None;
    }

    /// Vero se lo stash `id` è già stato applicato da un pop che non l'ha tolto dall'elenco:
    /// Pop non si offre più (riapplicherebbe le stesse modifiche), resta Elimina.
    pub fn pop_blocked(&self, id: Oid) -> bool {
        self.popped.contains(&id)
    }

    /// Lo stash selezionato, se c'è ancora nell'elenco.
    pub fn selected(&self) -> Option<&Stash> {
        let id = self.selected?;
        self.list.iter().flatten().find(|s| s.id == id)
    }

    /// Seleziona lo stash `id` (o nessuno).
    pub fn select(&mut self, id: Option<Oid>) {
        self.selected = id.filter(|id| self.list.iter().flatten().any(|s| s.id == *id));
    }

    /// Lo stash dell'elenco con questo id (la posizione può essere cambiata).
    pub fn find(&self, id: Oid) -> Option<&Stash> {
        self.list.iter().flatten().find(|s| s.id == id)
    }

    /// Il dialogo da disegnare: l'eliminazione si vede solo quando chiede la conferma.
    pub fn dialog(&self) -> Option<&Dialog> {
        self.dialog.as_ref().filter(|d| match d {
            Dialog::Drop(drop) => matches!(drop.phase, Phase::Confirm { .. }),
            Dialog::Push(_) => true,
        })
    }

    /// Vero se un dialogo è aperto: niente altre scritture, i tasti vanno a lui.
    pub fn modal_open(&self) -> bool {
        self.dialog().is_some()
    }

    /// La finestra dei conflitti di apply o pop, se è aperta.
    pub fn conflicts(&self) -> Option<&Conflicts> {
        self.conflicts.as_ref()
    }

    /// Chiude la finestra dei conflitti.
    pub fn dismiss(&mut self) {
        self.conflicts = None;
    }

    /// Apre il dialogo "Stash" con i campi vuoti.
    pub fn open_push(&mut self) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        self.dialog = Some(Dialog::Push(PushDialog {
            fields: PushFields::default(),
            error: None,
            sending: false,
        }));
        true
    }

    /// I campi del dialogo "Stash" sono cambiati.
    pub fn set_fields(&mut self, fields: PushFields) {
        if let Some(Dialog::Push(dialog)) = &mut self.dialog
            && !dialog.sending
        {
            dialog.fields = fields;
        }
    }

    /// Invio del dialogo "Stash": parametri e descrizione. `None` se non si può.
    pub fn submit_push(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Push(dialog)) = &mut self.dialog else {
            return None;
        };
        if dialog.sending {
            return None;
        }
        dialog.sending = true;
        dialog.error = None;
        Some((dialog.params(), "Stash delle modifiche".to_owned()))
    }

    /// Comincia l'apply (`pop`: falso) o il pop dello stash `id`: parametri e descrizione.
    /// `None` se lo stash non è nell'elenco o un altro apply è in corso.
    pub fn start_apply(&mut self, id: Oid, pop: bool) -> Option<(Value, String)> {
        if self.applying.is_some() || self.dialog.is_some() {
            return None;
        }
        if pop && self.pop_blocked(id) {
            return None;
        }
        let stash = self.find(id)?.clone();
        let op = if pop { Op::Pop } else { Op::Apply };
        let params = json!({
            "index": stash.index,
            "expected": stash.id.to_string(),
            "restore_index": self.restore_index,
        });
        let label = match op {
            Op::Pop => format!("Pop di {}", stash.name),
            _ => format!("Applicazione di {}", stash.name),
        };
        self.conflicts = None;
        self.applying = Some((op, stash));
        Some((params, label))
    }

    /// Comincia l'eliminazione dello stash `id`: i parametri del primo invio, senza `confirm`
    /// (il motore risponde con cosa si perderebbe).
    pub fn start_drop(&mut self, id: Oid) -> Option<(Value, String)> {
        if self.dialog.is_some() || self.applying.is_some() {
            return None;
        }
        let stash = self.find(id)?.clone();
        let params = json!({ "index": stash.index, "expected": stash.id.to_string() });
        let label = format!("Controllo per l'eliminazione di {}", stash.name);
        self.dialog = Some(Dialog::Drop(DropDialog {
            stash,
            phase: Phase::Checking,
        }));
        Some((params, label))
    }

    /// L'utente ha confermato l'eliminazione: i parametri con `"confirm": true`.
    pub fn confirm_drop(&mut self) -> Option<(Value, String)> {
        let Some(Dialog::Drop(drop)) = &mut self.dialog else {
            return None;
        };
        if !matches!(drop.phase, Phase::Confirm { .. }) {
            return None;
        }
        drop.phase = Phase::Sending;
        Some((
            json!({
                "index": drop.stash.index,
                "expected": drop.stash.id.to_string(),
                "confirm": true,
            }),
            format!("Eliminazione di {}", drop.stash.name),
        ))
    }

    /// Chiude il dialogo aperto senza mandare niente; non durante un invio.
    pub fn close(&mut self) {
        let closable = match &self.dialog {
            Some(Dialog::Push(dialog)) => !dialog.sending,
            Some(Dialog::Drop(drop)) => matches!(drop.phase, Phase::Confirm { .. }),
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
            Op::Push => self.receive_push(result, kind),
            Op::Apply | Op::Pop => self.receive_apply(op, result, kind),
            Op::Drop => self.receive_drop(result, kind, detail),
        }
    }

    fn receive_push(&mut self, result: Result<Value, String>, kind: Option<ErrorKind>) -> Received {
        let Some(Dialog::Push(dialog)) = &mut self.dialog else {
            return unexpected(Op::Push);
        };
        if !dialog.sending {
            return unexpected(Op::Push);
        }
        match result {
            // Niente da mettere da parte, conflitti, messaggio non valido: niente è cambiato,
            // si corregge nel dialogo.
            Err(e) if matches!(kind, Some(ErrorKind::InvalidParams | ErrorKind::NotAllowed)) => {
                dialog.sending = false;
                dialog.error = Some(e);
                Received::Inline
            }
            Err(e) => {
                // Un errore di git: forse lo stash c'è, si rilegge tutto.
                self.dialog = None;
                Received::Failed {
                    message: format!("Stash non riuscito: {e}"),
                    reload: true,
                }
            }
            Ok(value) => {
                self.dialog = None;
                match parse::<PushResult>(Op::Push, value) {
                    Ok(result) => {
                        let warning = reread_warning(
                            "Stash creato",
                            result.stashes_error.clone(),
                            result.status_error.clone(),
                        );
                        if let Some(list) = result.stashes {
                            self.set_list(list);
                        }
                        let untracked = if result.stash.untracked {
                            ", con i file non tracciati"
                        } else {
                            ""
                        };
                        Received::Done(Box::new(Outcome {
                            op: Op::Push,
                            notice: format!(
                                "Modifiche messe da parte in {} («{}»{untracked}). «Annulla» le \
                                 rimette",
                                result.stash.name,
                                result.stash.title()
                            ),
                            status: result.status,
                            history: Some(result.history),
                            warning,
                            reload_log: true,
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

    fn receive_apply(
        &mut self,
        op: Op,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
    ) -> Received {
        let Some((sent, stash)) = self.applying.take() else {
            return unexpected(op);
        };
        if sent != op {
            return unexpected(op);
        }
        let what = match op {
            Op::Pop => format!("Pop di {}", stash.name),
            _ => format!("Applicazione di {}", stash.name),
        };
        match result {
            // Modifiche locali sui file dello stash, elenco cambiato, conflitti aperti: niente
            // è cambiato. Si rilegge l'elenco (se è cambiato, si vede dov'è lo stash).
            Err(e) if matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams)) => {
                Received::Failed {
                    message: format!("{what} non possibile: {e}"),
                    reload: false,
                }
            }
            Err(e) => Received::Failed {
                message: format!("{what} non riuscita: {e}"),
                reload: true,
            },
            Ok(value) => match parse::<ApplyResult>(op, value) {
                Ok(result) => {
                    let mut warning = reread_warning(
                        &format!("{what} fatta"),
                        result.stashes_error.clone(),
                        result.status_error.clone(),
                    );
                    // Pop applicato ma lo stash non si è tolto: lo si dice in evidenza e il pop
                    // non si offre più per quello stash.
                    if op == Op::Pop && !result.dropped && result.conflicts.is_empty() {
                        if !self.popped.contains(&stash.id) {
                            self.popped.push(stash.id);
                        }
                        let text = format!(
                            "{} applicato, ma non tolto dall'elenco: le modifiche sono già nei \
                             file. Non ripetere il pop; per toglierlo usare Elimina…{}",
                            stash.name,
                            warnings_text(&result.warnings)
                        );
                        warning = Some(match warning {
                            Some(w) => format!("{text}; {w}"),
                            None => text,
                        });
                    }
                    if let Some(list) = result.stashes.clone() {
                        self.set_list(list);
                    }
                    if !result.conflicts.is_empty() {
                        self.conflicts = Some(Conflicts {
                            op,
                            stash: stash.name.clone(),
                            files: result.conflicts.clone(),
                        });
                    }
                    let notice = result.notice(op, &stash);
                    Received::Done(Box::new(Outcome {
                        op,
                        notice,
                        status: result.status,
                        history: None,
                        warning,
                        reload_log: result.dropped,
                    }))
                }
                Err(message) => Received::Failed {
                    message,
                    reload: true,
                },
            },
        }
    }

    fn receive_drop(
        &mut self,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let Some(Dialog::Drop(drop)) = self.dialog.take() else {
            return unexpected(Op::Drop);
        };
        match result {
            Err(_) if kind == Some(ErrorKind::ConfirmRequired) && drop.phase == Phase::Checking => {
                self.dialog = Some(Dialog::Drop(DropDialog {
                    stash: drop.stash,
                    phase: Phase::Confirm {
                        reason: detail.unwrap_or_default(),
                    },
                }));
                Received::Confirm
            }
            Err(e) => {
                let refused =
                    matches!(kind, Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams));
                let what = if refused {
                    "non possibile"
                } else {
                    "non riuscita"
                };
                Received::Failed {
                    message: format!("Eliminazione di {} {what}: {e}", drop.stash.name),
                    reload: !refused,
                }
            }
            Ok(value) => match parse::<DropResult>(Op::Drop, value) {
                Ok(result) => {
                    let warning =
                        reread_warning("Stash eliminato", result.stashes_error.clone(), None);
                    if let Some(list) = result.stashes {
                        self.set_list(list);
                    }
                    let mut notice = format!(
                        "{} («{}») eliminato; backup in {}. «Annulla» lo rimette in cima \
                         all'elenco",
                        result.stash.name,
                        result.stash.title(),
                        result.backup
                    );
                    notice.push_str(&warnings_text(&result.warnings));
                    Received::Done(Box::new(Outcome {
                        op: Op::Drop,
                        notice,
                        status: None,
                        history: Some(result.history),
                        warning,
                        reload_log: true,
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

/// `; attenzione: …` per ogni avviso.
fn warnings_text(warnings: &[String]) -> String {
    warnings
        .iter()
        .map(|w| format!("; attenzione: {w}"))
        .collect()
}

/// Il problema dopo un'operazione riuscita: elenco o stato non riletti.
fn reread_warning(what: &str, stashes: Option<String>, status: Option<String>) -> Option<String> {
    let parts: Vec<String> = [
        stashes.map(|e| format!("gli stash non si rileggono: {e}")),
        status.map(|e| format!("lo stato non si rilegge: {e}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| format!("{what}, ma {}", parts.join("; ")))
}

/// Risultato di `stash.push`.
#[derive(Debug, Clone, Deserialize)]
struct PushResult {
    stash: Stash,
    stashes: Option<Vec<Stash>>,
    stashes_error: Option<String>,
    status: Option<Status>,
    status_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

/// Risultato di `stash.apply`/`stash.pop`.
#[derive(Debug, Clone, Deserialize)]
struct ApplyResult {
    #[serde(default)]
    conflicts: Vec<String>,
    #[serde(default)]
    dropped: bool,
    backup: Option<String>,
    #[serde(default)]
    warnings: Vec<String>,
    stashes: Option<Vec<Stash>>,
    stashes_error: Option<String>,
    status: Option<Status>,
    status_error: Option<String>,
}

impl ApplyResult {
    /// Il messaggio per la barra di stato. Apply e pop non entrano nel diario: non si dice
    /// mai che «Annulla» li annulla (annullerebbe l'operazione di prima).
    fn notice(&self, op: Op, stash: &Stash) -> String {
        let name = &stash.name;
        let mut notice = match (op, self.conflicts.len(), self.dropped) {
            (Op::Pop, 0, true) => {
                let backup = self
                    .backup
                    .as_deref()
                    .map(|b| format!("; backup in {b}"))
                    .unwrap_or_default();
                format!("{name} applicato ed eliminato dall'elenco{backup}")
            }
            (Op::Pop, 0, false) => format!(
                "{name} applicato, ma non tolto dall'elenco: non ripetere il pop, eliminarlo con \
                 Elimina…"
            ),
            (_, 0, _) => format!("{name} applicato; resta nell'elenco"),
            (_, 1, _) => format!(
                "{name} applicato con 1 file in conflitto: risolverlo e metterlo in stage; lo \
                 stash resta nell'elenco"
            ),
            (_, n, _) => format!(
                "{name} applicato con {n} file in conflitto: risolverli e metterli in stage; lo \
                 stash resta nell'elenco"
            ),
        };
        notice.push_str(". Non si annulla con «Annulla»");
        // L'avviso del motore sul pop in conflitto ripete quello che la finestra spiega.
        let warnings: Vec<String> = self
            .warnings
            .iter()
            .filter(|w| !(!self.conflicts.is_empty() && w.contains("resta nell'elenco")))
            .cloned()
            .collect();
        notice.push_str(&warnings_text(&warnings));
        notice
    }
}

/// Risultato di `stash.drop`.
#[derive(Debug, Clone, Deserialize)]
struct DropResult {
    stash: Stash,
    backup: String,
    #[serde(default)]
    warnings: Vec<String>,
    stashes: Option<Vec<Stash>>,
    stashes_error: Option<String>,
    #[serde(flatten)]
    history: History,
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_stash(&mut self, action: Action) {
        match action {
            Action::OpenStash => {
                if let Some(repo) = &mut self.repo
                    && self.executor.writing().is_none()
                    && !repo.modal_open()
                {
                    repo.stash.open_push();
                }
            }
            Action::StashFields(fields) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.set_fields(fields);
                }
            }
            Action::SubmitStash => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.stash.submit_push() {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = stash::Op::Push;
                    self.submit(op.command(), params, Request::Stash(generation, op));
                }
            }
            Action::SelectStash(id) => {
                let Some(repo) = &mut self.repo else {
                    return;
                };
                let Some(base) = repo.stash.find(id).map(|s| s.base) else {
                    return;
                };
                // La base si seleziona nel grafo (portata in vista), lo stash a destra.
                self.seek(base);
                if let Some(repo) = &mut self.repo {
                    repo.worktree_selected = false;
                    repo.stash.select(Some(id));
                }
            }
            Action::ApplyStash(id, pop) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                if let Some((params, label)) = repo.stash.start_apply(id, pop) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = if pop {
                        stash::Op::Pop
                    } else {
                        stash::Op::Apply
                    };
                    self.submit(op.command(), params, Request::Stash(generation, op));
                }
            }
            Action::StashRestoreIndex(on) => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.restore_index = on;
                }
            }
            Action::DropStash(id) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                if let Some((params, label)) = repo.stash.start_drop(id) {
                    repo.write_error = None;
                    repo.write_label = Some(label);
                    let op = stash::Op::Drop;
                    self.submit(op.command(), params, Request::Stash(generation, op));
                }
            }
            Action::ConfirmDropStash => {
                let generation = self.generation;
                if self.executor.writing().is_some() {
                    return;
                }
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if let Some((params, label)) = repo.stash.confirm_drop() {
                    repo.write_label = Some(label);
                    let op = stash::Op::Drop;
                    self.submit(op.command(), params, Request::Stash(generation, op));
                }
            }
            Action::CloseStashDialog => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.close();
                }
            }
            Action::DismissStashConflicts => {
                if let Some(repo) = &mut self.repo {
                    repo.stash.dismiss();
                }
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Risposta di `stash.*`: elenco, stato e diario dalla risposta, le etichette degli stash
    /// nel grafo rifatte e, se le basi sono cambiate, il log riletto. Un push rifiutato resta
    /// nel suo dialogo; i conflitti di apply e pop aprono la loro finestra.
    pub(crate) fn receive_stash(&mut self, generation: u64, op: stash::Op, done: Done) {
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        match repo
            .stash
            .receive(op, done.result, done.error_kind, done.error_detail)
        {
            stash::Received::Inline | stash::Received::Confirm => {
                self.error.clear();
                self.notice.clear();
            }
            stash::Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                repo.update_labels();
                if let Some(history) = outcome.history {
                    repo.history = history;
                }
                let status_read = outcome.status.is_some();
                if let Some(status) = outcome.status {
                    repo.set_status(status);
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                self.follow_diff();
                // Il drop non tocca i file; gli altri sì, e se lo stato non è arrivato si
                // rilegge.
                if !status_read && outcome.op != stash::Op::Drop {
                    self.submit("repo.status", json!({}), Request::Status(generation));
                }
                if outcome.reload_log
                    && let Some(repo) = self.current(generation)
                {
                    let first = repo.list.reset();
                    self.request_page(first);
                }
            }
            stash::Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.load();
                } else {
                    // L'elenco può essere cambiato (lo stash visto si è spostato): si rilegge.
                    self.submit(stash::LIST, json!({}), Request::Stashes(generation));
                    self.submit("edit.history", json!({}), Request::History(generation));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_domain::{Change, ChangedFile, Commit, EntryKind, RepoPath};
    use project_engine::{GitBackend, MemoryBackend, MemoryOpener, Registry, Session, StashPush};
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

    /// Esegue `op` come l'executor e passa la risposta a `receive`.
    fn run(
        registry: &Registry,
        session: &mut Session,
        b: &mut StashBox,
        op: Op,
        params: Value,
    ) -> Received {
        let result = registry.run(op.command(), session, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        b.receive(op, result.map_err(|e| e.to_string()), kind, detail)
    }

    fn list(registry: &Registry, session: &mut Session, b: &mut StashBox) {
        let result = registry.run(LIST, session, json!({}));
        b.receive_list(result.map_err(|e| e.to_string()));
    }

    fn push(registry: &Registry, session: &mut Session, b: &mut StashBox, fields: PushFields) {
        assert!(b.open_push());
        b.set_fields(fields);
        let (params, _) = b.submit_push().unwrap();
        let r = run(registry, session, b, Op::Push, params);
        assert!(matches!(r, Received::Done(_)), "{r:?}");
    }

    #[test]
    fn title_and_origin() {
        let stash = |message: &str, branch: Option<&str>| Stash {
            index: 0,
            name: "stash@{0}".into(),
            id: Oid::from_bytes([1; 20]),
            base: Oid::from_bytes([2; 20]),
            message: message.into(),
            branch: branch.map(Into::into),
            time: 0,
            untracked: true,
        };
        let s = stash("On main: prova: due punti", Some("main"));
        assert_eq!(s.title(), "prova: due punti");
        assert_eq!(s.origin(), "su main · 0202020");
        assert_eq!(s.date(), "1970-01-01 00:00");
        assert!(s.tooltip().contains("non tracciati"));
        let s = stash("WIP on (no branch): 1a2b3c4 Oggetto", None);
        assert_eq!(s.title(), "1a2b3c4 Oggetto");
        assert_eq!(s.origin(), "HEAD staccata · 0202020");
        assert_eq!(stash("salvato a mano", None).title(), "salvato a mano");
    }

    #[test]
    fn a_pop_that_could_not_drop_is_not_offered_again() {
        let stash = Stash {
            index: 0,
            name: "stash@{0}".into(),
            id: Oid::from_bytes([1; 20]),
            base: Oid::from_bytes([2; 20]),
            message: "On main: prova".into(),
            branch: Some("main".into()),
            time: 0,
            untracked: false,
        };
        let mut b = StashBox::default();
        b.set_list(vec![stash.clone()]);
        let (_, _) = b.start_apply(stash.id, true).unwrap();
        let value = json!({
            "stash": stash.clone(),
            "conflicts": [],
            "dropped": false,
            "backup": null,
            "warnings": ["lo stash `stash@{0}` è stato applicato ma non tolto dall'elenco (rotto): \
                          le modifiche sono nei file; eliminarlo con `stash.drop` (non ripetere \
                          il pop)"],
            "stashes": [stash.clone()],
            "status": null,
            "status_error": "x",
        });
        let Received::Done(outcome) = b.receive(Op::Pop, Ok(value), None, None) else {
            panic!()
        };
        assert!(
            outcome
                .notice
                .starts_with("stash@{0} applicato, ma non tolto dall'elenco: non ripetere il pop"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome.notice.contains("(rotto)"),
            "l'avviso del motore resta"
        );
        let warning = outcome.warning.unwrap();
        assert!(warning.contains("Non ripetere il pop"), "{warning}");
        assert!(!outcome.reload_log);
        assert!(b.pop_blocked(stash.id));
        assert_eq!(b.start_apply(stash.id, true), None, "niente secondo pop");
        assert!(b.start_drop(stash.id).is_some(), "resta Elimina");
    }

    #[test]
    fn push_dialog_params() {
        let mut d = PushDialog {
            fields: PushFields {
                message: "  prima riga\n seconda \r\n".into(),
                include_untracked: true,
                keep_index: false,
            },
            error: None,
            sending: false,
        };
        assert_eq!(
            d.params(),
            json!({ "message": "prima riga seconda", "include_untracked": true, "keep_index": false })
        );
        d.fields.message = "   ".into();
        d.fields.keep_index = true;
        assert_eq!(
            d.params(),
            json!({ "include_untracked": true, "keep_index": true }),
            "vuoto = assente"
        );
    }

    #[test]
    fn push_then_apply_keeps_the_stash() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        assert!(b.list().is_none());
        list(&registry, &mut session, &mut b);
        assert_eq!(b.list(), Some(&[][..]));

        assert!(b.open_push());
        assert!(!b.open_push(), "un dialogo alla volta");
        assert!(b.modal_open());
        b.set_fields(PushFields {
            message: "prova".into(),
            include_untracked: true,
            keep_index: false,
        });
        let (params, label) = b.submit_push().unwrap();
        assert_eq!(label, "Stash delle modifiche");
        assert_eq!(b.submit_push(), None, "una volta sola");
        b.close();
        assert!(b.modal_open(), "non si chiude durante l'invio");
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Push, params) else {
            panic!()
        };
        assert_eq!(
            outcome.notice,
            "Modifiche messe da parte in stash@{0} («prova», con i file non tracciati). \
             «Annulla» le rimette"
        );
        assert!(outcome.history.as_ref().unwrap().can_undo);
        assert!(
            outcome.status.unwrap().entries.is_empty(),
            "worktree pulito"
        );
        assert!(outcome.reload_log);
        assert!(b.dialog().is_none());
        let stash = b.list().unwrap()[0].clone();
        assert_eq!(stash.name, "stash@{0}");
        assert!(stash.untracked);

        // Selezione per id.
        b.select(Some(stash.id));
        assert_eq!(b.selected(), Some(&stash));
        b.select(Some(Oid::from_bytes([9; 20])));
        assert_eq!(b.selected(), None, "solo uno stash dell'elenco");
        b.select(Some(stash.id));

        let (params, label) = b.start_apply(stash.id, false).unwrap();
        assert_eq!(label, "Applicazione di stash@{0}");
        assert_eq!(
            params,
            json!({ "index": 0, "expected": stash.id.to_string(), "restore_index": false })
        );
        assert_eq!(b.start_apply(stash.id, true), None, "uno alla volta");
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Apply, params)
        else {
            panic!()
        };
        assert_eq!(
            outcome.notice,
            "stash@{0} applicato; resta nell'elenco. Non si annulla con «Annulla»"
        );
        assert!(outcome.history.is_none(), "apply non entra nel diario");
        assert!(!outcome.reload_log);
        assert!(!outcome.status.unwrap().entries.is_empty());
        assert_eq!(b.list().unwrap().len(), 1);
        assert_eq!(b.selected(), Some(&stash), "lo stash resta selezionato");
        assert!(b.conflicts().is_none());
    }

    #[test]
    fn push_with_nothing_to_save_stays_in_the_dialog() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        // Restano solo i non tracciati: senza "includi i non tracciati" non c'è niente.
        b.open_push();
        let (params, _) = b.submit_push().unwrap();
        let r = run(&registry, &mut session, &mut b, Op::Push, params);
        assert!(matches!(r, Received::Inline), "{r:?}");
        let Some(Dialog::Push(dialog)) = b.dialog() else {
            panic!()
        };
        assert!(dialog.error.is_some());
        assert!(!dialog.sending);
        b.close();
        assert!(b.dialog().is_none());
    }

    #[test]
    fn pop_drops_and_tells_the_backup() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let stash = b.list().unwrap()[0].clone();
        b.select(Some(stash.id));
        b.restore_index = true;
        let (params, label) = b.start_apply(stash.id, true).unwrap();
        assert_eq!(label, "Pop di stash@{0}");
        assert_eq!(params["restore_index"], true);
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Pop, params) else {
            panic!()
        };
        assert!(
            outcome
                .notice
                .starts_with("stash@{0} applicato ed eliminato dall'elenco; backup in refs/"),
            "{}",
            outcome.notice
        );
        assert!(outcome.notice.ends_with("Non si annulla con «Annulla»"));
        assert!(outcome.reload_log);
        assert!(outcome.history.is_none());
        assert_eq!(b.list(), Some(&[][..]));
        assert_eq!(
            b.selected(),
            None,
            "lo stash eliminato non è più selezionato"
        );
    }

    #[test]
    fn apply_of_a_changed_list_is_refused() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let old = b.list().unwrap()[0].clone();
        // Un altro stash arriva in cima senza che la UI lo sappia.
        registry
            .run(
                "stash.push",
                &mut session,
                json!({ "include_untracked": true }),
            )
            .unwrap();
        let (params, _) = b.start_apply(old.id, false).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Apply, params)
        else {
            panic!()
        };
        assert!(
            message.starts_with("Applicazione di stash@{0} non possibile: "),
            "{message}"
        );
        assert!(message.contains("ora è `stash@{1}`"), "{message}");
        assert!(!reload);
        // Riletto l'elenco, lo stesso stash si applica dalla sua nuova posizione.
        list(&registry, &mut session, &mut b);
        let (params, _) = b.start_apply(old.id, false).unwrap();
        assert_eq!(params["index"], 1);
        assert!(matches!(
            run(&registry, &mut session, &mut b, Op::Apply, params),
            Received::Done(_)
        ));
    }

    /// Il repo di esempio con le modifiche messe da parte, poi un commit che cambia
    /// `README.md` su `main`: lo stash va in conflitto su quel file.
    fn moved_head() -> MemoryBackend {
        let mut repo = MemoryBackend::sample();
        let old = match &repo.status().unwrap().entries[0].kind {
            EntryKind::Ordinary(info) => info.oid_head,
            other => panic!("{other:?}"),
        };
        repo.stash_push(&StashPush::default()).unwrap();
        let head = repo.head().unwrap().target().unwrap();
        let new = MemoryBackend::fake_oid(4242);
        repo.set_blob(new, "README cambiato su main\n");
        let id = MemoryBackend::fake_oid(4243);
        let who = Signature {
            name: "Ada".to_owned(),
            email: "ada@example.com".to_owned(),
            time: 1_800_000_000,
            offset_min: 0,
        };
        repo.insert_commit(Commit {
            id,
            parents: vec![head],
            author: who.clone(),
            committer: who,
            summary: "README".to_owned(),
            body: String::new(),
        });
        repo.set_changes(
            id,
            vec![ChangedFile {
                path: RepoPath::from("README.md"),
                change: Change::Modified,
                original: None,
                score: None,
                old_mode: 0o100644,
                new_mode: 0o100644,
                old_oid: old,
                new_oid: new,
            }],
        );
        repo.set_branch("main", id);
        repo
    }

    #[test]
    fn conflicts_open_the_window_and_pop_keeps_the_stash() {
        let (registry, mut session) = opened(moved_head());
        let mut b = StashBox::default();
        list(&registry, &mut session, &mut b);
        let stash = b.list().unwrap()[0].clone();
        let (params, _) = b.start_apply(stash.id, true).unwrap();
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Pop, params) else {
            panic!()
        };
        assert_eq!(
            outcome.notice,
            "stash@{0} applicato con 1 file in conflitto: risolverlo e metterlo in stage; lo \
             stash resta nell'elenco. Non si annulla con «Annulla»"
        );
        assert!(!outcome.reload_log, "lo stash resta: le basi non cambiano");
        let status = outcome.status.unwrap();
        assert!(
            status
                .entries
                .iter()
                .any(|e| matches!(e.kind, EntryKind::Unmerged(_)))
        );
        let conflicts = b.conflicts().unwrap();
        assert_eq!(conflicts.op, Op::Pop);
        assert_eq!(conflicts.stash, "stash@{0}");
        assert_eq!(conflicts.files, ["README.md"]);
        assert_eq!(
            b.list().unwrap().len(),
            1,
            "il pop in conflitto non elimina"
        );
        b.dismiss();
        assert!(b.conflicts().is_none());

        // Con il conflitto aperto un altro apply è rifiutato: il motivo nella barra.
        let (params, _) = b.start_apply(stash.id, false).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Apply, params)
        else {
            panic!()
        };
        assert!(message.contains("in conflitto"), "{message}");
        assert!(!reload);
    }

    #[test]
    fn drop_asks_for_confirmation_and_undo_restores() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let stash = b.list().unwrap()[0].clone();
        let (params, label) = b.start_drop(stash.id).unwrap();
        assert_eq!(
            params,
            json!({ "index": 0, "expected": stash.id.to_string() }),
            "prima senza confirm"
        );
        assert_eq!(label, "Controllo per l'eliminazione di stash@{0}");
        assert!(!b.modal_open(), "in controllo: niente dialogo");
        assert_eq!(b.start_drop(stash.id), None);
        let r = run(&registry, &mut session, &mut b, Op::Drop, params);
        assert!(matches!(r, Received::Confirm), "{r:?}");
        let Some(Dialog::Drop(drop)) = b.dialog() else {
            panic!()
        };
        let Phase::Confirm { reason } = &drop.phase else {
            panic!()
        };
        assert!(reason.contains("stash@{0}"), "{reason}");
        // Niente è cambiato finché non si conferma; "Non eliminare" chiude.
        list(&registry, &mut session, &mut b);
        assert_eq!(b.list().unwrap().len(), 1);
        b.close();
        assert!(b.dialog().is_none());

        let (params, _) = b.start_drop(stash.id).unwrap();
        run(&registry, &mut session, &mut b, Op::Drop, params);
        let (params, label) = b.confirm_drop().unwrap();
        assert_eq!(params["confirm"], true);
        assert_eq!(params["expected"], stash.id.to_string());
        assert_eq!(label, "Eliminazione di stash@{0}");
        assert_eq!(b.confirm_drop(), None, "una volta sola");
        let Received::Done(outcome) = run(&registry, &mut session, &mut b, Op::Drop, params) else {
            panic!()
        };
        assert!(
            outcome.notice.starts_with("stash@{0} («"),
            "{}",
            outcome.notice
        );
        assert!(
            outcome
                .notice
                .contains("backup in refs/project/backup/stash-")
        );
        assert!(outcome.notice.contains("«Annulla» lo rimette in cima"));
        assert!(outcome.history.unwrap().can_undo);
        assert!(outcome.status.is_none());
        assert_eq!(b.list(), Some(&[][..]));

        registry.run("edit.undo", &mut session, json!({})).unwrap();
        list(&registry, &mut session, &mut b);
        assert_eq!(b.list().unwrap()[0].id, stash.id);
    }

    #[test]
    fn drop_of_a_missing_stash_is_refused() {
        let (registry, mut session) = opened(MemoryBackend::sample());
        let mut b = StashBox::default();
        push(&registry, &mut session, &mut b, PushFields::default());
        let stash = b.list().unwrap()[0].clone();
        registry
            .run(
                "stash.drop",
                &mut session,
                json!({ "index": 0, "expected": stash.id.to_string(), "confirm": true }),
            )
            .unwrap();
        let (params, _) = b.start_drop(stash.id).unwrap();
        let Received::Failed { message, reload } =
            run(&registry, &mut session, &mut b, Op::Drop, params)
        else {
            panic!()
        };
        assert!(
            message.starts_with("Eliminazione di stash@{0} non possibile"),
            "{message}"
        );
        assert!(!reload);
        assert!(b.dialog().is_none());
    }
}
