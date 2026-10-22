//! L6 — interfaccia egui condivisa da desktop e web.
//!
//! Regola: i callback grafici non modificano il repository direttamente, producono
//! un'[`Action`]; i comandi passano dall'[`Executor`], che li esegue con [`Registry::run`] fuori
//! dal thread della UI.
//!
//! Per ora è provvisoria: il client git (vedi `impl-docs/ROADMAP-git.md`) mostra il repo aperto
//! e i comandi in corso finché non arriva il layout principale (step S11).

mod executor;

pub use executor::{Done, Executor, Job, Ticket, Wake};

use project_engine::{Platform, Registry, RepoOpener, Session};
use serde_json::{Value, json};
use std::sync::Arc;

/// Ciò che l'utente ha chiesto durante il frame.
enum Action {
    Run(&'static str, Value),
    Quit,
}

pub struct App {
    executor: Executor,
    platform: Arc<dyn Platform>,
    /// Risposta dell'ultimo `repo.open` riuscito: `{"path", "head"}`.
    opened: Option<Value>,
    status: String,
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
            opened: None,
            status: String::new(),
        };
        if let Some(path) = repo {
            app.apply(
                &cc.egui_ctx,
                Action::Run("repo.open", json!({ "path": path })),
            );
        }
        app
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Run(id, params) => {
                self.executor.submit(id, params);
            }
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn receive(&mut self) {
        for done in self.executor.poll() {
            match done.result {
                Ok(out) => {
                    if done.command == "repo.open" {
                        self.opened = Some(out);
                    }
                    self.status.clear();
                }
                Err(e) => {
                    log::warn!("{}: {e}", done.command);
                    self.status = e;
                }
            }
        }
    }

    fn menu_bar(&self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        if !self.platform.capabilities().window_control {
            return;
        }
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Esci").clicked() {
                    actions.push(Action::Quit);
                }
            });
        });
    }

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(job) = self.executor.running().first() {
                ui.spinner();
                let others = self.executor.running().len() - 1;
                let text = match others {
                    0 => format!("{} in corso…", job.command),
                    n => format!("{} in corso… (+{n} in coda)", job.command),
                };
                ui.label(text);
            } else if !self.status.is_empty() {
                ui.colored_label(ui.visuals().error_fg_color, &self.status);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::warn_if_debug_build(ui);
            });
        });
    }

    fn placeholder(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.heading("Client git in costruzione");
            ui.add_space(8.0);
            match &self.opened {
                Some(opened) => {
                    let path = opened["path"].as_str().unwrap_or_default();
                    ui.label(format!("Repository aperto: {path}"));
                    if let Some(branch) = opened["head"]["Branch"]["name"].as_str() {
                        ui.weak(format!("Branch: {branch}"));
                    } else if let Some(id) = opened["head"]["Detached"].as_str() {
                        ui.weak(format!("HEAD staccata su {}", &id[..id.len().min(7)]));
                    }
                }
                None => {
                    ui.weak("Nessun repository aperto.");
                }
            }
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.receive();
        let mut actions = Vec::new();

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::default().show(ui, |ui| self.placeholder(ui));

        let ctx = ui.ctx().clone();
        for action in actions {
            self.apply(&ctx, action);
        }
    }
}
