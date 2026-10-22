//! L6 — interfaccia egui condivisa da desktop e web.
//!
//! Regola: i callback grafici non modificano il repository direttamente, producono
//! un'[`Action`]; i comandi passano dall'[`Executor`], che li esegue con
//! [`Registry::run`](project_engine::Registry::run) fuori dal thread della UI.
//!
//! Layout (`impl-docs/ARCHITETTURA-git.md` §8): barra in alto, ref a sinistra, commit al
//! centro, dettagli del commit selezionato a destra. In cima alla lista la riga "Modifiche non
//! committate" apre a destra l'area di stage. Il diff di un file si apre al posto del grafo,
//! con il pannello a destra accanto per passare da un file all'altro.

mod details;
mod diff;
mod diff_view;
mod executor;
mod format;
mod graph;
mod list;
mod stage;
mod view;

pub use executor::{Done, Executor, Job, Ticket, Wake};

use details::DetailsState;
use diff::{DiffSource, DiffState, DiffTarget, Pane};
use graph::RefLabel;
use list::{CommitList, PageRequest};
use project_domain::{Commit, CommitDetails, Diff, Head, Oid, RefInfo, RepoPath, Status};
use project_engine::{Platform, Registry, RepoOpener, Session};
use serde::Deserialize;
use serde_json::{Value, json};
use stage::{Follow, PendingWrite, Side, StageLists, StageOp};
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
    /// Seleziona la riga "Modifiche non committate": a destra l'area di stage.
    SelectWorktree,
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
    /// Apre il diff di un file del commit selezionato al posto del grafo.
    OpenDiff(DiffTarget),
    /// Chiude il diff e torna al grafo.
    CloseDiff,
    /// Richiede il diff aperto dopo un errore.
    RetryDiff,
    /// Stage o unstage dei percorsi indicati (mai vuoti). Lo stage di file in conflitto chiede
    /// prima una conferma.
    Write(StageOp, Vec<RepoPath>),
    /// Conferma la scrittura in attesa.
    ConfirmWrite,
    /// Annulla la scrittura in attesa di conferma.
    CancelWrite,
    /// Nasconde l'errore dell'ultima scrittura.
    DismissWriteError,
}

/// A cosa serve un comando mandato all'executor. Le letture portano la generazione del repo
/// aperto: se nel frattempo se n'è aperto un altro il risultato si scarta.
#[derive(Debug, Clone, Copy)]
enum Request {
    /// `index.stage`/`index.unstage`: il risultato è il nuovo stato.
    Write(u64, StageOp),
    Open,
    Refs(u64),
    Status(u64),
    Log(u64, PageRequest),
    /// `commit.show` del commit indicato.
    Show(u64, Oid),
    /// `diff.commit` con il numero di richiesta di [`DiffState`].
    Diff(u64, u64),
}

/// Il repo mostrato e i dati letti finora.
struct RepoView {
    path: String,
    head: Head,
    refs: Vec<RefInfo>,
    status: Option<Status>,
    /// Le voci dello stato divise per l'area di stage.
    stage: StageLists,
    /// Vero se è selezionata la riga "Modifiche non committate" invece di un commit.
    worktree_selected: bool,
    /// Porta in vista la riga delle modifiche al prossimo frame.
    scroll_header: bool,
    /// Scrittura in attesa di conferma.
    confirm: Option<PendingWrite>,
    /// La scrittura in corso, descritta (es. "Stage di note.txt").
    write_label: Option<String>,
    /// L'errore dell'ultima scrittura.
    write_error: Option<String>,
    list: CommitList,
    /// Etichette dei ref per commit, rifatte quando cambiano ref o HEAD.
    labels: HashMap<Oid, Vec<RefLabel>>,
    /// Dettagli del commit selezionato (`commit.show`).
    details: DetailsState,
    /// Diff del file aperto (`diff.commit`), mostrato al posto del grafo.
    diff: DiffState,
}

impl RepoView {
    fn new(path: String, head: Head) -> Self {
        let mut view = Self {
            path,
            head,
            refs: Vec::new(),
            status: None,
            stage: StageLists::default(),
            worktree_selected: false,
            scroll_header: false,
            confirm: None,
            write_label: None,
            write_error: None,
            list: CommitList::default(),
            labels: HashMap::new(),
            details: DetailsState::default(),
            diff: DiffState::default(),
        };
        view.update_labels();
        view
    }

    fn update_labels(&mut self) {
        self.labels = graph::ref_labels(&self.refs, &self.head);
    }

    /// Nuovo stato del repo, da `repo.status` o da una scrittura. Senza modifiche la riga
    /// delle modifiche sparisce: se era selezionata si torna al commit.
    fn set_status(&mut self, status: Status) {
        self.head = status.head.clone();
        self.stage = StageLists::new(&status);
        self.status = Some(status);
        if self.stage.is_empty() {
            self.worktree_selected = false;
            self.confirm = None;
        }
        self.update_labels();
    }

    /// Vero se c'è la riga "Modifiche non committate" in cima alla lista.
    fn has_changes(&self) -> bool {
        !self.stage.is_empty()
    }

    /// Seleziona la riga delle modifiche (`true`) o torna ai commit.
    fn select_worktree(&mut self, on: bool) {
        self.worktree_selected = on && self.has_changes();
        if self.worktree_selected {
            self.scroll_header = true;
        }
    }

    /// Frecce e Pagina su/giù: la riga delle modifiche sta sopra il primo commit.
    fn move_selection(&mut self, delta: isize) {
        if self.worktree_selected {
            if delta > 0 && !self.list.commits().is_empty() {
                self.worktree_selected = false;
                self.list.select(0);
                self.list.move_selection(delta - 1);
            }
            return;
        }
        let above_top = self
            .list
            .selected()
            .is_none_or(|i| i.checked_add_signed(delta).is_none());
        if delta < 0 && above_top && self.has_changes() {
            self.select_worktree(true);
        } else {
            self.list.move_selection(delta);
        }
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
                    repo.select_worktree(false);
                    repo.list.select(index);
                }
            }
            Action::SelectWorktree => {
                if let Some(repo) = &mut self.repo {
                    repo.select_worktree(true);
                }
            }
            Action::SelectCommit(id) => {
                if let Some(repo) = &mut self.repo {
                    if repo.list.select_id(id) {
                        repo.select_worktree(false);
                    } else {
                        self.error = format!("il commit {} non è tra quelli caricati", id.short());
                    }
                }
            }
            Action::Move(delta) => {
                if let Some(repo) = &mut self.repo {
                    repo.move_selection(delta);
                }
            }
            Action::Edge(first) => {
                if let Some(repo) = &mut self.repo {
                    if first && repo.has_changes() {
                        repo.select_worktree(true);
                    } else {
                        repo.select_worktree(false);
                        repo.list.select_edge(first);
                    }
                }
            }
            Action::Write(op, paths) => {
                let Some(repo) = &mut self.repo else {
                    return;
                };
                if self.executor.writing().is_some() || repo.confirm.is_some() {
                    return;
                }
                if let Some(write) = stage::plan_write(&repo.stage, op, paths) {
                    if write.needs_confirm() {
                        repo.confirm = Some(write);
                    } else {
                        self.send_write(write);
                    }
                }
            }
            Action::ConfirmWrite => {
                if let Some(write) = self.repo.as_mut().and_then(|r| r.confirm.take()) {
                    self.send_write(write);
                }
            }
            Action::CancelWrite => {
                if let Some(repo) = &mut self.repo {
                    repo.confirm = None;
                }
            }
            Action::DismissWriteError => {
                if let Some(repo) = &mut self.repo {
                    repo.write_error = None;
                }
            }
            Action::RetryDetails => {
                if let Some(repo) = &mut self.repo {
                    repo.details.retry();
                }
            }
            Action::OpenDiff(target) => {
                if let Some(seq) = self.repo.as_mut().and_then(|r| r.diff.open(target.clone())) {
                    self.request_diff(&target, seq);
                }
            }
            Action::CloseDiff => {
                if let Some(repo) = &mut self.repo {
                    repo.diff.close();
                }
            }
            Action::RetryDiff => {
                let repo = self.repo.as_mut();
                if let Some((seq, target)) = repo.and_then(|r| {
                    let seq = r.diff.retry()?;
                    Some((seq, r.diff.target()?.clone()))
                }) {
                    self.request_diff(&target, seq);
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

    fn request_diff(&mut self, target: &DiffTarget, seq: u64) {
        let (command, params) = target.request();
        self.submit(command, params, Request::Diff(self.generation, seq));
    }

    /// Manda una scrittura sull'indice, se non ce n'è già una in corso: mai due insieme.
    fn send_write(&mut self, write: PendingWrite) {
        if self.executor.writing().is_some() || write.paths.is_empty() {
            return;
        }
        let generation = self.generation;
        let Some(repo) = &mut self.repo else {
            return;
        };
        repo.write_error = None;
        repo.write_label = Some(write.op.describe(&write.paths));
        let params = json!({ "paths": write.paths });
        self.submit(
            write.op.command(),
            params,
            Request::Write(generation, write.op),
        );
    }

    /// Dopo un nuovo stato, il diff aperto di worktree o indice si richiede; se il file è
    /// passato tutto nell'altra lista si apre quel diff, se non ha più modifiche si chiude.
    fn follow_diff(&mut self) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        let Some(target) = repo.diff.target().cloned() else {
            return;
        };
        let side = match target.source {
            DiffSource::Worktree => Side::Unstaged,
            DiffSource::Index => Side::Staged,
            DiffSource::Commit(_) => return,
        };
        let request = match stage::follow(&repo.stage, side, &target.path) {
            Follow::Reload => repo.diff.reload().map(|seq| (seq, target)),
            Follow::Switch(other) => {
                let target = DiffTarget {
                    source: side_source(other),
                    path: target.path,
                };
                repo.diff.open(target.clone()).map(|seq| (seq, target))
            }
            Follow::Close => {
                repo.diff.close();
                None
            }
        };
        if let Some((seq, target)) = request {
            self.request_diff(&target, seq);
        }
    }

    /// Chiude il diff se non è più di ciò che è selezionato: il diff di un commit vale finché
    /// è selezionato quel commit, quelli di worktree e indice finché è selezionata la riga
    /// delle modifiche.
    fn close_stale_diff(&mut self) {
        if let Some(repo) = &mut self.repo {
            let selected = repo.list.selected_commit().map(|c| c.id);
            let stale = repo.diff.target().is_some_and(|t| match t.source {
                DiffSource::Commit(id) => repo.worktree_selected || Some(id) != selected,
                DiffSource::Worktree | DiffSource::Index => !repo.worktree_selected,
            });
            if stale {
                repo.diff.close();
            }
        }
    }

    /// Chiede i dettagli del commit selezionato, se è cambiato e non ci sono già.
    fn request_details(&mut self) {
        let Some(repo) = &mut self.repo else {
            return;
        };
        let selected = repo
            .list
            .selected_commit()
            .map(|c| c.id)
            .filter(|_| !repo.worktree_selected);
        if let Some(id) = repo.details.want(selected) {
            let request = Request::Show(self.generation, id);
            self.submit("commit.show", json!({ "id": id.to_string() }), request);
        }
    }

    /// Azioni dei tasti della lista: frecce, Pagina su/giù, Inizio/Fine. Con il diff aperto,
    /// Esc lo chiude e le frecce restano alla vista del diff. Solo se nessun campo di testo sta
    /// ricevendo la tastiera.
    fn keyboard(&self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(repo) = &self.repo else {
            return;
        };
        // Con la conferma aperta i tasti vanno solo a lei.
        if ctx.egui_wants_keyboard_input() || repo.confirm.is_some() {
            return;
        }
        if repo.diff.target().is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                actions.push(Action::CloseDiff);
            }
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
            // Le scritture: il nuovo stato, oppure l'errore nell'area di stage.
            if let Request::Write(generation, op) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<Status>(value)
                        .map_err(|e| format!("{}: risultato inatteso: {e}", done.command))
                });
                let Some(repo) = self.current(generation) else {
                    continue;
                };
                let label = repo
                    .write_label
                    .take()
                    .unwrap_or_else(|| op.command().to_owned());
                match result {
                    Ok(status) => {
                        repo.set_status(status);
                        self.error.clear();
                        self.follow_diff();
                    }
                    Err(e) => {
                        log::warn!("{}: {e}", done.command);
                        let message = format!("{label} non riuscito: {e}");
                        repo.write_error = Some(message.clone());
                        self.error = message;
                    }
                }
                continue;
            }
            // Anche il diff: errori nella sua vista, risposte vecchie scartate.
            if let Request::Diff(generation, seq) = request {
                let result = done.result.and_then(|value| {
                    serde_json::from_value::<Diff>(value)
                        .map_err(|e| format!("{}: risultato inatteso: {e}", done.command))
                });
                if let Some(repo) = self.current(generation) {
                    repo.diff.receive(seq, result);
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
                    repo.set_status(status);
                    self.follow_diff();
                }
            }
            Request::Log(generation, page) => {
                if let Some(repo) = self.current(generation) {
                    let LogPage { commits, has_more } = serde_json::from_value(value)?;
                    repo.list.receive(page, commits, has_more);
                }
            }
            // Gestiti a parte in `receive`.
            Request::Show(..) | Request::Diff(..) | Request::Write(..) => {}
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
        let writing = self.executor.writing().is_some();

        egui::Panel::top("barra").show(ui, |ui| {
            view::top_bar(ui, self.repo.as_ref(), can_pick, &mut actions);
        });
        egui::Panel::bottom("stato").show(ui, |ui| {
            let write = self.repo.as_ref().and_then(|r| r.write_label.as_deref());
            view::status_bar(ui, self.executor.running(), write, &self.error);
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
                        if repo.worktree_selected {
                            view::stage_panel(ui, repo, writing, &mut actions);
                        } else {
                            view::details_panel(
                                ui,
                                repo.list.selected_commit(),
                                repo.details.shown(),
                                repo.diff.target(),
                                &mut actions,
                            );
                        }
                    });
                if let Some(write) = &repo.confirm {
                    view::confirm_write(&ctx, write, &mut actions);
                }
                let viewport = &mut self.viewport;
                egui::CentralPanel::default().show(ui, |ui| match repo.diff.pane() {
                    Pane::Closed => view::commit_list(ui, repo, viewport, &mut actions),
                    pane => diff_view::diff_panel(ui, pane, &mut actions),
                });
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
            repo.scroll_header = false;
        }
        for action in actions {
            self.apply(&ctx, action);
        }
        // Dopo le azioni: la selezione appena cambiata si chiede in questo stesso frame.
        self.close_stale_diff();
        self.request_details();
    }
}

/// Il diff di un file della lista `side`.
fn side_source(side: Side) -> DiffSource {
    match side {
        Side::Staged => DiffSource::Index,
        Side::Unstaged => DiffSource::Worktree,
    }
}
