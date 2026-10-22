//! L6 — interfaccia egui condivisa da desktop e web.
//!
//! Regola: i callback grafici non modificano il repository direttamente, producono
//! un'[`Action`]; i comandi passano dall'[`Executor`], che li esegue con
//! [`Registry::run`](project_engine::Registry::run) fuori dal thread della UI.
//!
//! Layout (`impl-docs/ARCHITETTURA-git.md` §8): barra in alto, ref a sinistra, commit al
//! centro, dettagli del commit selezionato a destra.

mod executor;
mod format;
mod view;

pub use executor::{Done, Executor, Job, Ticket, Wake};

use project_domain::{Commit, Head, Oid, RefInfo, Status};
use project_engine::{Platform, Registry, RepoOpener, Session};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Commit chiesti a `repo.log` per la lista (le pagine successive arrivano con lo step S12).
const LOG_PAGE: usize = 1000;

/// Ciò che l'utente ha chiesto durante il frame.
enum Action {
    /// Dialogo per scegliere la cartella del repo.
    PickFolder,
    Open(String),
    /// Rilegge ref, stato e log del repo aperto.
    Refresh,
    /// Seleziona il commit alla riga indicata della lista.
    Select(usize),
    /// Seleziona il commit puntato da un ref, se è nella lista.
    SelectCommit(Oid),
}

/// A cosa serve un comando mandato all'executor. Le letture portano la generazione del repo
/// aperto: se nel frattempo se n'è aperto un altro il risultato si scarta.
#[derive(Debug, Clone, Copy)]
enum Request {
    Open,
    Refs(u64),
    Status(u64),
    Log(u64),
}

/// Il repo mostrato e i dati letti finora.
struct RepoView {
    path: String,
    head: Head,
    refs: Vec<RefInfo>,
    status: Option<Status>,
    commits: Vec<Commit>,
    has_more: bool,
    /// Indice in `commits`.
    selected: Option<usize>,
}

impl RepoView {
    fn new(path: String, head: Head) -> Self {
        Self {
            path,
            head,
            refs: Vec::new(),
            status: None,
            commits: Vec::new(),
            has_more: false,
            selected: None,
        }
    }

    fn selected_commit(&self) -> Option<&Commit> {
        self.commits.get(self.selected?)
    }
}

#[derive(Deserialize)]
struct Opened {
    path: String,
    head: Head,
}

#[derive(Deserialize)]
struct LogPage {
    commits: Vec<Commit>,
    has_more: bool,
}

pub struct App {
    executor: Executor,
    platform: Arc<dyn Platform>,
    /// Cartella scelta nel dialogo; sul desktop arriva subito, in generale da un callback.
    picked: Arc<Mutex<Option<String>>>,
    pending: HashMap<Ticket, Request>,
    /// Cresce a ogni repo aperto.
    generation: u64,
    repo: Option<RepoView>,
    error: String,
}

impl App {
    /// `opener` apre i repository: `CliOpener` sul desktop, `MemoryOpener` sul web. Se c'è,
    /// `repo` si apre subito, in background.
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        platform: Arc<dyn Platform>,
        opener: Arc<dyn RepoOpener>,
        repo: Option<String>,
    ) -> Self {
        let ctx = cc.egui_ctx.clone();
        let wake: Wake = Arc::new(move || ctx.request_repaint());
        let mut app = Self {
            executor: Executor::new(
                Session::new(opener),
                Registry::builtin(),
                Arc::clone(&platform),
                wake,
            ),
            platform,
            picked: Arc::default(),
            pending: HashMap::new(),
            generation: 0,
            repo: None,
            error: String::new(),
        };
        if let Some(path) = repo {
            app.apply(&cc.egui_ctx, Action::Open(path));
        }
        app
    }

    fn submit(&mut self, command: &str, params: Value, request: Request) {
        let ticket = self.executor.submit(command, params);
        self.pending.insert(ticket, request);
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::PickFolder => {
                let picked = Arc::clone(&self.picked);
                let ctx = ctx.clone();
                self.platform.pick_folder(Box::new(move |folder| {
                    if let Some(folder) = folder {
                        *picked.lock().unwrap_or_else(|e| e.into_inner()) = Some(folder);
                        ctx.request_repaint();
                    }
                }));
            }
            Action::Open(path) => self.submit("repo.open", json!({ "path": path }), Request::Open),
            Action::Refresh => self.load(),
            Action::Select(index) => {
                if let Some(repo) = &mut self.repo {
                    repo.selected = Some(index);
                }
            }
            Action::SelectCommit(id) => {
                if let Some(repo) = &mut self.repo
                    && let Some(index) = repo.commits.iter().position(|c| c.id == id)
                {
                    repo.selected = Some(index);
                }
            }
        }
    }

    /// Chiede ref, stato e log del repo aperto.
    fn load(&mut self) {
        if self.repo.is_none() {
            return;
        }
        let generation = self.generation;
        self.submit("repo.refs", json!({}), Request::Refs(generation));
        self.submit("repo.status", json!({}), Request::Status(generation));
        let params = json!({ "limit": LOG_PAGE });
        self.submit("repo.log", params, Request::Log(generation));
    }

    fn receive(&mut self, ctx: &egui::Context) {
        let folder = self.picked.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(folder) = folder {
            self.apply(ctx, Action::Open(folder));
        }
        for done in self.executor.poll() {
            let Some(request) = self.pending.remove(&done.ticket) else {
                continue;
            };
            match done.result {
                Ok(value) => {
                    if let Err(e) = self.store(request, value) {
                        log::error!("{}: risultato inatteso: {e}", done.command);
                        self.error = format!("{}: risultato inatteso: {e}", done.command);
                    } else {
                        self.error.clear();
                    }
                }
                Err(e) => {
                    log::warn!("{}: {e}", done.command);
                    self.error = e;
                }
            }
        }
    }

    /// Il repo mostrato, se è ancora quello della generazione `generation`.
    fn current(&mut self, generation: u64) -> Option<&mut RepoView> {
        self.repo.as_mut().filter(|_| generation == self.generation)
    }

    /// Mette il risultato di `request` nella vista.
    fn store(&mut self, request: Request, value: Value) -> Result<(), serde_json::Error> {
        match request {
            Request::Open => {
                let Opened { path, head } = serde_json::from_value(value)?;
                self.generation += 1;
                self.repo = Some(RepoView::new(path, head));
                self.load();
            }
            Request::Refs(generation) => {
                if let Some(repo) = self.current(generation) {
                    repo.refs = serde_json::from_value(value)?;
                }
            }
            Request::Status(generation) => {
                if let Some(repo) = self.current(generation) {
                    let status: Status = serde_json::from_value(value)?;
                    repo.head = status.head.clone();
                    repo.status = Some(status);
                }
            }
            Request::Log(generation) => {
                if let Some(repo) = self.current(generation) {
                    let LogPage { commits, has_more } = serde_json::from_value(value)?;
                    // Dopo un aggiornamento resta selezionato lo stesso commit, se c'è ancora.
                    let selected = repo.selected_commit().map(|c| c.id);
                    repo.selected = selected
                        .and_then(|id| commits.iter().position(|c| c.id == id))
                        .or((!commits.is_empty()).then_some(0));
                    repo.commits = commits;
                    repo.has_more = has_more;
                }
            }
        }
        Ok(())
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        let mut actions = Vec::new();
        let can_pick = self.platform.capabilities().open_folder;

        egui::Panel::top("barra").show(ui, |ui| {
            view::top_bar(ui, self.repo.as_ref(), can_pick, &mut actions);
        });
        egui::Panel::bottom("stato").show(ui, |ui| {
            view::status_bar(ui, self.executor.running(), &self.error);
        });
        // I pannelli laterali non superano un terzo della finestra: il contenuto va a capo.
        let side_max = (ui.available_width() / 3.0).max(200.0);
        match &self.repo {
            Some(repo) => {
                egui::Panel::left("ref")
                    .resizable(true)
                    .default_size(220.0)
                    .min_size(140.0)
                    .max_size(side_max)
                    .show(ui, |ui| view::refs_panel(ui, repo, &mut actions));
                egui::Panel::right("dettagli")
                    .resizable(true)
                    .default_size(320.0)
                    .min_size(200.0)
                    .max_size(side_max)
                    .show(ui, |ui| view::details_panel(ui, repo.selected_commit()));
                egui::CentralPanel::default()
                    .show(ui, |ui| view::commit_list(ui, repo, &mut actions));
            }
            None => {
                egui::CentralPanel::default().show(ui, |ui| {
                    view::welcome(ui, can_pick, self.executor.is_busy(), &mut actions);
                });
            }
        }

        for action in actions {
            self.apply(&ctx, action);
        }
    }
}
