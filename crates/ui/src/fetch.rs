//! Fetch dal pulsante della barra (S25): `remote.fetch` in background, con avanzamento e
//! "Interrompi". Solo dati, senza egui: si prova con test unitari.
//!
//! Un fetch alla volta. Mentre gira, la sessione resta bloccata dal job (§6 dell'architettura
//! git): `remote.fetch` è una scrittura ([`CommandSpec::writes`](project_engine::CommandSpec)),
//! così la UI spegne le altre scritture e "Aggiorna" finché non è finito. Gli errori di
//! autenticazione e di rete si spiegano in una finestra con il testo di git.

use crate::{Action, App, Done, Request, fetch};
use crate::{ErrorKind, Ticket};
use project_domain::{Oid, RefInfo};
use project_engine::Progress;
use serde::Deserialize;
use serde_json::{Value, json};

/// L'id del comando del Registry.
pub const COMMAND: &str = "remote.fetch";

/// La descrizione del fetch per la barra di stato.
pub fn label(remote: Option<&str>) -> String {
    match remote {
        Some(remote) => format!("Fetch da «{remote}»"),
        None => "Fetch".to_owned(),
    }
}

/// Il fetch in corso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Running {
    pub ticket: Ticket,
    /// Il remoto chiesto; `None` = quello scelto dal motore (upstream, `origin`, l'unico).
    pub remote: Option<String>,
    /// Vero dopo "Interrompi", finché il comando non risponde.
    pub cancelling: bool,
}

/// Un fetch non riuscito per un motivo da spiegare: la finestra con il testo di git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub title: String,
    /// Cosa è successo e cosa fare.
    pub explanation: String,
    /// Il testo di git (stderr), se c'è.
    pub git: Option<String>,
}

/// Il fetch: quello in corso e l'ultimo errore da spiegare.
#[derive(Debug, Default)]
pub struct FetchBox {
    running: Option<Running>,
    failure: Option<Failure>,
}

/// Cosa è arrivato da `remote.fetch`.
#[derive(Debug)]
pub enum Received {
    /// Fatto: cosa aggiornare nella vista.
    Done(Box<Outcome>),
    /// Interrotto con "Interrompi": i branch remoti già aggiornati restano, vanno riletti.
    Cancelled { notice: String },
    /// Non riuscito o rifiutato: il messaggio per la barra di stato (i dettagli, se servono,
    /// sono in [`FetchBox::failure`]); con `reload` si rileggono ref e log.
    Failed { message: String, reload: bool },
}

/// Il risultato di un fetch riuscito, per la vista.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub notice: String,
    /// I ref nuovi, se il comando li ha riletti.
    pub refs: Option<Vec<RefInfo>>,
    /// Vero se qualcosa è cambiato (o non si sa): log e stato si rileggono.
    pub changed: bool,
    /// Un problema dopo il fetch riuscito (ref non riletti).
    pub warning: Option<String>,
}

impl FetchBox {
    /// Il fetch in corso, se c'è.
    pub fn running(&self) -> Option<&Running> {
        self.running.as_ref()
    }

    /// L'errore da spiegare nella finestra, se c'è.
    pub fn failure(&self) -> Option<&Failure> {
        self.failure.as_ref()
    }

    /// Chiude la finestra dell'errore.
    pub fn dismiss(&mut self) {
        self.failure = None;
    }

    /// I parametri per un fetch da `remote` (`None`: quello scelto dal motore), se non ce n'è
    /// già uno in corso.
    pub fn request(&self, remote: Option<&str>) -> Option<Value> {
        if self.running.is_some() {
            return None;
        }
        Some(match remote {
            Some(remote) => json!({ "remote": remote }),
            None => json!({}),
        })
    }

    /// Il fetch chiesto con [`FetchBox::request`] è partito con `ticket`.
    pub fn started(&mut self, ticket: Ticket, remote: Option<String>) {
        self.failure = None;
        self.running = Some(Running {
            ticket,
            remote,
            cancelling: false,
        });
    }

    /// "Interrompi": il ticket da annullare, una volta sola.
    pub fn cancel(&mut self) -> Option<Ticket> {
        let running = self.running.as_mut().filter(|r| !r.cancelling)?;
        running.cancelling = true;
        Some(running.ticket)
    }

    /// La risposta di `remote.fetch` per `ticket`.
    pub fn receive(
        &mut self,
        ticket: Ticket,
        result: Result<Value, String>,
        kind: Option<ErrorKind>,
        detail: Option<String>,
    ) -> Received {
        let remote = match self.running.take() {
            Some(running) if running.ticket == ticket => running.remote,
            other => {
                self.running = other;
                return Received::Failed {
                    message: format!("risposta di `{COMMAND}` inattesa"),
                    reload: true,
                };
            }
        };
        let who = remote
            .as_deref()
            .map_or_else(|| "Il remoto".to_owned(), |r| format!("Il remoto «{r}»"));
        let label = label(remote.as_deref());
        let error = match result {
            Ok(value) => {
                return match serde_json::from_value::<FetchResult>(value) {
                    Ok(result) => Received::Done(Box::new(result.outcome())),
                    Err(e) => Received::Failed {
                        message: format!("{COMMAND}: risultato inatteso: {e}"),
                        reload: true,
                    },
                };
            }
            Err(error) => error,
        };
        match kind {
            Some(ErrorKind::Cancelled) => Received::Cancelled {
                notice: format!(
                    "{label} interrotto: i branch remoti già aggiornati restano, nessun branch \
                     locale né file è stato toccato"
                ),
            },
            Some(ErrorKind::Auth) => {
                self.failure = Some(auth_failure(&who, detail));
                Received::Failed {
                    message: format!("{label} non riuscito: autenticazione rifiutata o richiesta"),
                    reload: false,
                }
            }
            Some(ErrorKind::RemoteUnreachable) => {
                self.failure = Some(unreachable_failure(&who, detail));
                Received::Failed {
                    message: format!("{label} non riuscito: remoto non raggiungibile"),
                    reload: false,
                }
            }
            // Rifiutato prima di cominciare (remoto sconosciuto, nessun remoto): niente è
            // cambiato.
            Some(ErrorKind::NotAllowed | ErrorKind::InvalidParams) => Received::Failed {
                message: format!("{label} non possibile: {error}"),
                reload: false,
            },
            _ => {
                self.failure = Some(Failure {
                    title: "Fetch non riuscito".to_owned(),
                    explanation: error.clone(),
                    git: detail,
                });
                // Git potrebbe aver aggiornato qualche branch remoto prima di fallire.
                Received::Failed {
                    message: format!("{label} non riuscito: {error}"),
                    reload: true,
                }
            }
        }
    }
}

/// La finestra per un remoto (`who`: «Il remoto "origin"») che rifiuta le credenziali o
/// servirebbe chiederle; `detail` è il testo di git. Vale per fetch, pull e push.
pub fn auth_failure(who: &str, detail: Option<String>) -> Failure {
    Failure {
        title: "Autenticazione non riuscita".to_owned(),
        explanation: format!(
            "{who} ha rifiutato le credenziali, oppure servirebbe chiederle (nome utente, \
             password, passphrase della chiave SSH, conferma della chiave dell'host). Questa app \
             non chiede mai credenziali: configurare un credential helper (git config \
             credential.helper) oppure caricare la chiave in ssh-agent (ssh-add); per un host SSH \
             nuovo, aggiungerlo a known_hosts con un primo fetch da terminale. Poi riprovare. \
             Niente è stato modificato."
        ),
        git: detail,
    }
}

/// La finestra per un remoto che non si raggiunge (vedi [`auth_failure`]).
pub fn unreachable_failure(who: &str, detail: Option<String>) -> Failure {
    Failure {
        title: "Remoto non raggiungibile".to_owned(),
        explanation: format!(
            "{who} non risponde: controllare l'indirizzo del remoto (git remote -v), la rete e \
             che il repository esista sul server. Niente è stato modificato."
        ),
        git: detail,
    }
}

/// Un ref cambiato dal fetch.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RefUpdate {
    pub name: String,
    pub before: Option<Oid>,
    pub after: Option<Oid>,
}

/// Risultato di `remote.fetch`.
#[derive(Debug, Clone, Deserialize)]
pub struct FetchResult {
    pub remote: String,
    pub branch: Option<String>,
    pub updated: Option<Vec<RefUpdate>>,
    #[serde(default)]
    pub messages: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub refs: Option<Vec<RefInfo>>,
    pub refs_error: Option<String>,
}

/// Quanti nomi di ref elencare nell'avviso prima di "e altri N".
const NAMES_SHOWN: usize = 3;

/// Il nome di un ref per l'avviso: `origin/main`, `tag v1`.
fn ref_text(name: &str) -> String {
    if let Some(remote) = name.strip_prefix("refs/remotes/") {
        remote.to_owned()
    } else if let Some(tag) = name.strip_prefix("refs/tags/") {
        format!("tag {tag}")
    } else {
        name.to_owned()
    }
}

impl FetchResult {
    /// L'avviso per la barra di stato (anche per il fetch di `remote.add`).
    pub(crate) fn outcome(self) -> Outcome {
        // Le righe di git (`From ...`, `* [new branch] ...`) vanno solo nel log.
        for line in &self.messages {
            log::info!("{COMMAND}: {line}");
        }
        let what = match &self.branch {
            Some(branch) => format!("Fetch di «{branch}» da «{}»", self.remote),
            None => format!("Fetch da «{}»", self.remote),
        };
        let summary = match &self.updated {
            None => format!("{what} fatto"),
            Some(updated) if updated.is_empty() => format!("{what}: niente di nuovo"),
            Some(updated) => {
                let mut names: Vec<String> = updated
                    .iter()
                    .take(NAMES_SHOWN)
                    .map(|u| {
                        let name = ref_text(&u.name);
                        match (u.before, u.after) {
                            (None, Some(_)) => format!("{name} nuovo"),
                            (Some(_), None) => format!("{name} tolto"),
                            _ => name,
                        }
                    })
                    .collect();
                if updated.len() > NAMES_SHOWN {
                    names.push(format!("e altri {}", updated.len() - NAMES_SHOWN));
                }
                let count = match updated.len() {
                    1 => "1 ref aggiornato".to_owned(),
                    n => format!("{n} ref aggiornati"),
                };
                format!("{what}: {count} ({})", names.join(", "))
            }
        };
        let warnings: String = self
            .warnings
            .iter()
            .map(|w| format!("; attenzione: {w}"))
            .collect();
        Outcome {
            notice: summary + &warnings,
            changed: self.updated.as_ref().is_none_or(|u| !u.is_empty()),
            refs: self.refs,
            warning: self
                .refs_error
                .map(|e| format!("Fetch fatto, ma i ref non si rileggono: {e}")),
        }
    }
}

/// Il nome italiano di una fase di git, se è una di quelle note.
fn phase_name(phase: &str) -> &str {
    match phase {
        "Enumerating objects" => "Enumerazione oggetti",
        "Counting objects" => "Conteggio oggetti",
        "Compressing objects" => "Compressione oggetti",
        "Receiving objects" => "Ricezione oggetti",
        "Unpacking objects" => "Estrazione oggetti",
        "Resolving deltas" => "Risoluzione delta",
        "Writing objects" => "Scrittura oggetti",
        "Total" => "Totale",
        other => other,
    }
}

/// Come mostrare un passo di avanzamento: il testo (fase e conteggio) e la frazione per la
/// barra, se git dà la percentuale.
pub fn describe(progress: &Progress) -> (String, Option<f32>) {
    let mut text = String::new();
    if progress.remote {
        text.push_str("remoto: ");
    }
    text.push_str(phase_name(&progress.phase));
    match (progress.current, progress.total) {
        (Some(current), Some(total)) => text.push_str(&format!(" {current}/{total}")),
        (Some(current), None) => text.push_str(&format!(" {current}")),
        _ => {}
    }
    let fraction = progress.percent.map(|p| f32::from(p.min(100)) / 100.0);
    (text, fraction)
}

impl App {
    /// Le azioni di questa funzione, smistate da [`App::apply`].
    pub(crate) fn apply_fetch(&mut self, action: Action) {
        match action {
            Action::Fetch(remote) => {
                let generation = self.generation;
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.modal_open() {
                    return;
                }
                let Some(params) = repo.fetch.request(remote.as_deref()) else {
                    return;
                };
                repo.write_error = None;
                repo.write_label = Some(fetch::label(remote.as_deref()));
                let ticket = self.executor.submit(fetch::COMMAND, params);
                self.pending.insert(ticket, Request::Fetch(generation));
                if let Some(repo) = &mut self.repo {
                    repo.fetch.started(ticket, remote);
                }
            }
            Action::CancelFetch => {
                if let Some(ticket) = self.repo.as_mut().and_then(|r| r.fetch.cancel()) {
                    self.executor.cancel(ticket);
                }
            }
            Action::DismissFetchError => {
                if let Some(repo) = &mut self.repo {
                    repo.fetch.dismiss();
                }
            }
            _ => unreachable!("azione smistata alla funzione sbagliata"),
        }
    }

    /// Risposta di `remote.fetch`: i ref dalla risposta (la colonna mostra i branch remoti
    /// nuovi e quanto i branch locali sono indietro), poi log e stato riletti se qualcosa è
    /// cambiato; l'avviso con i ref aggiornati, o l'errore.
    pub(crate) fn receive_fetch(&mut self, generation: u64, done: Done) {
        // Quanti branch remoti ha ogni remoto: un fetch ne porta di nuovi o ne toglie.
        if self.current(generation).is_some() {
            self.request_remotes(generation);
        }
        let Some(repo) = self.current(generation) else {
            return;
        };
        repo.write_label = None;
        let received =
            repo.fetch
                .receive(done.ticket, done.result, done.error_kind, done.error_detail);
        match received {
            fetch::Received::Done(outcome) => {
                log::info!("{}: {}", done.command, outcome.notice);
                let refs_read = outcome.refs.is_some();
                if let Some(refs) = outcome.refs {
                    repo.refs = refs;
                    repo.update_labels();
                }
                self.notice = outcome.notice;
                self.error = outcome.warning.unwrap_or_default();
                if !refs_read {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                }
                if outcome.changed {
                    self.reload_after_fetch(generation);
                }
            }
            fetch::Received::Cancelled { notice } => {
                self.notice = notice;
                self.error.clear();
                // Qualche branch remoto può essere già aggiornato.
                self.submit("repo.refs", json!({}), Request::Refs(generation));
                self.reload_after_fetch(generation);
            }
            fetch::Received::Failed { message, reload } => {
                log::warn!("{}: {message}", done.command);
                self.error = message;
                self.notice.clear();
                if reload {
                    self.submit("repo.refs", json!({}), Request::Refs(generation));
                    self.reload_after_fetch(generation);
                }
            }
        }
    }

    /// Dopo un fetch: lo stato (avanti/indietro rispetto all'upstream) e il log (i commit
    /// nuovi dei branch remoti).
    pub(crate) fn reload_after_fetch(&mut self, generation: u64) {
        self.submit("repo.status", json!({}), Request::Status(generation));
        if let Some(repo) = self.current(generation) {
            let first = repo.list.reset();
            self.request_page(first);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Executor, Wake};
    use project_engine::{
        Capabilities, FetchFailure, MemoryBackend, MemoryOpener, Picked, Platform, Registry,
        Session,
    };
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Come sul web: il job gira dentro `spawn`, sullo stesso thread.
    struct Inline;

    impl Platform for Inline {
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        fn pick_file(&self, on_done: Box<dyn FnOnce(Option<Picked>) + Send>) {
            on_done(None);
        }
        fn save_file(&self, _: &str, _: Vec<u8>) {}
        fn pick_folder(&self, on_done: Box<dyn FnOnce(Option<String>) + Send>) {
            on_done(None);
        }
        fn spawn(&self, job: Box<dyn FnOnce() + Send>) {
            job();
        }
    }

    #[test]
    fn progress_is_described_for_the_bar() {
        let p = Progress {
            phase: "Receiving objects".into(),
            remote: false,
            percent: Some(45),
            current: Some(9),
            total: Some(20),
            done: false,
            text: "Receiving objects:  45% (9/20)".into(),
        };
        assert_eq!(
            describe(&p),
            ("Ricezione oggetti 9/20".to_owned(), Some(0.45))
        );
        let remote = Progress {
            phase: "Enumerating objects".into(),
            remote: true,
            percent: None,
            current: Some(5),
            total: None,
            done: true,
            text: "remote: Enumerating objects: 5, done.".into(),
        };
        assert_eq!(
            describe(&remote),
            ("remoto: Enumerazione oggetti 5".to_owned(), None)
        );
    }

    #[test]
    fn one_fetch_at_a_time_and_cancel_once() {
        let mut f = FetchBox::default();
        assert_eq!(f.request(None), Some(json!({})));
        assert_eq!(
            f.request(Some("origin")),
            Some(json!({ "remote": "origin" }))
        );
        assert_eq!(f.cancel(), None, "niente da interrompere");
        let ticket = Ticket::for_test(4);
        f.started(ticket, Some("origin".to_owned()));
        assert_eq!(f.request(None), None);
        assert_eq!(f.cancel(), Some(ticket));
        assert!(f.running().unwrap().cancelling);
        assert_eq!(f.cancel(), None);
        // Una risposta di un altro ticket non chiude quello in corso.
        let other = f.receive(Ticket::for_test(9), Ok(json!({})), None, None);
        assert!(matches!(other, Received::Failed { reload: true, .. }));
        assert!(f.running().is_some());
        let r = f.receive(
            ticket,
            Err("annullato".into()),
            Some(ErrorKind::Cancelled),
            None,
        );
        let Received::Cancelled { notice } = r else {
            panic!("{r:?}")
        };
        assert!(
            notice.starts_with("Fetch da «origin» interrotto"),
            "{notice}"
        );
        assert!(f.running().is_none());
        assert!(f.failure().is_none());
    }

    #[test]
    fn auth_and_network_errors_are_explained() {
        let mut f = FetchBox::default();
        let ticket = Ticket::for_test(1);
        f.started(ticket, None);
        let r = f.receive(
            ticket,
            Err("autenticazione...".into()),
            Some(ErrorKind::Auth),
            Some("fatal: Authentication failed for 'https://x'".into()),
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert!(!reload);
        assert!(message.contains("autenticazione"), "{message}");
        let failure = f.failure().unwrap();
        assert_eq!(failure.title, "Autenticazione non riuscita");
        assert!(failure.explanation.contains("non chiede mai credenziali"));
        assert!(failure.explanation.contains("credential helper"));
        assert!(failure.explanation.contains("ssh-agent"));
        assert_eq!(
            failure.git.as_deref(),
            Some("fatal: Authentication failed for 'https://x'")
        );
        f.dismiss();
        assert!(f.failure().is_none());

        f.started(ticket, Some("origin".to_owned()));
        f.receive(
            ticket,
            Err("x".into()),
            Some(ErrorKind::RemoteUnreachable),
            Some("fatal: Could not resolve host".into()),
        );
        let failure = f.failure().unwrap();
        assert_eq!(failure.title, "Remoto non raggiungibile");
        assert!(failure.explanation.starts_with("Il remoto «origin»"));

        // Un nuovo fetch toglie la finestra vecchia; un rifiuto va solo nella barra.
        f.started(ticket, Some("altro".to_owned()));
        assert!(f.failure().is_none());
        let r = f.receive(
            ticket,
            Err("non esiste un remoto `altro`".into()),
            Some(ErrorKind::NotAllowed),
            None,
        );
        let Received::Failed { message, reload } = r else {
            panic!("{r:?}")
        };
        assert_eq!(
            message,
            "Fetch da «altro» non possibile: non esiste un remoto `altro`"
        );
        assert!(!reload);
        assert!(f.failure().is_none());
    }

    fn fetch_on(repo: MemoryBackend, ticket: u64) -> (Registry, Session, Received, FetchBox) {
        let mut opener = MemoryOpener::new();
        opener.insert("r", repo);
        let registry = Registry::builtin();
        let mut session = Session::new(Arc::new(opener));
        registry
            .run("repo.open", &mut session, json!({ "path": "r" }))
            .unwrap();
        let mut f = FetchBox::default();
        let params = f.request(None).unwrap();
        let ticket = Ticket::for_test(ticket);
        f.started(ticket, None);
        let result = registry.run(COMMAND, &mut session, params);
        let kind = result.as_ref().err().map(ErrorKind::of);
        let detail = result.as_ref().err().and_then(ErrorKind::detail);
        let received = f.receive(ticket, result.map_err(|e| e.to_string()), kind, detail);
        (registry, session, received, f)
    }

    #[test]
    fn fetch_with_the_sample_repo_updates_the_remote_branch() {
        let (registry, mut session, r, _) = fetch_on(MemoryBackend::sample(), 0);
        let Received::Done(outcome) = r else {
            panic!("{r:?}")
        };
        assert_eq!(
            outcome.notice,
            "Fetch da «origin»: 1 ref aggiornato (origin/feature/grafo)"
        );
        assert!(outcome.changed);
        assert_eq!(outcome.warning, None);
        let refs = outcome.refs.unwrap();
        let grafo = refs
            .iter()
            .find(|r| r.name == "refs/heads/feature/grafo")
            .unwrap();
        // Il branch locale resta dov'era ed è indietro di uno rispetto al remoto.
        assert_eq!(grafo.upstream.as_ref().unwrap().behind, Some(1));

        // Il secondo fetch non ha niente di nuovo.
        let mut f = FetchBox::default();
        let ticket = Ticket::for_test(1);
        let params = f.request(None).unwrap();
        f.started(ticket, None);
        let result = registry.run(COMMAND, &mut session, params);
        let Received::Done(again) =
            f.receive(ticket, result.map_err(|e| e.to_string()), None, None)
        else {
            panic!()
        };
        assert_eq!(again.notice, "Fetch da «origin»: niente di nuovo");
        assert!(!again.changed);
    }

    #[test]
    fn auth_failure_from_the_backend() {
        let mut repo = MemoryBackend::sample();
        repo.fail_fetch(
            "origin",
            Some(FetchFailure::Auth(
                "fatal: could not read Username for 'https://example.com'".into(),
            )),
        );
        let (_, _, r, f) = fetch_on(repo, 0);
        assert!(matches!(r, Received::Failed { reload: false, .. }), "{r:?}");
        let failure = f.failure().unwrap();
        assert_eq!(failure.title, "Autenticazione non riuscita");
        assert_eq!(
            failure.git.as_deref(),
            Some("fatal: could not read Username for 'https://example.com'")
        );
    }

    #[test]
    fn executor_reports_progress_on_the_same_thread() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let wake: Wake = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let session = Session::new(Arc::new(MemoryOpener::sample()));
        let mut ex = Executor::new(session, Registry::builtin(), Arc::new(Inline), wake);
        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        assert!(ex.poll()[0].result.is_ok());
        let before = wakes.load(Ordering::SeqCst);
        // Sul web il fetch finisce dentro `submit`: l'avanzamento arriva prima del risultato,
        // e ognuno chiede un frame.
        let ticket = ex.submit(COMMAND, json!({}));
        assert!(
            wakes.load(Ordering::SeqCst) > before + 1,
            "avanzamento senza frame"
        );
        let last = ex.progress(ticket).expect("ultimo avanzamento");
        assert!(last.done, "{last:?}");
        let done = ex.poll().remove(0);
        assert!(done.result.is_ok(), "{:?}", done.result);
        assert_eq!(ex.progress(ticket), None, "raccolto: niente avanzamento");
        assert!(!ex.cancel(ticket));
    }
}
