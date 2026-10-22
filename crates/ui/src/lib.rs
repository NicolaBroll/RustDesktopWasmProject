//! L6 — interfaccia egui condivisa da desktop e web.
//!
//! Regola: i callback grafici non modificano il repository direttamente, producono
//! un'[`Action`]; i comandi passano dall'[`Executor`], che li esegue con
//! [`Registry::run`](project_engine::Registry::run) fuori dal thread della UI.
//!
//! Layout (`impl-docs/ARCHITETTURA-git.md` §8): barra in alto, ref a sinistra, commit al
//! centro, dettagli del commit selezionato a destra.

mod details;
mod executor;
mod format;
mod graph;
mod list;
mod view;

pub use executor::{Done, Executor, Job, Ticket, Wake};

use details::DetailsState;
use graph::RefLabel;
use list::{CommitList, PageRequest};
use project_domain::{Commit, CommitDetails, Head, Oid, RefInfo, Status};
use project_engine::{Platform, Registry, RepoOpener, Session};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Ciò che l'utente ha chiesto durante il frame.
#[derive(Debug, Clone)]
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
    /// Sposta la selezione di tante righe (tastiera).
    Move(isize),
    /// Seleziona il primo commit (`true`) o l'ultimo caricato (`false`).
    Edge(bool),
    /// La lista mostra le righe fino a questa (esclusa): se serve chiede la pagina seguente.
    Visible(usize),
    /// Richiede i dettagli del commit selezionato dopo un errore.
    RetryDetails,
}

/// A cosa serve un comando mandato all'executor. Le letture portano la generazione del repo
/// aperto: se nel frattempo se n'è aperto un altro il risultato si scarta.
#[derive(Debug, Clone, Copy)]
enum Request {
    Open,
    Refs(u64),
    Status(u64),
    Log(u64, PageRequest),
    /// `commit.show` del commit indicato.
    Show(u64, Oid),
}

/// Il repo mostrato e i dati letti finora.
struct RepoView {
    path: String,
    head: Head,
    refs: Vec<RefInfo>,
    status: Option<Status>,
    list: CommitList,
    /// Etichette dei ref per commit, rifatte quando cambiano ref o HEAD.
    labels: HashMap<Oid, Vec<RefLabel>>,
    /// Dettagli del commit selezionato (`commit.show`).
    details: DetailsState,
}

impl RepoView {
    fn new(path: String, head: Head) -> Self {
        let mut view = Self {
            path,
            head,
            refs: Vec::new(),
            status: None,
            list: CommitList::default(),
            labels: HashMap::new(),
            details: DetailsState::default(),
        };
        view.update_labels();
        view
    }

    fn update_labels(&mut self) {
        self.labels = graph::ref_labels(&self.refs, &self.head);
    }
}

/// Dove era la lista dei commit nell'ultimo frame: serve per portare in vista la selezione e
/// per sapere quante righe salta Pagina giù.
#[derive(Debug, Default)]
struct ListViewport {
    /// Righe disegnate, anche solo in parte.
    rows: std::ops::Range<usize>,
    height: f32,
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
    viewport: ListViewport,
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
            viewport: ListViewport::default(),
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
                    repo.list.select(index);
                }
            }
            Action::SelectCommit(id) => {
                if let Some(repo) = &mut self.repo
                    && !repo.list.select_id(id)
                {
                    self.error = format!("il commit {} non è tra quelli caricati", id.short());
                }
            }
            Action::Move(delta) => {
                if let Some(repo) = &mut self.repo {
                    repo.list.move_selection(delta);
                }
            }
            Action::Edge(first) => {
                if let Some(repo) = &mut self.repo {
                    repo.list.select_edge(first);
                }
            }
            Action::RetryDetails => {
                if let Some(repo) = &mut self.repo {
                    repo.details.retry();
                }
            }
            Action::Visible(end) => {
                if let Some(request) = self.repo.as_mut().and_then(|r| r.list.next_page(end)) {
                    self.request_page(request);
                }
            }
        }
    }

    fn request_page(&mut self, page: PageRequest) {
        let params = json!({ "skip": page.skip, "limit": page.limit });
        self.submit("repo.log", params, Request::Log(self.generation, page));
    }

    /// Chiede i dettagli del commit selezionato, se è cambiato e non ci sono già.
    fn request_details(&mut self) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        let selected = repo.list.selected_commit().map(|c| c.id);
        if let Some(id) = repo.details.want(selected) {
            let request = Request::Show(self.generation, id);
            self.submit("commit.show", json!({ "id": id.to_string() }), request);
        }
    }

    /// Azioni dei tasti della lista: frecce, Pagina su/giù, Inizio/Fine. Solo se nessun campo
    /// di testo sta ricevendo la tastiera.
    fn keyboard(&self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.repo.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let page = isize::try_from(self.viewport.rows.len().saturating_sub(2).max(1))
            .unwrap_or(isize::MAX);
        ctx.input(|i| {
            let keys = [
                (egui::Key::ArrowDown, Action::Move(1)),
                (egui::Key::ArrowUp, Action::Move(-1)),
                (egui::Key::PageDown, Action::Move(page)),
                (egui::Key::PageUp, Action::Move(-page)),
                (egui::Key::Home, Action::Edge(true)),
                (egui::Key::End, Action::Edge(false)),
            ];
            for (key, action) in keys {
                for _ in 0..i.num_presses(key) {
                    actions.push(action.clone());
                }
            }
        });
    }

    /// Chiede ref, stato e log del repo aperto.
    fn load(&mut self) {
        if self.repo.is_none() {
            return;
        }
        let generation = self.generation;
        self.submit("repo.refs", json!({}), Request::Refs(generation));
        self.submit("repo.status", json!({}), Request::Status(generation));
        if let Some(repo) = &mut self.repo {
            let first = repo.list.reset();
            self.request_page(first);
        }
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
            // I dettagli hanno il loro stato, errori compresi: li mostra il pannello.
            if let Request::Show(generation, id) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<CommitDetails>(value)
                        .map_err(|e| format!("{}: risultato inatteso: {e}", done.command))
                });
                if let Some(repo) = self.current(generation) {
                    repo.details.receive(id, result);
                }
                continue;
            }
            // Una pagina non arrivata si potrà chiedere di nuovo.
            if done.result.is_err()
                && let Request::Log(generation, page) = request
                && let Some(repo) = self.current(generation)
            {
                repo.list.failed(page);
            }
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
                    repo.update_labels();
                }
            }
            Request::Status(generation) => {
                if let Some(repo) = self.current(generation) {
                    let status: Status = serde_json::from_value(value)?;
                    repo.head = status.head.clone();
                    repo.status = Some(status);
                    repo.update_labels();
                }
            }
            Request::Log(generation, page) => {
                if let Some(repo) = self.current(generation) {
                    let LogPage { commits, has_more } = serde_json::from_value(value)?;
                    repo.list.receive(page, commits, has_more);
                }
            }
            // Gestiti a parte in `receive`.
            Request::Show(..) => {}
        }
        Ok(())
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.receive(&ctx);
        let mut actions = Vec::new();
        self.keyboard(&ctx, &mut actions);
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
                    .show(ui, |ui| {
                        view::details_panel(
                            ui,
                            repo.list.selected_commit(),
                            repo.details.shown(),
                            &mut actions,
                        )
                    });
                let viewport = &mut self.viewport;
                egui::CentralPanel::default()
                    .show(ui, |ui| view::commit_list(ui, repo, viewport, &mut actions));
            }
            None => {
                egui::CentralPanel::default().show(ui, |ui| {
                    view::welcome(ui, can_pick, self.executor.is_busy(), &mut actions);
                });
            }
        }

        // La lista ha portato in vista la riga chiesta: non serve più.
        if let Some(repo) = &mut self.repo {
            repo.list.take_scroll_to();
        }
        for action in actions {
            self.apply(&ctx, action);
        }
        // Dopo le azioni: la selezione appena cambiata si chiede in questo stesso frame.
        self.request_details();
    }
}
