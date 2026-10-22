//! L6 — interfaccia egui condivisa da desktop e web.
//!
//! Regola: i callback grafici non modificano il repository direttamente,
//! producono un'[`Action`] che passa da [`Registry::run`].
//!
//! Per ora è provvisoria: il client git (vedi `impl-docs/ROADMAP-git.md`) mostra un messaggio
//! finché non arriva il layout principale (step S11).

use project_engine::{Platform, Registry, RepoOpener, Session};
use std::sync::Arc;

/// Ciò che l'utente ha chiesto durante il frame.
enum Action {
    Quit,
}

pub struct App {
    session: Session,
    registry: Registry,
    platform: Arc<dyn Platform>,
}

impl App {
    /// `opener` apre i repository: `CliOpener` sul desktop, `MemoryOpener` sul web.
    pub fn new(
        _cc: &eframe::CreationContext<'_>,
        platform: Arc<dyn Platform>,
        opener: Arc<dyn RepoOpener>,
    ) -> Self {
        Self {
            session: Session::new(opener),
            registry: Registry::builtin(),
            platform,
        }
    }

    fn apply(&mut self, ctx: &egui::Context, action: Action) {
        match action {
            Action::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
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

    fn placeholder(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.heading("Client git in costruzione");
            ui.add_space(8.0);
            match self.session.path() {
                Some(path) => ui.label(format!("Repository aperto: {path}")),
                None => ui.weak("Nessun repository aperto."),
            };
            let commands: Vec<_> = self.registry.list().map(|c| c.id).collect();
            ui.weak(format!("Comandi disponibili: {}", commands.join(", ")));
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let mut actions = Vec::new();

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui, &mut actions));
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::warn_if_debug_build(ui);
            });
        });
        egui::CentralPanel::default().show(ui, |ui| self.placeholder(ui));

        let ctx = ui.ctx().clone();
        for action in actions {
            self.apply(&ctx, action);
        }
    }
}
