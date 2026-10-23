//! Esecuzione dei comandi fuori dal thread della UI (`impl-docs/ARCHITETTURA-git.md` §6).
//!
//! La UI non chiama [`Registry::run`] direttamente: manda il comando all'[`Executor`], che lo
//! esegue con [`Platform::spawn`] sulla sessione condivisa e restituisce il risultato su un
//! canale. La UI legge i risultati con [`Executor::poll`] a ogni frame e non tocca mai il lock
//! della sessione, così non si blocca mentre un comando lento lo tiene.
//!
//! Ogni comando parte con [`Registry::run_with`] e un [`JobContext`] del suo ticket: l'ultimo
//! [`Progress`] ricevuto si legge con [`Executor::progress`], e [`Executor::cancel`] lo
//! annulla (S25, `remote.fetch`). I comandi che non sono lenti ignorano entrambi.

use project_engine::{
    CancelToken, EngineError, JobContext, Platform, Progress, PullOperation, PushRejection,
    Registry, Session,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Identifica un comando mandato all'[`Executor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket(u64);

impl Ticket {
    #[cfg(test)]
    pub(crate) fn for_test(n: u64) -> Self {
        Ticket(n)
    }
}

/// Un comando in corso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub ticket: Ticket,
    /// Id del comando, es. `repo.open`.
    pub command: String,
    /// Vero se il comando modifica il repository ([`CommandSpec::writes`](project_engine::CommandSpec::writes)).
    pub writes: bool,
}

/// Un comando finito.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub ticket: Ticket,
    pub command: String,
    /// Il risultato del comando, o il messaggio d'errore.
    pub result: Result<Value, String>,
    /// Il tipo dell'errore, se il comando è fallito: la UI reagisce in modo diverso, ad
    /// esempio rilegge il diff dopo [`ErrorKind::StaleDiff`].
    pub error_kind: Option<ErrorKind>,
    /// Il dettaglio strutturato dell'errore, se il tipo ne ha uno: il motivo di
    /// [`ErrorKind::ConfirmRequired`], l'output completo di git (stderr) per [`ErrorKind::Git`].
    pub error_detail: Option<String>,
    /// L'id completo da rimandare come `"expected"` con `"confirm": true`, se il comando l'ha
    /// dato con [`ErrorKind::ConfirmRequired`] (il force di `remote.push`).
    pub confirm_expected: Option<String>,
}

/// Il tipo di un errore del motore, per le reazioni della UI che non dipendono dal testo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// [`EngineError::StaleDiff`]: il diff visto non è più quello attuale.
    StaleDiff,
    /// [`EngineError::NotAllowed`]: operazione rifiutata nello stato attuale del repo.
    NotAllowed,
    /// [`EngineError::InvalidParams`].
    InvalidParams,
    /// [`EngineError::IndexLocked`].
    IndexLocked,
    /// [`EngineError::UnknownRev`]: la rev non corrisponde a nessun ref né commit.
    UnknownRev,
    /// [`EngineError::ConfirmRequired`]: serve `"confirm": true`; il motivo è in
    /// [`Done::error_detail`].
    ConfirmRequired,
    /// [`EngineError::NothingToCommit`].
    NothingToCommit,
    /// [`EngineError::Git`]: git è terminato con errore (es. un hook che fallisce); il suo
    /// stderr è in [`Done::error_detail`].
    Git,
    /// [`EngineError::PostCommit`]: il commit è riuscito, ma dopo qualcosa non torna (es. HEAD
    /// spostato da un hook `post-commit`); non va ripetuto, vanno riletti stato e log.
    PostCommit,
    /// [`EngineError::CheckoutConflict`]: il checkout sovrascriverebbe modifiche locali e
    /// niente è cambiato; i file indicati da git sono in [`Done::error_detail`], uno per riga.
    CheckoutConflict,
    /// [`EngineError::Auth`]: il remoto ha rifiutato le credenziali o servirebbe chiederle;
    /// il testo di git è in [`Done::error_detail`].
    Auth,
    /// [`EngineError::RemoteUnreachable`]: il remoto non si raggiunge; il testo di git è in
    /// [`Done::error_detail`].
    RemoteUnreachable,
    /// [`EngineError::Cancelled`]: annullato con [`Executor::cancel`].
    Cancelled,
    /// [`EngineError::PullConflict`]: il merge o il rebase del pull si è fermato in conflitto
    /// ed è stato annullato (il repo è come prima, il fetch resta fatto); i file in conflitto
    /// sono in [`Done::error_detail`], uno per riga.
    PullConflict(PullOperation),
    /// [`EngineError::PushRejected`]: il push è stato rifiutato e niente è cambiato; il testo
    /// di git (anche l'output di un hook) è in [`Done::error_detail`].
    PushRejected(PushRejection),
    /// Tutti gli altri errori.
    Other,
}

impl ErrorKind {
    pub fn of(error: &EngineError) -> Self {
        match error {
            EngineError::StaleDiff { .. } => ErrorKind::StaleDiff,
            EngineError::NotAllowed(_) => ErrorKind::NotAllowed,
            EngineError::InvalidParams { .. } => ErrorKind::InvalidParams,
            EngineError::IndexLocked { .. } => ErrorKind::IndexLocked,
            EngineError::UnknownRev(_) => ErrorKind::UnknownRev,
            EngineError::ConfirmRequired { .. } => ErrorKind::ConfirmRequired,
            EngineError::NothingToCommit => ErrorKind::NothingToCommit,
            EngineError::Git { .. } => ErrorKind::Git,
            EngineError::PostCommit { .. } => ErrorKind::PostCommit,
            EngineError::CheckoutConflict { .. } => ErrorKind::CheckoutConflict,
            EngineError::Auth { .. } => ErrorKind::Auth,
            EngineError::RemoteUnreachable { .. } => ErrorKind::RemoteUnreachable,
            EngineError::Cancelled { .. } => ErrorKind::Cancelled,
            EngineError::PullConflict { operation, .. } => ErrorKind::PullConflict(*operation),
            EngineError::PushRejected { reason, .. } => ErrorKind::PushRejected(*reason),
            _ => ErrorKind::Other,
        }
    }

    /// L'id da mettere in [`Done::confirm_expected`]: l'`expected` di una conferma.
    pub fn expected(error: &EngineError) -> Option<String> {
        match error {
            EngineError::ConfirmRequired { expected, .. } => expected.clone(),
            _ => None,
        }
    }

    /// Il dettaglio da mettere in [`Done::error_detail`]: il motivo di una conferma, lo stderr
    /// di git, i file che un checkout sovrascriverebbe (uno per riga), il testo di git per un
    /// remoto che rifiuta le credenziali o non si raggiunge, i file in conflitto di un pull (uno
    /// per riga), il testo di git di un push rifiutato.
    pub fn detail(error: &EngineError) -> Option<String> {
        match error {
            EngineError::ConfirmRequired { reason, .. } => Some(reason.clone()),
            EngineError::Git { stderr, .. } => Some(stderr.clone()),
            EngineError::Auth { message, .. }
            | EngineError::RemoteUnreachable { message, .. }
            | EngineError::PushRejected { message, .. } => Some(message.clone()),
            EngineError::PullConflict { files, .. } => Some(files.join("\n")),
            EngineError::CheckoutConflict { files, .. } => Some(files.join("\n")),
            _ => None,
        }
    }
}

/// Un errore del comando, come arriva dal job.
struct Failure {
    kind: ErrorKind,
    detail: Option<String>,
    message: String,
    expected: Option<String>,
}

impl Failure {
    fn of(error: &EngineError) -> Self {
        Self {
            kind: ErrorKind::of(error),
            detail: ErrorKind::detail(error),
            message: error.to_string(),
            expected: ErrorKind::expected(error),
        }
    }
}

/// Chiamata quando arriva un risultato, da qualunque thread: nella UI chiede un nuovo frame.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// L'ultimo avanzamento di ogni comando in corso (`None` finché non ne arriva uno). Il sink
/// del job aggiorna solo i ticket presenti: uno già raccolto non torna.
type ProgressMap = Arc<Mutex<HashMap<Ticket, Option<Progress>>>>;

/// Manda comandi alla sessione condivisa e ne raccoglie i risultati.
pub struct Executor {
    session: Arc<Mutex<Session>>,
    registry: Arc<Registry>,
    platform: Arc<dyn Platform>,
    wake: Wake,
    sender: Sender<Done>,
    receiver: Receiver<Done>,
    next: u64,
    running: Vec<Job>,
    progress: ProgressMap,
    cancel: HashMap<Ticket, CancelToken>,
}

impl Executor {
    pub fn new(
        session: Session,
        registry: Registry,
        platform: Arc<dyn Platform>,
        wake: Wake,
    ) -> Self {
        let (sender, receiver) = channel();
        Self {
            session: Arc::new(Mutex::new(session)),
            registry: Arc::new(registry),
            platform,
            wake,
            sender,
            receiver,
            next: 0,
            running: Vec::new(),
            progress: Arc::default(),
            cancel: HashMap::new(),
        }
    }

    /// Manda il comando `command` con `params` e ritorna subito (sul web, dove non ci sono
    /// thread, dopo averlo eseguito). Il risultato arriva da [`Executor::poll`].
    pub fn submit(&mut self, command: &str, params: Value) -> Ticket {
        let ticket = Ticket(self.next);
        self.next += 1;
        let writes = self.registry.get(command).is_some_and(|spec| spec.writes);
        self.running.push(Job {
            ticket,
            command: command.to_owned(),
            writes,
        });

        // Prima di avviare il job: sul web il comando gira (e manda l'avanzamento) dentro
        // `spawn`.
        lock(&self.progress).insert(ticket, None);
        let token = CancelToken::new();
        self.cancel.insert(ticket, token.clone());
        let progress = Arc::clone(&self.progress);
        let wake = Arc::clone(&self.wake);
        let job = JobContext::new()
            .with_cancel(token)
            .with_progress(move |p: &Progress| {
                if let Some(slot) = lock(&progress).get_mut(&ticket) {
                    *slot = Some(p.clone());
                }
                wake();
            });

        let session = Arc::clone(&self.session);
        let registry = Arc::clone(&self.registry);
        let reply = Reply {
            ticket,
            command: command.to_owned(),
            sender: self.sender.clone(),
            wake: Arc::clone(&self.wake),
            sent: false,
        };
        self.platform.spawn(Box::new(move || {
            // Un job finito male avvelena il lock, ma la sessione resta valida: ogni comando
            // la lascia coerente anche quando fallisce.
            let mut session = session.lock().unwrap_or_else(|e| e.into_inner());
            let result = registry
                .run_with(&reply.command, &mut session, params, job)
                .map_err(|e| Failure::of(&e));
            drop(session);
            reply.send(result);
        }));
        ticket
    }

    /// I comandi finiti dall'ultima chiamata, nell'ordine in cui sono finiti. Non blocca.
    pub fn poll(&mut self) -> Vec<Done> {
        let done: Vec<Done> = self.receiver.try_iter().collect();
        self.running
            .retain(|job| done.iter().all(|d| d.ticket != job.ticket));
        if !done.is_empty() {
            let mut progress = lock(&self.progress);
            for d in &done {
                progress.remove(&d.ticket);
                self.cancel.remove(&d.ticket);
            }
        }
        done
    }

    /// L'ultimo avanzamento del comando `ticket`, se è ancora in corso e ne ha mandato uno.
    pub fn progress(&self, ticket: Ticket) -> Option<Progress> {
        lock(&self.progress).get(&ticket).cloned().flatten()
    }

    /// Chiede di annullare il comando `ticket`: un comando lento si ferma appena può e finisce
    /// con [`ErrorKind::Cancelled`]; gli altri finiscono normalmente. Falso se il comando non
    /// è più in corso.
    pub fn cancel(&self, ticket: Ticket) -> bool {
        match self.cancel.get(&ticket) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// I comandi mandati e non ancora raccolti da [`Executor::poll`], dal più vecchio.
    pub fn running(&self) -> &[Job] {
        &self.running
    }

    /// Vero se c'è almeno un comando in corso.
    pub fn is_busy(&self) -> bool {
        !self.running.is_empty()
    }

    /// La scrittura in corso, se c'è: finché non è raccolta da [`Executor::poll`] la UI non ne
    /// manda un'altra e disabilita le azioni che modificano il repo.
    pub fn writing(&self) -> Option<&Job> {
        self.running.iter().find(|job| job.writes)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Manda il risultato di un job. Se il job termina senza mandarlo (panico nel comando, o la
/// piattaforma che non riesce ad avviarlo e lo scarta) manda un errore, così il comando non
/// resta "in corso" per sempre.
struct Reply {
    ticket: Ticket,
    command: String,
    sender: Sender<Done>,
    wake: Wake,
    sent: bool,
}

impl Reply {
    fn send(mut self, result: Result<Value, Failure>) {
        self.deliver(result);
    }

    fn deliver(&mut self, result: Result<Value, Failure>) {
        self.sent = true;
        let (error_kind, error_detail, confirm_expected) = match &result {
            Ok(_) => (None, None, None),
            Err(f) => (Some(f.kind), f.detail.clone(), f.expected.clone()),
        };
        // Se la UI è già chiusa nessuno aspetta il risultato.
        let _ = self.sender.send(Done {
            ticket: self.ticket,
            command: self.command.clone(),
            result: result.map_err(|f| f.message),
            error_kind,
            error_detail,
            confirm_expected,
        });
        (self.wake)();
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if !self.sent {
            self.deliver(Err(Failure {
                kind: ErrorKind::Other,
                detail: None,
                message: format!("`{}` è terminato senza risultato", self.command),
                expected: None,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use project_engine::{Capabilities, CommandSpec, EngineError, MemoryOpener, Picked, Platform};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    const SLOW: Duration = Duration::from_millis(300);

    /// Piattaforma di prova: thread veri come sul desktop, oppure esecuzione immediata come sul
    /// web, oppure job scartati (thread che non parte).
    enum TestPlatform {
        Threads,
        Inline,
        Drop,
    }

    impl Platform for TestPlatform {
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
            match self {
                TestPlatform::Threads => drop(std::thread::spawn(job)),
                TestPlatform::Inline => job(),
                TestPlatform::Drop => drop(job),
            }
        }
    }

    fn slow_write(_: &mut Session, _: Value) -> Result<Value, EngineError> {
        std::thread::sleep(SLOW);
        Ok(json!({}))
    }

    fn slow(_: &mut Session, params: Value) -> Result<Value, EngineError> {
        std::thread::sleep(SLOW);
        Ok(json!({ "echo": params }))
    }

    /// Manda avanzamento finché non è annullato (al massimo 5 secondi).
    fn until_cancelled(s: &mut Session, _: Value) -> Result<Value, EngineError> {
        let start = Instant::now();
        let mut n = 0;
        while start.elapsed() < Duration::from_secs(5) {
            if s.job().is_cancelled() {
                return Err(EngineError::Cancelled {
                    command: "test.cancel",
                });
            }
            n += 1;
            s.job().report(&Progress {
                phase: "Receiving objects".into(),
                remote: false,
                percent: None,
                current: Some(n),
                total: None,
                done: false,
                text: format!("Receiving objects: {n}"),
            });
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(json!({}))
    }

    fn panics(_: &mut Session, _: Value) -> Result<Value, EngineError> {
        panic!("comando finto che va in panico");
    }

    /// Il catalogo vero più `test.slow` (dorme [`SLOW`]), `test.panic` e `test.write` (una
    /// scrittura che dorme [`SLOW`]).
    fn registry() -> Registry {
        let mut registry = Registry::builtin();
        let spec = |id| CommandSpec {
            id,
            title: "prova",
            params_help: "{}",
            writes: false,
        };
        registry.register(spec("test.slow"), slow);
        registry.register(spec("test.panic"), panics);
        registry.register(spec("test.cancel"), until_cancelled);
        registry.register(
            CommandSpec {
                writes: true,
                ..spec("test.write")
            },
            slow_write,
        );
        registry
    }

    fn executor(platform: TestPlatform) -> (Executor, Arc<AtomicUsize>) {
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        let wake: Wake = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        let session = Session::new(Arc::new(MemoryOpener::sample()));
        let executor = Executor::new(session, registry(), Arc::new(platform), wake);
        (executor, wakes)
    }

    /// Chiama `poll` finché arrivano `n` risultati (al massimo 5 secondi).
    fn wait(executor: &mut Executor, n: usize) -> Vec<Done> {
        let start = Instant::now();
        let mut done = Vec::new();
        while done.len() < n {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "risultati mancanti"
            );
            done.extend(executor.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        done
    }

    #[test]
    fn slow_command_does_not_block_the_caller() {
        let (mut ex, wakes) = executor(TestPlatform::Threads);
        let start = Instant::now();
        let ticket = ex.submit("test.slow", json!({ "n": 1 }));
        // La UI continua a disegnare frame mentre il comando lavora.
        assert!(start.elapsed() < SLOW / 2, "submit ha aspettato il comando");
        assert!(ex.is_busy());
        assert_eq!(ex.running()[0].command, "test.slow");
        assert!(ex.poll().is_empty());

        let done = wait(&mut ex, 1);
        assert!(start.elapsed() >= SLOW);
        assert_eq!(done[0].ticket, ticket);
        assert_eq!(done[0].result, Ok(json!({ "echo": { "n": 1 } })));
        assert!(!ex.is_busy());
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn commands_share_the_session() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        ex.submit("test.slow", json!({}));
        let open = ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        // Il lock della sessione mette in fila i comandi: lo stato vede il repo aperto prima.
        let done = wait(&mut ex, 2);
        let status = ex.submit("repo.status", json!({}));
        let after = wait(&mut ex, 1);
        assert!(done.iter().any(|d| d.ticket == open && d.result.is_ok()));
        assert_eq!(after[0].ticket, status);
        assert!(after[0].result.is_ok(), "{:?}", after[0].result);
    }

    #[test]
    fn inline_platform_delivers_before_returning() {
        let (mut ex, _) = executor(TestPlatform::Inline);
        ex.submit("repo.status", json!({}));
        let done = ex.poll();
        assert_eq!(done.len(), 1);
        assert!(done[0].result.as_ref().unwrap_err().contains("repo.open"));
        assert_eq!(done[0].error_kind, Some(ErrorKind::Other));
        assert!(!ex.is_busy());
    }

    #[test]
    fn errors_carry_their_kind() {
        let (mut ex, _) = executor(TestPlatform::Inline);
        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        assert_eq!(ex.poll()[0].error_kind, None);
        ex.submit("index.stage", json!({ "paths": [] }));
        assert_eq!(ex.poll()[0].error_kind, Some(ErrorKind::InvalidParams));
        let stale =
            json!({ "path": "src/lib.rs", "lines": [{ "new": 1 }], "digest": "0".repeat(32) });
        ex.submit("index.stage_lines", stale);
        assert_eq!(ex.poll()[0].error_kind, Some(ErrorKind::StaleDiff));

        ex.submit("commit.create", json!({ "message": "primo" }));
        let done = ex.poll().remove(0);
        assert!(done.result.is_ok(), "{:?}", done.result);
        assert_eq!(done.error_detail, None);
        ex.submit("commit.create", json!({ "message": "ancora" }));
        let done = ex.poll().remove(0);
        assert_eq!(done.error_kind, Some(ErrorKind::NothingToCommit));
        assert_eq!(done.error_detail, None);
    }

    #[test]
    fn confirm_and_git_errors_carry_their_detail() {
        let confirm = EngineError::ConfirmRequired {
            command: "commit.create",
            reason: "il commit è su refs/remotes/origin/main".to_owned(),
            expected: None,
        };
        assert_eq!(ErrorKind::of(&confirm), ErrorKind::ConfirmRequired);
        assert_eq!(
            ErrorKind::detail(&confirm).as_deref(),
            Some("il commit è su refs/remotes/origin/main")
        );
        assert_eq!(ErrorKind::expected(&confirm), None);
        let force = EngineError::ConfirmRequired {
            command: "remote.push",
            reason: "il remoto perderebbe 1 commit".to_owned(),
            expected: Some("a".repeat(40)),
        };
        assert_eq!(ErrorKind::expected(&force), Some("a".repeat(40)));
        let git = EngineError::Git {
            command: "commit".to_owned(),
            stderr: "pre-commit: riga 1\nriga 2".to_owned(),
        };
        assert_eq!(ErrorKind::of(&git), ErrorKind::Git);
        assert_eq!(
            ErrorKind::detail(&git).as_deref(),
            Some("pre-commit: riga 1\nriga 2")
        );
        assert_eq!(ErrorKind::detail(&EngineError::NoRepo), None);
        let post = EngineError::PostCommit {
            commit: None,
            backup: None,
            reason: "HEAD è cambiato".to_owned(),
        };
        assert_eq!(ErrorKind::of(&post), ErrorKind::PostCommit);
        let checkout = EngineError::CheckoutConflict {
            target: "feature/x".to_owned(),
            files: vec!["README.md".to_owned(), "src/lib.rs".to_owned()],
            stderr: "error: ...".to_owned(),
        };
        assert_eq!(ErrorKind::of(&checkout), ErrorKind::CheckoutConflict);
        assert_eq!(
            ErrorKind::detail(&checkout).as_deref(),
            Some("README.md\nsrc/lib.rs")
        );
    }

    #[test]
    fn progress_is_kept_per_ticket_and_cancel_stops_the_job() {
        let (mut ex, wakes) = executor(TestPlatform::Threads);
        let ticket = ex.submit("test.cancel", json!({}));
        let start = Instant::now();
        while ex.progress(ticket).is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "niente avanzamento"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            wakes.load(Ordering::SeqCst) >= 1,
            "l'avanzamento chiede un frame"
        );
        assert_eq!(ex.progress(ticket).unwrap().phase, "Receiving objects");
        assert!(ex.is_busy());
        assert!(ex.cancel(ticket));
        let done = wait(&mut ex, 1).remove(0);
        assert_eq!(done.ticket, ticket);
        assert_eq!(done.error_kind, Some(ErrorKind::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(4), "non si è fermato");
        assert_eq!(ex.progress(ticket), None);
        assert!(!ex.cancel(ticket), "già finito");
    }

    #[test]
    fn a_fetch_cancelled_while_waiting_does_not_start() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        wait(&mut ex, 1);
        // Il fetch aspetta il lock della sessione dietro un comando lento; annullato prima
        // di partire non scarica niente.
        ex.submit("test.slow", json!({}));
        let fetch = ex.submit("remote.fetch", json!({}));
        assert!(ex.writing().is_some_and(|j| j.ticket == fetch));
        assert!(ex.cancel(fetch));
        let done = wait(&mut ex, 2);
        let fetched = done.iter().find(|d| d.ticket == fetch).unwrap();
        assert_eq!(fetched.error_kind, Some(ErrorKind::Cancelled));
        ex.submit("remote.fetch", json!({}));
        let again = wait(&mut ex, 1).remove(0);
        let updated = &again.result.unwrap()["updated"];
        assert_eq!(updated.as_array().map(Vec::len), Some(1), "{updated}");
    }

    #[test]
    fn remote_errors_carry_the_git_message() {
        let auth = EngineError::Auth {
            remote: "origin".into(),
            message: "fatal: Authentication failed".into(),
        };
        assert_eq!(ErrorKind::of(&auth), ErrorKind::Auth);
        assert_eq!(
            ErrorKind::detail(&auth).as_deref(),
            Some("fatal: Authentication failed")
        );
        let net = EngineError::RemoteUnreachable {
            remote: "origin".into(),
            message: "fatal: Could not resolve host".into(),
        };
        assert_eq!(ErrorKind::of(&net), ErrorKind::RemoteUnreachable);
        assert_eq!(
            ErrorKind::detail(&net).as_deref(),
            Some("fatal: Could not resolve host")
        );
        let cancelled = EngineError::Cancelled {
            command: "remote.fetch",
        };
        assert_eq!(ErrorKind::of(&cancelled), ErrorKind::Cancelled);
        assert_eq!(ErrorKind::detail(&cancelled), None);
        let conflict = EngineError::PullConflict {
            branch: "main".into(),
            upstream: "origin/main".into(),
            operation: PullOperation::Rebase,
            files: vec!["a.txt".into(), "b/c.rs".into()],
            message: "CONFLICT (content): ...".into(),
        };
        assert_eq!(
            ErrorKind::of(&conflict),
            ErrorKind::PullConflict(PullOperation::Rebase)
        );
        assert_eq!(
            ErrorKind::detail(&conflict).as_deref(),
            Some("a.txt\nb/c.rs")
        );
        let rejected = EngineError::PushRejected {
            remote: "origin".into(),
            branch: "main".into(),
            reason: PushRejection::Hook,
            message: "pre-push: no".into(),
        };
        assert_eq!(
            ErrorKind::of(&rejected),
            ErrorKind::PushRejected(PushRejection::Hook)
        );
        assert_eq!(
            ErrorKind::detail(&rejected).as_deref(),
            Some("pre-push: no")
        );
    }

    #[test]
    fn a_panicking_command_reports_an_error_and_keeps_the_session() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        ex.submit("test.panic", json!({}));
        let done = wait(&mut ex, 1);
        assert!(done[0].result.is_err());
        assert!(!ex.is_busy());

        ex.submit("repo.open", json!({ "path": MemoryOpener::SAMPLE_PATH }));
        assert!(wait(&mut ex, 1)[0].result.is_ok());
    }

    #[test]
    fn a_dropped_job_reports_an_error() {
        let (mut ex, _) = executor(TestPlatform::Drop);
        ex.submit("repo.status", json!({}));
        let done = ex.poll();
        assert_eq!(done.len(), 1);
        assert!(
            done[0]
                .result
                .as_ref()
                .unwrap_err()
                .contains("senza risultato")
        );
        assert!(!ex.is_busy());
    }

    #[test]
    fn write_jobs_are_tracked_until_collected() {
        let (mut ex, _) = executor(TestPlatform::Threads);
        let read = ex.submit("test.slow", json!({}));
        assert!(ex.writing().is_none(), "una lettura non è una scrittura");
        let write = ex.submit("test.write", json!({}));
        let job = ex.writing().expect("scrittura in corso");
        assert_eq!(job.ticket, write);
        assert!(job.writes);
        assert!(
            !ex.running()
                .iter()
                .find(|j| j.ticket == read)
                .unwrap()
                .writes
        );

        let done = wait(&mut ex, 2);
        assert!(done.iter().all(|d| d.result.is_ok()));
        assert!(ex.writing().is_none());
    }

    #[test]
    fn builtin_write_commands_are_marked() {
        let (mut ex, _) = executor(TestPlatform::Drop);
        ex.submit("index.stage", json!({ "paths": ["a"] }));
        assert!(ex.writing().is_some());
        ex.poll();
        assert!(ex.writing().is_none());
        ex.submit("repo.status", json!({}));
        assert!(ex.writing().is_none());
        ex.submit("comando.inesistente", json!({}));
        assert!(ex.writing().is_none());
    }
}
