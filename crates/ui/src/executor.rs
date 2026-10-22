//! Esecuzione dei comandi fuori dal thread della UI (`impl-docs/ARCHITETTURA-git.md` §6).
//!
//! La UI non chiama [`Registry::run`] direttamente: manda il comando all'[`Executor`], che lo
//! esegue con [`Platform::spawn`] sulla sessione condivisa e restituisce il risultato su un
//! canale. La UI legge i risultati con [`Executor::poll`] a ogni frame e non tocca mai il lock
//! della sessione, così non si blocca mentre un comando lento lo tiene.

use project_engine::{Platform, Registry, Session};
use serde_json::Value;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

/// Identifica un comando mandato all'[`Executor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ticket(u64);

/// Un comando in corso.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub ticket: Ticket,
    /// Id del comando, es. `repo.open`.
    pub command: String,
}

/// Un comando finito.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub ticket: Ticket,
    pub command: String,
    /// Il risultato del comando, o il messaggio d'errore.
    pub result: Result<Value, String>,
}

/// Chiamata quando arriva un risultato, da qualunque thread: nella UI chiede un nuovo frame.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

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
        }
    }

    /// Manda il comando `command` con `params` e ritorna subito (sul web, dove non ci sono
    /// thread, dopo averlo eseguito). Il risultato arriva da [`Executor::poll`].
    pub fn submit(&mut self, command: &str, params: Value) -> Ticket {
        let ticket = Ticket(self.next);
        self.next += 1;
        self.running.push(Job {
            ticket,
            command: command.to_owned(),
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
                .run(&reply.command, &mut session, params)
                .map_err(|e| e.to_string());
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
        done
    }

    /// I comandi mandati e non ancora raccolti da [`Executor::poll`], dal più vecchio.
    pub fn running(&self) -> &[Job] {
        &self.running
    }

    /// Vero se c'è almeno un comando in corso.
    pub fn is_busy(&self) -> bool {
        !self.running.is_empty()
    }
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
    fn send(mut self, result: Result<Value, String>) {
        self.deliver(result);
    }

    fn deliver(&mut self, result: Result<Value, String>) {
        self.sent = true;
        // Se la UI è già chiusa nessuno aspetta il risultato.
        let _ = self.sender.send(Done {
            ticket: self.ticket,
            command: self.command.clone(),
            result,
        });
        (self.wake)();
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if !self.sent {
            self.deliver(Err(format!(
                "`{}` è terminato senza risultato",
                self.command
            )));
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

    fn slow(_: &mut Session, params: Value) -> Result<Value, EngineError> {
        std::thread::sleep(SLOW);
        Ok(json!({ "echo": params }))
    }

    fn panics(_: &mut Session, _: Value) -> Result<Value, EngineError> {
        panic!("comando finto che va in panico");
    }

    /// Il catalogo vero più `test.slow` (dorme [`SLOW`]) e `test.panic`.
    fn registry() -> Registry {
        let mut registry = Registry::builtin();
        let spec = |id| CommandSpec {
            id,
            title: "prova",
            params_help: "{}",
        };
        registry.register(spec("test.slow"), slow);
        registry.register(spec("test.panic"), panics);
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
        assert!(!ex.is_busy());
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
}
